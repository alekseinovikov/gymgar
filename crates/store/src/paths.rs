//! Where Gymgar keeps its data on disk.
//!
//! Only computes paths; creating the directories is [`crate::Store::open`]'s job. A debug build gets its own
//! directory so that developing never touches the data of an installed release.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::StoreError;

/// File name of the database inside the data directory.
const DB_FILE_NAME: &str = "gymgar.sqlite3";

// Spelled out per OS instead of through `ProjectDirs::from`: `from` rewrites the name (`Gymgar Dev` becomes
// `Gymgar-Dev` on macOS, `gymgardev` on Linux), and the spec names these directories exactly.
#[cfg(any(target_os = "macos", windows))]
const DIR_NAME: &str = if cfg!(debug_assertions) { "Gymgar Dev" } else { "Gymgar" };
#[cfg(not(any(target_os = "macos", windows)))]
const DIR_NAME: &str = if cfg!(debug_assertions) { "gymgar-dev" } else { "gymgar" };

/// The per-user data directory (`~/Library/Application Support/Gymgar` on macOS, `~/.local/share/gymgar` on Linux,
/// `%APPDATA%\Gymgar\data` on Windows).
pub fn data_dir() -> Result<PathBuf, StoreError> {
    let dirs = ProjectDirs::from_path(PathBuf::from(DIR_NAME)).ok_or_else(|| StoreError::Open {
        path: PathBuf::from(DIR_NAME),
        reason: "cannot determine the user data directory: the operating system reports no home directory".to_owned(),
    })?;
    Ok(dirs.data_dir().to_path_buf())
}

/// The database file inside `data_dir`.
pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DB_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_build_uses_dev_dir() {
        // No home directory (a bare CI container) means no data directory; nothing to assert then.
        let Ok(dir) = data_dir() else { return };
        // `directories` appends a `data` component on Windows only.
        let project_dir = if cfg!(windows) { dir.parent().map(Path::to_path_buf).unwrap_or_default() } else { dir };
        let expected = if cfg!(any(target_os = "macos", windows)) { "Gymgar Dev" } else { "gymgar-dev" };
        assert_eq!(project_dir.file_name().and_then(|name| name.to_str()), Some(expected));
    }

    #[test]
    fn db_lives_in_the_data_dir() {
        assert_eq!(db_path(Path::new("data")), Path::new("data").join("gymgar.sqlite3"));
    }
}
