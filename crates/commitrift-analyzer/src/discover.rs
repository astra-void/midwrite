//! Source file discovery.

use std::path::{Component, Path, PathBuf};

use walkdir::WalkDir;

use crate::error::DiscoveryError;

/// Directory names skipped during recursive discovery. Explicitly named
/// files are never skipped.
pub const DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".output",
    ".turbo",
    ".vercel",
    ".svelte-kit",
    "coverage",
];

const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts"];
const DECLARATION_SUFFIXES: &[&str] = &[".d.ts", ".d.mts", ".d.cts"];

/// Whether `path` names a TypeScript source file CommitRift analyzes
/// (`.ts`, `.tsx`, `.mts`, `.cts`, excluding declaration files).
pub fn is_source_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let has_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext));
    has_extension
        && !DECLARATION_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Expand `inputs` (files or directories) to a sorted, de-duplicated list of
/// source files.
pub fn discover(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, DiscoveryError> {
    discover_excluding(inputs, &|_| false)
}

/// Like [`discover`], skipping files under directory inputs whose path
/// relative to that directory (with `/` separators) satisfies `excluded`.
pub fn discover_excluding(
    inputs: &[PathBuf],
    excluded: &dyn Fn(&str) -> bool,
) -> Result<Vec<PathBuf>, DiscoveryError> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_file() {
            if !is_source_file(input) {
                return Err(DiscoveryError::Unsupported(input.clone()));
            }
            files.push(normalize(input));
        } else if input.is_dir() {
            let walker = WalkDir::new(input)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|entry| {
                    entry.depth() == 0
                        || !(entry.file_type().is_dir()
                            && entry
                                .file_name()
                                .to_str()
                                .is_some_and(|name| DEFAULT_EXCLUDED_DIRS.contains(&name)))
                });
            for entry in walker {
                let entry = entry.map_err(|source| DiscoveryError::Walk {
                    path: input.clone(),
                    source,
                })?;
                if entry.file_type().is_file() && is_source_file(entry.path()) {
                    let relative = entry
                        .path()
                        .strip_prefix(input)
                        .map(display_path)
                        .unwrap_or_default();
                    if !excluded(&relative) {
                        files.push(normalize(entry.path()));
                    }
                }
            }
        } else {
            return Err(DiscoveryError::NotFound(input.clone()));
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Drop `.` components so that `./src/a.ts` and `src/a.ts` compare equal.
fn normalize(path: &Path) -> PathBuf {
    let normalized: PathBuf = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

/// The path as shown in reports: forward slashes, no leading `./`.
pub fn display_path(path: &Path) -> String {
    let text = normalize(path).to_string_lossy().into_owned();
    if std::path::MAIN_SEPARATOR == '\\' {
        text.replace('\\', "/")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_typescript_sources() {
        assert!(is_source_file(Path::new("a/b.ts")));
        assert!(is_source_file(Path::new("page.tsx")));
        assert!(is_source_file(Path::new("x.mts")));
        assert!(!is_source_file(Path::new("types.d.ts")));
        assert!(!is_source_file(Path::new("index.js")));
        assert!(!is_source_file(Path::new("README.md")));
    }

    #[test]
    fn display_strips_current_dir() {
        assert_eq!(display_path(Path::new("./src/a.ts")), "src/a.ts");
        assert_eq!(display_path(Path::new("src/./a.ts")), "src/a.ts");
    }
}
