// Generated from mouseless.toml by platform/install-extension.sh.
//
// This checked-in copy carries the built-in defaults so the headless smoke
// test can pack the extension without a system install. The installer
// regenerates it in the packed extension from the installed TOML, keeping the
// socket path and legacy-payload appearance fallback in sync.

function hex(value) {
    const number = Number.parseInt(value.slice(1), 16);
    return [(number >> 16) & 0xff, (number >> 8) & 0xff, number & 0xff];
}

export const CONFIGURED_SOCKET_PATH = '/run/mouseless/mouselessd.sock';

export const CONFIGURED_APPEARANCE = {
    grid_color: hex('#d4de04'),
    grid_opacity: 0.2,
    label_color: hex('#ebe9e8'),
    highlight_color: hex('#ffb22c'),
};
