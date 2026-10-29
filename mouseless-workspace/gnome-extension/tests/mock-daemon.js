// Mock mouselessd for the headless Shell smoke test (SPEC-overlay #8).
//
// Speaks the same NDJSON v1 contract as the daemon and drives every frame the
// extension must render: Show, HighlightCell, ShowSubgrid, QueryCursor, Hide.
// After Hide it closes the socket so the extension exercises EOF-hide and a
// 500 ms reconnect; the second Hello proves the reconnect and replay path.
//
// Run with: MOUSELESS_SOCKET=/tmp/x.sock gjs -m tests/mock-daemon.js

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const path = GLib.getenv('MOUSELESS_SOCKET');
if (!path) {
    printerr('MOUSELESS_SOCKET not set');
    imports.system.exit(2);
}
try {
    Gio.File.new_for_path(path).delete(null);
} catch {
    // Fresh path: nothing to remove.
}

const monitor = {x: 0, y: 0, w: 1920, h: 1080, scale: 1.0};
const keys = ['a', 's', 'd', 'f', 'j', 'k', 'l', ';'];
const cells = keys.map((label, i) => ({
    index: i,
    label,
    x: (i % 4) * 480,
    y: Math.floor(i / 4) * 540,
    w: 480,
    h: 540,
}));
const subCells = keys.map((label, i) => ({
    index: i,
    label,
    x: (i % 4) * 120,
    y: Math.floor(i / 4) * 270,
    w: 120,
    h: 270,
}));

const service = new Gio.SocketService();
service.add_address(Gio.UnixSocketAddress.new(path), Gio.SocketType.STREAM,
    Gio.SocketProtocol.DEFAULT, null);

let connections = 0;
service.connect('incoming', (service_, connection) => {
    const id = ++connections;
    print(`mock: client ${id} connected`);
    const input = new Gio.DataInputStream({
        base_stream: connection.get_input_stream(),
    });
    const output = connection.get_output_stream();
    const send = payload => {
        const frame = JSON.stringify({
            schema_version: 1,
            kind: 'daemon',
            payload,
        });
        try {
            output.write_all(new TextEncoder().encode(`${frame}\n`), null);
            print(`mock: sent ${payload.type}`);
        } catch (error) {
            print(`mock: send failed (${error.message})`);
        }
    };
    const later = (delay, fn) => GLib.timeout_add(GLib.PRIORITY_DEFAULT, delay, () => {
        fn();
        return GLib.SOURCE_REMOVE;
    });

    const readNext = () => input.read_line_async(GLib.PRIORITY_DEFAULT, null,
        (stream, result) => {
            let line;
            try {
                [line] = stream.read_line_finish_utf8(result);
            } catch (error) {
                print(`mock: read failed (${error.message})`);
                return;
            }
            if (line === null) {
                print(`mock: client ${id} EOF`);
                return;
            }
            let envelope;
            try {
                envelope = JSON.parse(line);
            } catch {
                print('mock: malformed client frame');
                readNext();
                return;
            }
            const type = envelope?.payload?.type;
            print(`mock: got ${type}`);
            if (type === 'Hello') {
                // Replay current state (the real daemon does this too).
                send({
                    type: 'Show',
                    model: {monitor, cells, highlight: null},
                });
                if (id === 1) {
                    later(400, () => send({type: 'HighlightCell', index: 5}));
                    later(800, () => send({
                        type: 'ShowSubgrid',
                        parent: 0,
                        model: {monitor, cells: subCells, highlight: null},
                    }));
                    later(1200, () => send({type: 'QueryCursor'}));
                    later(1600, () => send({type: 'Hide'}));
                    // EOF after Hide: extension must hide and reconnect.
                    later(2500, () => {
                        print('mock: closing client 1 to test EOF-hide');
                        connection.close(null);
                    });
                } else {
                    // Reconnect proof: Show again, then close once more.
                    later(300, () => connection.close(null));
                }
            }
            readNext();
        });
    readNext();
    return true;
});
service.start();
print(`mock: listening on ${path}`);

// Outlive at least one 500 ms reconnect after the first EOF.
const loop = GLib.MainLoop.new(null, false);
GLib.timeout_add(GLib.PRIORITY_DEFAULT, 15000, () => {
    loop.quit();
    return GLib.SOURCE_REMOVE;
});
loop.run();
