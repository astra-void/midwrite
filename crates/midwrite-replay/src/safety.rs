//! Safety measures around a harness process: private replay directories,
//! database target checks and report redaction. OS isolation is in
//! [`crate::isolation`]. None of this is a sandbox for hostile code (see
//! `docs/reference/security.md`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A private (0700) directory for one replay invocation, short enough for
/// Unix socket paths (104 bytes on macOS), removed on drop unless kept.
#[derive(Debug)]
pub struct ReplayRoot {
    pub path: PathBuf,
    pub keep: bool,
}

impl ReplayRoot {
    pub fn create(keep: bool) -> Result<Self, String> {
        let base = {
            let temp = std::env::temp_dir();
            if temp.as_os_str().len() <= 40 {
                temp
            } else {
                PathBuf::from("/tmp")
            }
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let name = format!(
            "cr-{}-{:x}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = base.join(name);
        create_private_dir(&path)?;
        let path = path
            .canonicalize()
            .map_err(|e| format!("cannot resolve the replay directory: {e}"))?;
        Ok(Self { path, keep })
    }
}

impl Drop for ReplayRoot {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

pub fn create_private_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(path)
        .map_err(|e| format!("cannot create `{}`: {e}", path.display()))
}

/// Check that every database setting handed to the harness points inside
/// the replay root. Defense in depth against a supervisor bug: the values
/// are built by the supervisor, never read from the environment.
pub fn check_database_target(root: &Path, socket: &Path) -> Result<(), String> {
    if socket.starts_with(root) {
        Ok(())
    } else {
        Err(format!(
            "refusing to hand the harness a database socket outside the replay directory ({})",
            socket.display()
        ))
    }
}

/// Remove volatile and sensitive text from harness output before it enters
/// a report: the replay root (a temporary path) and credentials embedded in
/// URLs (`scheme://user:password@`).
pub fn redact(text: &str, root: &Path) -> String {
    let root = root.to_string_lossy();
    let mut text = text.replace(root.as_ref(), "<replay-root>");
    // /tmp and /private/tmp are the same directory on macOS.
    if let Some(stripped) = root.strip_prefix("/private") {
        text = text.replace(stripped, "<replay-root>");
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("://") {
        let (head, tail) = rest.split_at(start + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '/' || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) if authority[..at].contains(':') => {
                let user = &authority[..authority.find(':').unwrap_or(0)];
                out.push_str(user);
                out.push_str(":***");
                out.push_str(&authority[at..]);
            }
            _ => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_removes_passwords_and_temporary_paths() {
        let root = Path::new("/private/tmp/cr-1-2-3");
        let text = "connecting to postgres://admin:hunter2@prod.example.com:5432/app from /tmp/cr-1-2-3/run-1 and /private/tmp/cr-1-2-3/x";
        let redacted = redact(text, root);
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(redacted.contains("postgres://admin:***@prod.example.com:5432/app"));
        assert!(!redacted.contains("cr-1-2-3"), "{redacted}");
        assert_eq!(
            redact("https://example.com/a", root),
            "https://example.com/a"
        );
    }

    #[test]
    fn database_targets_outside_the_root_are_refused() {
        let root = Path::new("/tmp/cr-root");
        assert!(check_database_target(root, Path::new("/tmp/cr-root/pg/.s.PGSQL.5432")).is_ok());
        assert!(check_database_target(root, Path::new("/tmp/.s.PGSQL.5432")).is_err());
        assert!(
            check_database_target(root, Path::new("/var/run/postgresql/.s.PGSQL.5432")).is_err()
        );
    }

    #[test]
    fn replay_roots_are_private_short_and_removed() {
        let root = ReplayRoot::create(false).unwrap();
        let path = root.path.clone();
        assert!(path.is_dir());
        assert!(path.join("pg/.s.PGSQL.5432").as_os_str().len() < 100);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "not accessible to other users");
        drop(root);
        assert!(!path.exists());
    }
}
