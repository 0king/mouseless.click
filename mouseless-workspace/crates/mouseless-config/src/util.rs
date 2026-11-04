//! Small filesystem helpers shared by the store and presets modules.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Write `bytes` to `path` atomically: write a sibling temp file, fsync, then
/// rename over the destination. A crash can leave the temp file but never a
/// partially written config (SPEC-settings lifecycle).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid file name: {}", path.display()))?;
    let temp = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| -> Result<(), String> {
        let mut file =
            fs::File::create(&temp).map_err(|e| format!("create {}: {e}", temp.display()))?;
        file.write_all(bytes)
            .map_err(|e| format!("write {}: {e}", temp.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync {}: {e}", temp.display()))?;
        drop(file);
        fs::rename(&temp, path)
            .map_err(|e| format!("rename {} -> {}: {e}", temp.display(), path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Copy `from` to `to` atomically (used for the last-good backup).
pub(crate) fn copy_atomic(from: &Path, to: &Path) -> Result<(), String> {
    let bytes = fs::read(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    write_atomic(to, &bytes)
}
