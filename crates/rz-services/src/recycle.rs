//! Moving a file to the system's trash, so deleting an archive can be undone.

use std::path::Path;

/// Sends a file to the Recycle Bin / Trash. Returns a readable reason on failure.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
}
