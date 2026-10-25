//! Module resolution for cross-module call summaries.
//!
//! A deliberately small subset of TypeScript's resolution, applied only to
//! the files being analyzed:
//!
//! * relative specifiers (`./order`, `../services/order.js`);
//! * `compilerOptions.paths` and `compilerOptions.baseUrl` of the nearest
//!   `tsconfig.json` above the importing file, following relative `extends`
//!   chains (patterns with at most one `*`, longest prefix wins, targets
//!   tried in order);
//! * workspace packages: a bare specifier whose package name is the `name`
//!   of a `package.json` that contains analyzed files (a monorepo package),
//!   resolved through its `exports` map (string targets, single-`*`
//!   patterns, and the conditions `types`, `import`, `module`, `default`,
//!   `require`, `node`, in that order) or, without `exports`, through
//!   `main`/`module`/`types` or the subpath itself;
//! * extension candidates `.ts`, `.tsx`, `/index.ts`, `/index.tsx`, and
//!   `.js`/`.jsx`/`.mjs`/`.cjs` specifiers mapped to their TypeScript
//!   sources.
//!
//! Anything else (`node_modules`, packages outside the analyzed files,
//! `typesVersions`, project references) is reported as a package import and
//! not followed. When more than one candidate file exists for a specifier,
//! the import is reported as ambiguous instead of picking one.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Where an import specifier leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// A file of the project, by index.
    File(usize),
    /// A bare specifier not matched by `paths`/`baseUrl`: a package.
    Package,
    /// A relative or aliased specifier that leads to no analyzed file.
    NotFound(String),
    /// Several analyzed files match; none is chosen.
    Ambiguous(Vec<String>),
}

const TS_EXTENSIONS: [&str; 4] = ["ts", "tsx", "mts", "cts"];
const JS_TO_TS: [(&str, &[&str]); 4] = [
    ("js", &["ts", "tsx"]),
    ("jsx", &["tsx"]),
    ("mjs", &["mts"]),
    ("cjs", &["cts"]),
];

/// `compilerOptions` relevant to resolution, with paths made absolute.
#[derive(Clone, Debug, Default)]
struct PathOptions {
    base_url: Option<PathBuf>,
    /// `(pattern, targets)`; targets are absolute patterns.
    paths: Vec<(String, Vec<String>)>,
}

/// A `package.json` whose directory contains analyzed files.
#[derive(Clone, Debug)]
struct WorkspacePackage {
    name: String,
    directory: PathBuf,
    exports: Option<Value>,
    /// `main`, `module` and `types`, in that order.
    entries: Vec<String>,
}

pub(crate) struct Resolver {
    /// Absolute, lexically normalized path of every analyzed file.
    files: HashMap<PathBuf, usize>,
    /// Display path of every analyzed file, by index.
    names: Vec<String>,
    /// Nearest `tsconfig.json` options, by directory.
    configs: HashMap<PathBuf, Option<PathOptions>>,
    packages: Vec<WorkspacePackage>,
}

impl Resolver {
    /// `files[i]` is the absolute normalized path of file `i`, `names[i]`
    /// its display path.
    pub fn new(files: &[PathBuf], names: Vec<String>) -> Self {
        Self {
            files: files
                .iter()
                .enumerate()
                .map(|(index, path)| (path.clone(), index))
                .collect(),
            names,
            configs: HashMap::new(),
            packages: workspace_packages(files),
        }
    }

    /// Resolve `specifier` imported by the file at absolute path `importer`.
    pub fn resolve(&mut self, importer: &Path, specifier: &str) -> Resolution {
        let directory = importer.parent().unwrap_or(Path::new("/"));
        if specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier == "."
            || specifier == ".."
        {
            return self.candidates(&normalize(&directory.join(specifier)), specifier);
        }
        if specifier.starts_with('/') {
            return Resolution::NotFound(format!(
                "`{specifier}` is an absolute path, which CommitRift does not resolve"
            ));
        }
        let Some(options) = self.options(directory) else {
            return self.workspace(specifier);
        };
        if let Some((pattern, targets, star)) = match_paths(&options.paths, specifier) {
            let mut tried = Vec::new();
            for target in targets {
                let substituted = target.replace('*', star);
                match self.candidates(Path::new(&substituted), specifier) {
                    Resolution::NotFound(_) => tried.push(substituted),
                    found => return found,
                }
            }
            return Resolution::NotFound(format!(
                "`{specifier}` matches the tsconfig `paths` pattern `{pattern}`, but no analyzed file exists at {}",
                tried.join(", ")
            ));
        }
        if let Some(base_url) = &options.base_url {
            match self.candidates(&normalize(&base_url.join(specifier)), specifier) {
                Resolution::NotFound(_) => {}
                found => return found,
            }
        }
        self.workspace(specifier)
    }

    /// A bare specifier naming a workspace package (or a subpath of one).
    fn workspace(&self, specifier: &str) -> Resolution {
        let Some(package) = self
            .packages
            .iter()
            .filter(|package| {
                specifier == package.name
                    || specifier
                        .strip_prefix(&package.name)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|package| package.name.len())
        else {
            return Resolution::Package;
        };
        let subpath = match &specifier[package.name.len()..] {
            "" => ".".to_owned(),
            rest => format!(".{rest}"),
        };
        let targets: Vec<String> = match &package.exports {
            Some(exports) => export_targets(exports, &subpath),
            None if subpath == "." => {
                let mut targets = package.entries.clone();
                targets.push("./index".to_owned());
                targets
            }
            None => vec![subpath.clone()],
        };
        let mut tried = Vec::new();
        for target in &targets {
            match self.candidates(&normalize(&package.directory.join(target)), specifier) {
                Resolution::NotFound(_) => tried.push(target.clone()),
                found => return found,
            }
        }
        Resolution::NotFound(if targets.is_empty() {
            format!(
                "`{specifier}` names the workspace package `{}`, whose `exports` do not provide `{subpath}`",
                package.name
            )
        } else {
            format!(
                "`{specifier}` names the workspace package `{}`, but no analyzed file exists at {}",
                package.name,
                tried.join(", ")
            )
        })
    }

    /// The analyzed file a resolved path (without or with an extension)
    /// denotes.
    fn candidates(&self, base: &Path, specifier: &str) -> Resolution {
        let extension = base.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mut candidates: Vec<PathBuf> = Vec::new();
        if TS_EXTENSIONS.contains(&extension) {
            candidates.push(base.to_path_buf());
        } else if let Some((_, replacements)) = JS_TO_TS.iter().find(|(js, _)| *js == extension) {
            for replacement in *replacements {
                candidates.push(base.with_extension(replacement));
            }
        } else {
            for extension in ["ts", "tsx"] {
                let mut name = base.as_os_str().to_owned();
                name.push(format!(".{extension}"));
                candidates.push(PathBuf::from(name));
            }
            for index in ["index.ts", "index.tsx"] {
                candidates.push(base.join(index));
            }
        }
        let found: Vec<usize> = candidates
            .iter()
            .filter_map(|candidate| self.files.get(candidate).copied())
            .collect();
        match found.as_slice() {
            [] => Resolution::NotFound(format!(
                "`{specifier}` does not lead to an analyzed TypeScript file"
            )),
            [file] => Resolution::File(*file),
            files => {
                Resolution::Ambiguous(files.iter().map(|&file| self.names[file].clone()).collect())
            }
        }
    }

    /// The resolution options of the nearest `tsconfig.json` at or above
    /// `directory`.
    fn options(&mut self, directory: &Path) -> Option<PathOptions> {
        if let Some(cached) = self.configs.get(directory) {
            return cached.clone();
        }
        let config = directory.join("tsconfig.json");
        let options = if config.is_file() {
            load_options(&config, 0)
        } else {
            match directory.parent() {
                Some(parent) if parent != directory => self.options(parent),
                _ => None,
            }
        };
        self.configs
            .insert(directory.to_path_buf(), options.clone());
        options
    }
}

/// Conditions tried, in order, when an `exports` target is an object.
const CONDITIONS: [&str; 6] = ["types", "import", "module", "default", "require", "node"];

/// The targets `exports` gives for `subpath` (`.` or `./x`), most preferred
/// first.
fn export_targets(exports: &Value, subpath: &str) -> Vec<String> {
    let map = match exports {
        Value::Object(map) if map.keys().all(|key| key.starts_with('.')) => map,
        // A string, an array, or a conditions object: the `.` export.
        other => {
            return if subpath == "." {
                condition_targets(other, 0)
            } else {
                Vec::new()
            };
        }
    };
    if let Some(target) = map.get(subpath) {
        return condition_targets(target, 0);
    }
    // Single-`*` patterns, longest prefix first.
    let mut best: Option<(usize, &Value, &str)> = None;
    for (key, target) in map {
        if let Some((prefix, suffix)) = key.split_once('*')
            && subpath.len() >= prefix.len() + suffix.len()
            && subpath.starts_with(prefix)
            && subpath.ends_with(suffix)
            && best.is_none_or(|(length, ..)| prefix.len() > length)
        {
            best = Some((
                prefix.len(),
                target,
                &subpath[prefix.len()..subpath.len() - suffix.len()],
            ));
        }
    }
    best.map(|(_, target, star)| {
        condition_targets(target, 0)
            .into_iter()
            .map(|t| t.replace('*', star))
            .collect()
    })
    .unwrap_or_default()
}

fn condition_targets(target: &Value, depth: usize) -> Vec<String> {
    if depth > 4 {
        return Vec::new();
    }
    match target {
        Value::String(path) => vec![path.clone()],
        Value::Array(items) => items
            .iter()
            .flat_map(|item| condition_targets(item, depth + 1))
            .collect(),
        Value::Object(conditions) => CONDITIONS
            .iter()
            .filter_map(|condition| conditions.get(*condition))
            .flat_map(|item| condition_targets(item, depth + 1))
            .collect(),
        _ => Vec::new(),
    }
}

/// The `package.json` files nearest to the analyzed files (at most 32
/// levels up), with a `name`. Packages under `node_modules` never contain
/// analyzed files.
fn workspace_packages(files: &[PathBuf]) -> Vec<WorkspacePackage> {
    let mut seen: HashMap<PathBuf, bool> = HashMap::new();
    let mut packages = Vec::new();
    for file in files {
        let mut directory = file.parent();
        for _ in 0..32 {
            let Some(dir) = directory else { break };
            if let Some(&found) = seen.get(dir) {
                if found {
                    break;
                }
                directory = dir.parent();
                continue;
            }
            let manifest = dir.join("package.json");
            let found = manifest.is_file();
            seen.insert(dir.to_path_buf(), found);
            if found {
                if let Some(package) = read_package(&manifest) {
                    packages.push(package);
                }
                break;
            }
            directory = dir.parent();
        }
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name).then(a.directory.cmp(&b.directory)));
    // Two packages with the same name: neither is used.
    let mut unique: Vec<WorkspacePackage> = Vec::new();
    let mut duplicated: Vec<String> = Vec::new();
    for package in packages {
        if unique.last().is_some_and(|last| last.name == package.name) {
            duplicated.push(package.name);
        } else {
            unique.push(package);
        }
    }
    unique.retain(|package| !duplicated.contains(&package.name));
    unique
}

fn read_package(manifest: &Path) -> Option<WorkspacePackage> {
    let json: Value = serde_json::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
    let name = json.get("name")?.as_str()?.to_owned();
    let entries = ["main", "module", "types"]
        .iter()
        .filter_map(|field| json.get(*field).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    Some(WorkspacePackage {
        name,
        directory: manifest.parent()?.to_path_buf(),
        exports: json.get("exports").filter(|v| !v.is_null()).cloned(),
        entries,
    })
}

/// The longest-prefix `paths` pattern matching `specifier`: the pattern, its
/// targets, and the text matched by `*`.
fn match_paths<'p, 's>(
    paths: &'p [(String, Vec<String>)],
    specifier: &'s str,
) -> Option<(&'p str, &'p [String], &'s str)> {
    let mut best: Option<(usize, &'p str, &'p [String], &'s str)> = None;
    for (pattern, targets) in paths {
        let matched = match pattern.split_once('*') {
            None => (pattern == specifier).then_some((pattern.len(), "")),
            Some((prefix, suffix)) => (specifier.len() >= prefix.len() + suffix.len()
                && specifier.starts_with(prefix)
                && specifier.ends_with(suffix))
            .then(|| {
                (
                    prefix.len(),
                    &specifier[prefix.len()..specifier.len() - suffix.len()],
                )
            }),
        };
        if let Some((length, star)) = matched
            && best.is_none_or(|(longest, ..)| length > longest)
        {
            best = Some((length, pattern, targets, star));
        }
    }
    best.map(|(_, pattern, targets, star)| (pattern, targets, star))
}

/// Read `compilerOptions.baseUrl` and `paths` from a tsconfig file and its
/// relative `extends` chain (at most 8 levels).
fn load_options(path: &Path, depth: usize) -> Option<PathOptions> {
    if depth > 8 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let json: Value = serde_json::from_str(&strip_jsonc(&text)).ok()?;
    let directory = path.parent()?.to_path_buf();
    let mut options = match json.get("extends") {
        Some(Value::String(base)) if base.starts_with('.') => {
            let mut base_path = normalize(&directory.join(base));
            if base_path.extension().is_none() {
                base_path.set_extension("json");
            }
            load_options(&base_path, depth + 1).unwrap_or_default()
        }
        _ => PathOptions::default(),
    };
    let compiler = json.get("compilerOptions");
    if let Some(Value::String(base_url)) = compiler.and_then(|c| c.get("baseUrl")) {
        options.base_url = Some(normalize(&directory.join(base_url)));
    }
    if let Some(Value::Object(paths)) = compiler.and_then(|c| c.get("paths")) {
        // Targets are relative to `baseUrl` when set, else to this file.
        let root = options
            .base_url
            .clone()
            .unwrap_or_else(|| directory.clone());
        options.paths = paths
            .iter()
            .filter(|(pattern, _)| pattern.matches('*').count() <= 1)
            .map(|(pattern, targets)| {
                let targets = targets
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|target| target.matches('*').count() <= 1)
                    .map(|target| normalize(&root.join(target)).to_string_lossy().into_owned())
                    .collect();
                (pattern.clone(), targets)
            })
            .collect();
    }
    Some(options)
}

/// Remove `//` and `/* */` comments and trailing commas from JSONC text.
pub(crate) fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas: a comma followed only by whitespace and `}` or `]`.
    let mut cleaned = String::with_capacity(out.len());
    let chars: Vec<char> = out.chars().collect();
    let mut in_string = false;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if in_string {
            cleaned.push(c);
            if c == '\\' && index + 1 < chars.len() {
                cleaned.push(chars[index + 1]);
                index += 1;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            cleaned.push(c);
        } else if c == ',' {
            let next = chars[index + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}' | ']')) {
                cleaned.push(c);
            }
        } else {
            cleaned.push(c);
        }
        index += 1;
    }
    cleaned
}

/// Lexically normalize a path: drop `.`, apply `..`.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The absolute, lexically normalized form of `path`.
pub(crate) fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    normalize(&joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas_are_removed() {
        let text = r#"{
          // comment
          "a": "http://x", /* block */
          "b": [1, 2,],
          "c": "// not a comment",
        }"#;
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value["a"], "http://x");
        assert_eq!(value["b"][1], 2);
        assert_eq!(value["c"], "// not a comment");
    }

    #[test]
    fn longest_paths_prefix_wins() {
        let paths = vec![
            ("@/*".to_owned(), vec!["/r/src/*".to_owned()]),
            ("@/db/*".to_owned(), vec!["/r/db/*".to_owned()]),
            ("exact".to_owned(), vec!["/r/exact.ts".to_owned()]),
        ];
        let (pattern, _, star) = match_paths(&paths, "@/db/client").unwrap();
        assert_eq!((pattern, star), ("@/db/*", "client"));
        let (pattern, _, star) = match_paths(&paths, "@/lib/x").unwrap();
        assert_eq!((pattern, star), ("@/*", "lib/x"));
        assert!(match_paths(&paths, "exact").is_some());
        assert!(match_paths(&paths, "other").is_none());
    }

    #[test]
    fn exports_maps_subpaths_patterns_and_conditions() {
        let exports: Value = serde_json::json!({
            "./client": "./src/client.ts",
            "./queries": { "types": "./dist/queries.d.ts", "import": "./src/queries/index.ts" },
            "./utils/*": "./src/utils/*.ts"
        });
        assert_eq!(export_targets(&exports, "./client"), ["./src/client.ts"]);
        assert_eq!(
            export_targets(&exports, "./queries"),
            ["./dist/queries.d.ts", "./src/queries/index.ts"]
        );
        assert_eq!(
            export_targets(&exports, "./utils/tax"),
            ["./src/utils/tax.ts"]
        );
        assert!(export_targets(&exports, "./missing").is_empty());
        assert_eq!(
            export_targets(&serde_json::json!("./src/index.ts"), "."),
            ["./src/index.ts"]
        );
    }

    #[test]
    fn normalization_applies_parent_components() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d.ts")),
            PathBuf::from("/a/c/d.ts")
        );
    }

    #[test]
    fn candidates_are_ambiguous_when_several_files_match() {
        let files = vec![
            PathBuf::from("/p/a.ts"),
            PathBuf::from("/p/a.tsx"),
            PathBuf::from("/p/b/index.ts"),
            PathBuf::from("/p/c.ts"),
        ];
        let names = vec!["a.ts", "a.tsx", "b/index.ts", "c.ts"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let resolver = Resolver::new(&files, names);
        assert!(matches!(
            resolver.candidates(Path::new("/p/a"), "./a"),
            Resolution::Ambiguous(_)
        ));
        assert_eq!(
            resolver.candidates(Path::new("/p/b"), "./b"),
            Resolution::File(2)
        );
        assert_eq!(
            resolver.candidates(Path::new("/p/c.js"), "./c.js"),
            Resolution::File(3)
        );
        assert!(matches!(
            resolver.candidates(Path::new("/p/missing"), "./missing"),
            Resolution::NotFound(_)
        ));
    }
}
