//! Crash-safe file writes.
//!
//! Every persisted file stages through a sibling temp file that is flushed to
//! disk and then renamed over the target. A rename within a directory is atomic,
//! so a crash, a kill, or an abort mid-write leaves either the previous file or
//! the new one — never a truncated one.
//!
//! This matters because `std::fs::write` truncates the target *before* writing:
//! lyrfin saves the user's ratings, playlists and history on the way out, and
//! release builds run with `panic = "abort"`, so a panic on any thread during
//! shutdown would otherwise leave a half-written (or empty) file behind.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// The sibling temp path a write stages through. It keeps the target's full name
/// (plus `.tmp`) so two files in one directory can never stage through the same
/// path, and it sits in the target's directory so the rename stays on one
/// filesystem — a cross-device rename is neither atomic nor even permitted.
fn tmp_path(path: &Path) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(".tmp");
    PathBuf::from(raw)
}

/// Owner-only permissions for a file holding credentials (Unix). Applied to the
/// temp file *before* the rename, so the target is never briefly world-readable.
#[cfg(unix)]
fn restrict(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_file: &File) -> io::Result<()> {
    Ok(()) // Windows inherits the user-profile ACL; there is no mode to set
}

/// Write `contents` to `path` atomically, creating the parent directory.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_with(path, false, |w| w.write_all(contents))
}

/// [`write`] for text, so callers with a `String` don't spell out `.as_bytes()`.
pub fn write_str(path: &Path, contents: &str) -> io::Result<()> {
    write(path, contents.as_bytes())
}

/// [`write`] for a credential file (tokens, client id): the result is readable
/// only by its owner on Unix.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_with(path, true, |w| w.write_all(contents))
}

/// The general form: `fill` streams the new contents into a buffered writer over
/// the temp file. Used by the binary library cache, which encodes straight into
/// the writer rather than building the whole blob in memory first.
///
/// The data is flushed and `fsync`ed before the rename — without that the rename
/// can land while the contents are still only in the page cache, which is exactly
/// the truncated-file case this module exists to prevent.
pub fn write_with<F>(path: &Path, private: bool, fill: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> io::Result<()>,
{
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_path(path);
    let result = (|| -> io::Result<()> {
        let file = File::create(&tmp)?;
        if private {
            restrict(&file)?;
        }
        let mut w = BufWriter::new(file);
        fill(&mut w)?;
        w.flush()?;
        w.into_inner()
            .map_err(|e| io::Error::other(e.to_string()))?
            .sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp); // never leave a stale .tmp behind
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lyrfin-atomic-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn write_replaces_the_previous_contents_and_creates_the_directory() {
        let dir = tmpdir("basic").join("nested");
        let path = dir.join("store.json");
        write_str(&path, "first").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "first");
        // a shorter payload must not leave a tail of the longer previous one
        write_str(&path, "b").expect("rewrite");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "b");
        assert!(
            !tmp_path(&path).exists(),
            "the temp file is renamed away, never left behind"
        );
    }

    #[test]
    fn the_temp_file_never_collides_between_siblings() {
        // `with_extension("tmp")` would map store.json and store.bin onto ONE temp
        // path, so two saves racing would corrupt each other.
        let a = tmp_path(Path::new("/x/store.json"));
        let b = tmp_path(Path::new("/x/store.bin"));
        assert_ne!(a, b);
        assert_eq!(a, PathBuf::from("/x/store.json.tmp"));
    }

    #[test]
    fn write_with_streams_and_renames() {
        let path = tmpdir("stream").join("library.bin");
        write_with(&path, false, |w| w.write_all(&[1, 2, 3])).expect("write");
        assert_eq!(std::fs::read(&path).expect("read"), vec![1, 2, 3]);
    }

    /// A failure mid-fill must leave the *previous* file intact and clean up the
    /// temp — the property the whole module exists for.
    #[test]
    fn a_failed_write_keeps_the_previous_file() {
        let path = tmpdir("failed").join("store.json");
        write_str(&path, "good").expect("seed");
        let err = write_with(&path, false, |_| Err(io::Error::other("encoder blew up")));
        assert!(err.is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "good",
            "the previous contents survive a failed rewrite"
        );
        assert!(!tmp_path(&path).exists(), "the temp file is cleaned up");
    }

    #[cfg(unix)]
    #[test]
    fn credential_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmpdir("private").join("spotify_token.json");
        write_private(&path, b"{\"access_token\":\"secret\"}").expect("write");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "tokens must not be world-readable");
    }
}
