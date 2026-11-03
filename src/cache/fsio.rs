//! Private, atomic JSON files for the caches (#226).

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Read `path` as JSON. Missing, unreadable or corrupt is `None`.
pub fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Write `value` to `path` as JSON: to a per-process temp file in the same
/// directory, then renamed over, so a crash never leaves a half-written file.
/// The directory is created `0700` and the file `0600` on Unix.
pub fn write<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(value)?;
    write_private(&tmp, &bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // A directory made before this existed (or by another tool) is tightened.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    // The per-user cache directory is already private on Windows.
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_value_reads_back_and_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/x.json");
        write(&path, &vec![1, 2, 3]).unwrap();
        assert_eq!(read::<Vec<i32>>(&path), Some(vec![1, 2, 3]));
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("x.json")]);
    }

    #[test]
    fn missing_and_corrupt_files_read_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.json");
        assert_eq!(read::<Vec<i32>>(&path), None);
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(read::<Vec<i32>>(&path), None);
    }

    #[cfg(unix)]
    #[test]
    fn files_and_their_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("org");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.join("x.json");
        write(&path, &1).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
    }
}
