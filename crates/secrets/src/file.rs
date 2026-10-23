//! File-backed secret store: one secret per file, `0600` on Unix.
//!
//! Fallback for machines without a usable system secret store. It does not encrypt: the file mode is the
//! only protection, so prefer [`crate::SystemStore`] when it is available. Never logs or reports a secret.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};

use crate::SecretsError;

/// Upper bound for a stored secret. A real API key is ~132 bytes; the cap only keeps a corrupted or
/// hostile file from being read into memory whole.
const MAX_SECRET_BYTES: u64 = 16 * 1024;

/// A secret kept in a single file.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// A store backed by the file at `path`. Nothing is touched until `get`, `set` or `delete`.
    pub fn new(path: PathBuf) -> FileStore {
        FileStore { path }
    }

    /// The stored secret, or `None` when the file does not exist.
    pub fn get(&self) -> Result<Option<SecretString>, SecretsError> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error("cannot open", &self.path, &e)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_SECRET_BYTES + 1).read_to_end(&mut bytes).map_err(|e| io_error("cannot read", &self.path, &e))?;
        if too_large(bytes.len()) {
            return Err(SecretsError::Io(format!("secret file {} is too large", self.path.display())));
        }
        // The decoding error owns the file bytes, so it is dropped here instead of being reported.
        let text = String::from_utf8(bytes).map_err(|_| SecretsError::Io(format!("secret file {} is not valid UTF-8", self.path.display())))?;
        Ok(Some(SecretString::from(text)))
    }

    /// Stores `secret`, replacing any previous one. The new content is written to a temporary file next to
    /// the target and renamed over it, so a crash never leaves a half-written secret behind.
    pub fn set(&self, secret: &SecretString) -> Result<(), SecretsError> {
        let bytes = secret.expose_secret().as_bytes();
        if too_large(bytes.len()) {
            return Err(SecretsError::Io(format!("secret is too large to store (limit {MAX_SECRET_BYTES} bytes)")));
        }
        let name = self.path.file_name().ok_or_else(|| SecretsError::Io(format!("secret file path {} has no file name", self.path.display())))?;
        let dir = self.path.parent().unwrap_or(Path::new(""));
        fs::create_dir_all(dir).map_err(|e| io_error("cannot create directory", dir, &e))?;

        // The process id keeps two Gymgar processes (app and CLI) from clobbering each other's temp file.
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(".{}.tmp", std::process::id()));
        let tmp = dir.join(tmp_name);

        if let Err(e) = write_private(&tmp, bytes) {
            // Best effort: the write error is the one worth reporting.
            let _ = fs::remove_file(&tmp);
            return Err(io_error("cannot write", &tmp, &e));
        }
        if let Err(e) = fs::rename(&tmp, &self.path) {
            let _ = fs::remove_file(&tmp);
            return Err(io_error("cannot replace", &self.path, &e));
        }
        Ok(())
    }

    /// Removes the stored secret. A missing file is not an error.
    pub fn delete(&self) -> Result<(), SecretsError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error("cannot delete", &self.path, &e)),
        }
    }
}

fn too_large(len: usize) -> bool {
    u64::try_from(len).map_or(true, |len| len > MAX_SECRET_BYTES)
}

/// Creates `path` readable and writable by the owner only (Unix), writes `bytes` and flushes them to disk.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // A stale temp file from a crashed run may have looser permissions, and `mode` only applies on creation.
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Error text names the operation and the path, never the content.
fn io_error(action: &str, path: &Path, source: &io::Error) -> SecretsError {
    SecretsError::Io(format!("{action} {}: {source}", path.display()))
}

#[cfg(test)]
mod tests {
    use secrecy::{ExposeSecret, SecretString};
    use tempfile::TempDir;

    use super::*;

    fn store_in(dir: &TempDir) -> FileStore {
        FileStore::new(dir.path().join("key"))
    }

    fn secret(value: &str) -> SecretString {
        SecretString::from(value.to_owned())
    }

    #[test]
    fn file_round_trip() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        store.set(&secret("abc-123")).unwrap();
        assert_eq!(store.get().unwrap().unwrap().expose_secret(), "abc-123");
    }

    #[test]
    fn file_get_missing_is_none() {
        let dir = TempDir::new().unwrap();
        assert!(store_in(&dir).get().unwrap().is_none());
    }

    #[test]
    fn file_delete_missing_is_ok() {
        let dir = TempDir::new().unwrap();
        store_in(&dir).delete().unwrap();
    }

    #[test]
    fn file_set_replaces_and_delete_removes() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        store.set(&secret("first")).unwrap();
        store.set(&secret("second")).unwrap();
        assert_eq!(store.get().unwrap().unwrap().expose_secret(), "second");
        store.delete().unwrap();
        assert!(store.get().unwrap().is_none());
        // No temporary files are left next to the secret.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn file_keeps_long_keys() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let alphabet: Vec<char> = ('A'..='Z').chain('a'..='z').chain('0'..='9').chain(['.', '_', '-']).collect();
        let long: String = (0..300).map(|i| alphabet[i % alphabet.len()]).collect();
        assert_eq!(long.len(), 300);
        store.set(&secret(&long)).unwrap();
        assert_eq!(store.get().unwrap().unwrap().expose_secret(), long);
    }

    #[test]
    fn file_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let store = FileStore::new(dir.path().join("a").join("b").join("key"));
        store.set(&secret("nested")).unwrap();
        assert_eq!(store.get().unwrap().unwrap().expose_secret(), "nested");
    }

    #[cfg(unix)]
    #[test]
    fn file_mode_is_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        store.set(&secret("private")).unwrap();
        let mode = std::fs::metadata(dir.path().join("key")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // Overwriting keeps the file private too.
        store.set(&secret("still private")).unwrap();
        let mode = std::fs::metadata(dir.path().join("key")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn file_rejects_non_utf8_without_leaking_bytes() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("key"), [0x73, 0x65, 0x63, 0xff, 0xfe]).unwrap();
        let err = store_in(&dir).get().unwrap_err();
        assert!(matches!(err, SecretsError::Io(_)));
        assert!(err.to_string().contains("not valid UTF-8"), "{err}");
    }

    #[test]
    fn file_rejects_oversized_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("key"), vec![b'x'; 1024 * 1024]).unwrap();
        let err = store_in(&dir).get().unwrap_err();
        assert!(matches!(err, SecretsError::Io(_)));
        assert!(err.to_string().contains("too large"), "{err}");
        let huge = secret(&"x".repeat(1024 * 1024));
        let err = store_in(&dir).set(&huge).unwrap_err();
        assert!(matches!(err, SecretsError::Io(_)));
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[test]
    fn debug_does_not_leak() {
        let dir = TempDir::new().unwrap();
        let value = "super-secret-value-0123456789";
        // A regular file in place of the parent directory makes `set` fail.
        std::fs::write(dir.path().join("blocker"), b"x").unwrap();
        let store = FileStore::new(dir.path().join("blocker").join("key"));
        let err = store.set(&secret(value)).unwrap_err();
        assert!(matches!(err, SecretsError::Io(_)));
        assert!(err.to_string().contains("cannot create directory"), "{err}");
        assert!(!format!("{err:?}").contains(value));
        assert!(!err.to_string().contains(value));
        assert!(!format!("{:?}", SecretsError::Unavailable("store locked".into())).contains(value));
        assert!(!format!("{:?}", secret(value)).contains(value));
    }
}
