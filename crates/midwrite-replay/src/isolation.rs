//! OS-specific isolation of a harness process.
//!
//! Isolation reduces the chance that an *authorized, cooperative* harness
//! touches anything but its disposable database and replay directory. It is
//! not a sandbox for hostile code (see `docs/architecture/replay-protocol.md` and
//! `docs/reference/security.md`).
//!
//! Backends:
//!
//! * **macOS**: `sandbox-exec` with a profile that denies IP traffic, Unix
//!   socket connections outside the replay root, and file writes outside
//!   the replay root.
//! * **Linux**: bubblewrap (`bwrap`) with new user, network, PID, IPC, UTS
//!   and cgroup namespaces and a minimal filesystem view: read-only system
//!   directories, the harness project and the runtime's installation; the
//!   replay root as the only writable host directory; a private `/tmp`, a
//!   fresh `/proc` and a minimal `/dev`. The network namespace has only a
//!   loopback interface, so no TCP/UDP endpoint outside the sandbox is
//!   reachable; other local sockets (`/tmp/.s.PGSQL.5432`,
//!   `/run/postgresql`, `/var/run/docker.sock`, the user's home directory)
//!   are not mounted. The backend is probed before use; when it cannot
//!   create its namespaces the replay is blocked (fail closed).
//! * **None**: only with an explicit opt-in (`--allow-network` and the
//!   manifest's `"network": "allow"`), recorded in the report.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What the isolation needs to know about one replay invocation.
#[derive(Clone, Debug)]
pub struct IsolationRequest<'a> {
    /// The private replay root: the only writable host directory.
    pub root: &'a Path,
    /// The harness directory (canonical).
    pub harness_dir: &'a Path,
    /// `command[0]` of the manifest.
    pub program: &'a str,
    /// The user explicitly allowed running without isolation.
    pub allow_unisolated: bool,
}

/// A prepared isolation backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Isolation {
    SandboxExec {
        profile: String,
    },
    Bubblewrap {
        bwrap: PathBuf,
        /// Arguments before `--` and the command.
        args: Vec<String>,
        /// The program, resolved to an absolute path visible in the sandbox.
        program: PathBuf,
        /// Host directories visible read-only, for the report.
        read_only: Vec<PathBuf>,
    },
    None,
}

impl Isolation {
    /// Short machine-readable name of the backend.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SandboxExec { .. } => "sandbox-exec",
            Self::Bubblewrap { .. } => "bubblewrap",
            Self::None => "none",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::SandboxExec { .. } => "sandbox-exec: IP networking denied; Unix sockets and file writes limited to the replay directory".to_owned(),
            Self::Bubblewrap { read_only, .. } => format!(
                "bubblewrap: new user, network (loopback only), PID, IPC, UTS and cgroup namespaces; writable: the replay directory only; read-only: {}; private /tmp, fresh /proc, minimal /dev",
                read_only
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::None => "none: the harness may use the network (explicitly allowed)".to_owned(),
        }
    }

    /// The command line that runs `command` under this isolation.
    pub fn wrap(&self, command: &[String]) -> Vec<String> {
        match self {
            Self::SandboxExec { profile } => {
                let mut wrapped = vec![
                    "/usr/bin/sandbox-exec".to_owned(),
                    "-p".to_owned(),
                    profile.clone(),
                ];
                wrapped.extend(command.iter().cloned());
                wrapped
            }
            Self::Bubblewrap {
                bwrap,
                args,
                program,
                ..
            } => {
                let mut wrapped = vec![bwrap.display().to_string()];
                wrapped.extend(args.iter().cloned());
                wrapped.push("--".to_owned());
                wrapped.push(program.display().to_string());
                wrapped.extend(command.iter().skip(1).cloned());
                wrapped
            }
            Self::None => command.to_vec(),
        }
    }
}

/// Choose and prepare the isolation for a harness: the platform backend, or
/// an error that blocks the replay. Running without isolation requires the
/// explicit opt-in.
pub fn prepare(request: &IsolationRequest<'_>) -> Result<Isolation, String> {
    if request.allow_unisolated {
        return Ok(Isolation::None);
    }
    if cfg!(target_os = "macos") {
        let sandbox = Path::new("/usr/bin/sandbox-exec");
        if sandbox.is_file() {
            return Ok(Isolation::SandboxExec {
                profile: sandbox_profile(request.root),
            });
        }
        return Err(blocked("/usr/bin/sandbox-exec is missing"));
    }
    if cfg!(target_os = "linux") {
        return prepare_bubblewrap(request);
    }
    Err(blocked(&format!(
        "no isolation backend is implemented for {}",
        std::env::consts::OS
    )))
}

fn blocked(reason: &str) -> String {
    format!(
        "{reason}. Midwrite refuses to run a harness without OS isolation (macOS: sandbox-exec; Linux: bubblewrap with unprivileged user namespaces). Re-run with --allow-network (and the manifest's \"network\": \"allow\") only for a harness you trust not to reach external services; that disables all isolation."
    )
}

pub fn sandbox_profile(root: &Path) -> String {
    let root = root
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        concat!(
            "(version 1)",
            "(allow default)",
            "(deny network-outbound (remote ip))",
            "(deny network-inbound (local ip))",
            "(deny network-outbound (remote unix-socket))",
            "(allow network-outbound (remote unix-socket (subpath \"{root}\")))",
            "(deny file-write*)",
            "(allow file-write* (subpath \"{root}\") (regex #\"^/dev/\"))"
        ),
        root = root
    )
}

/// System directories mounted read-only when they exist. They hold
/// programs, libraries and configuration, not service sockets.
const SYSTEM_DIRS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/etc",
    "/sys",
    "/nix/store",
];

/// Find the bubblewrap executable.
/// The distribution's `/usr/bin/bwrap` first, so that a `bwrap` earlier on
/// `PATH` cannot replace the isolation; else an absolute `PATH` entry.
fn find_bwrap() -> Option<PathBuf> {
    let fixed = Path::new("/usr/bin/bwrap");
    if fixed.is_file() {
        return Some(fixed.to_path_buf());
    }
    crate::postgres::find_on_path("bwrap")
}

/// Resolve `program` like the supervisor would start it: a path relative to
/// the harness directory, an absolute path, or a name on `PATH`.
pub fn resolve_program(program: &str, harness_dir: &Path) -> Option<PathBuf> {
    let candidate = if program.contains('/') {
        let path = Path::new(program);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            harness_dir.join(path)
        }
    } else {
        crate::postgres::find_on_path(program)?
    };
    candidate.canonicalize().ok().filter(|p| p.is_file())
}

/// The installation directory of a program: `<prefix>` for
/// `<prefix>/bin/<program>`, else the program's directory.
pub fn installation_dir(program: &Path) -> Option<PathBuf> {
    let parent = program.parent()?;
    let prefix = if parent.file_name().is_some_and(|n| n == "bin") {
        parent.parent()?
    } else {
        parent
    };
    Some(prefix.to_path_buf())
}

/// The directory a harness belongs to: the enclosing Git work tree (so that
/// relative imports and `file:` dependencies inside the project resolve),
/// else the harness directory itself.
pub fn project_root(harness_dir: &Path) -> PathBuf {
    let mut current = Some(harness_dir);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }
    harness_dir.to_path_buf()
}

fn covered_by(path: &Path, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| path.starts_with(dir))
}

/// Whether mounting `dir` would expose the user's whole home directory (or
/// more): `/`, the home directory itself, or one of its ancestors.
pub fn exposes_home(dir: &Path, home: Option<&Path>) -> bool {
    dir == Path::new("/") || home.is_some_and(|home| home.starts_with(dir))
}

/// The read-only directory that makes `program` runnable: its installation
/// directory, narrowed to the program's own directory, then to the program
/// file, when the wider one would expose the home directory
/// (`~/bin/node` must not mount all of `~`).
pub fn runtime_mount(program: &Path, home: Option<&Path>) -> Option<PathBuf> {
    [
        installation_dir(program),
        program.parent().map(Path::to_path_buf),
    ]
    .into_iter()
    .flatten()
    .find(|dir| !exposes_home(dir, home))
    .or_else(|| Some(program.to_path_buf()))
}

/// The project directory mounted read-only: the enclosing Git work tree,
/// or the harness directory itself when the work tree would expose the
/// home directory (a dotfiles repository in `~`).
pub fn project_mount(harness_dir: &Path, home: Option<&Path>) -> PathBuf {
    let root = project_root(harness_dir);
    if exposes_home(&root, home) {
        harness_dir.to_path_buf()
    } else {
        root
    }
}

/// The user's home directory, canonicalized, for [`exposes_home`].
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.canonicalize().unwrap_or(p))
}

/// The bubblewrap arguments (without `--` and the command).
pub fn bubblewrap_args(
    root: &Path,
    harness_dir: &Path,
    extra_read_only: &[PathBuf],
    home: Option<&Path>,
) -> (Vec<String>, Vec<PathBuf>) {
    let mut args: Vec<String> = [
        "--die-with-parent",
        "--unshare-user",
        "--unshare-net",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup-try",
        "--hostname",
        "midwrite-replay",
    ]
    .map(str::to_owned)
    .to_vec();
    let mut read_only: Vec<PathBuf> = Vec::new();
    for dir in SYSTEM_DIRS {
        let path = Path::new(dir);
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                // Merged /usr: keep `/bin -> usr/bin` a symlink.
                if let Ok(target) = std::fs::read_link(path) {
                    args.extend([
                        "--symlink".to_owned(),
                        target.display().to_string(),
                        dir.to_string(),
                    ]);
                }
            }
            Ok(meta) if meta.is_dir() => {
                args.extend(["--ro-bind".to_owned(), dir.to_string(), dir.to_string()]);
                read_only.push(path.to_path_buf());
            }
            _ => {}
        }
    }
    // Fresh pseudo-filesystems first: binds below may live under /tmp.
    args.extend(
        ["--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]
            .iter()
            .map(|s| (*s).to_owned()),
    );
    // The runtime's installation and the project, unless already visible.
    let mut extra: Vec<PathBuf> = extra_read_only.to_vec();
    extra.push(project_mount(harness_dir, home));
    extra.sort();
    extra.dedup();
    for dir in extra {
        if dir == Path::new("/") || covered_by(&dir, &read_only) || dir.starts_with(root) {
            continue;
        }
        let text = dir.display().to_string();
        args.extend(["--ro-bind".to_owned(), text.clone(), text]);
        read_only.push(dir);
    }
    let root_text = root.display().to_string();
    args.extend(["--bind".to_owned(), root_text.clone(), root_text]);
    args.extend(["--chdir".to_owned(), harness_dir.display().to_string()]);
    (args, read_only)
}

fn prepare_bubblewrap(request: &IsolationRequest<'_>) -> Result<Isolation, String> {
    let Some(bwrap) = find_bwrap() else {
        return Err(blocked(
            "bubblewrap (`bwrap`) is not installed; install it (Debian/Ubuntu: `apt-get install bubblewrap`)",
        ));
    };
    // An unresolvable program is passed on unchanged: the run then fails as
    // "could not be started", exactly as without isolation.
    let resolved = resolve_program(request.program, request.harness_dir);
    let home = home_dir();
    let extra: Vec<PathBuf> = resolved
        .as_deref()
        .and_then(|program| runtime_mount(program, home.as_deref()))
        .into_iter()
        .collect();
    let program = resolved.unwrap_or_else(|| PathBuf::from(request.program));
    let (args, read_only) =
        bubblewrap_args(request.root, request.harness_dir, &extra, home.as_deref());
    probe(&bwrap, &args)?;
    Ok(Isolation::Bubblewrap {
        bwrap,
        args,
        program,
        read_only,
    })
}

/// Run `true` with exactly the prepared arguments. Fails when the kernel or
/// a security policy does not allow the namespaces.
fn probe(bwrap: &Path, args: &[String]) -> Result<(), String> {
    let mut child = Command::new(bwrap)
        .args(args)
        .arg("--")
        .arg("true")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| blocked(&format!("bubblewrap could not be started: {e}")))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(blocked("the bubblewrap probe did not finish"));
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    let message = stderr.lines().last().unwrap_or("").trim().to_owned();
    Err(blocked(&format!(
        "bubblewrap cannot create its namespaces here ({message}). Unprivileged user namespaces may be disabled (sysctl kernel.unprivileged_userns_clone, user.max_user_namespaces), restricted by AppArmor (Ubuntu >= 23.10: kernel.apparmor_restrict_unprivileged_userns), or blocked by a container's seccomp profile or masked /proc"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_profile_confines_sockets_and_writes_to_the_root() {
        let profile = sandbox_profile(Path::new("/private/tmp/cr-x"));
        assert!(profile.contains("(deny network-outbound (remote ip))"));
        assert!(profile.contains(
            "(allow network-outbound (remote unix-socket (subpath \"/private/tmp/cr-x\")))"
        ));
        assert!(profile.contains("(deny file-write*)"));
    }

    #[test]
    fn bubblewrap_arguments_unshare_the_network_and_expose_only_the_root_writable() {
        let root = Path::new("/tmp/cr-test-root");
        let harness = Path::new("/home/dev/project/harness");
        let (args, read_only) = bubblewrap_args(
            root,
            harness,
            &[PathBuf::from("/opt/node-v22")],
            Some(Path::new("/home/dev")),
        );
        let joined = args.join(" ");
        for flag in [
            "--unshare-user",
            "--unshare-net",
            "--unshare-pid",
            "--die-with-parent",
            "--proc /proc",
            "--dev /dev",
            "--tmpfs /tmp",
        ] {
            assert!(joined.contains(flag), "{flag} missing: {joined}");
        }
        // The only writable bind is the replay root.
        let writable: Vec<&[String]> = args.windows(3).filter(|w| w[0] == "--bind").collect();
        assert_eq!(writable.len(), 1);
        assert_eq!(writable[0][1], "/tmp/cr-test-root");
        // No root bind, no home directory, no /run or /var.
        assert!(!args.windows(2).any(|w| w[0] == "--ro-bind" && w[1] == "/"));
        for hidden in ["/run", "/var", "/home", "/root"] {
            assert!(
                !read_only.iter().any(|p| p == Path::new(hidden)),
                "{hidden} must not be mounted"
            );
        }
        // The runtime installation and the project are visible read-only.
        assert!(read_only.contains(&PathBuf::from("/opt/node-v22")));
        assert!(read_only.contains(&PathBuf::from("/home/dev/project/harness")));
        // The private /tmp is mounted before the binds that may lie below it.
        let tmpfs = args.iter().position(|a| a == "--tmpfs").unwrap();
        let bind = args.iter().position(|a| a == "--bind").unwrap();
        let project = args
            .iter()
            .position(|a| a == "/home/dev/project/harness")
            .unwrap();
        assert!(tmpfs < bind && tmpfs < project);
    }

    #[test]
    fn mounts_never_expose_the_home_directory() {
        let home = Some(Path::new("/home/dev"));
        // `~/bin/node`: the installation prefix would be `~` itself.
        assert_eq!(
            runtime_mount(Path::new("/home/dev/bin/node"), home),
            Some(PathBuf::from("/home/dev/bin"))
        );
        // A program directly in `~`: only the file.
        assert_eq!(
            runtime_mount(Path::new("/home/dev/node"), home),
            Some(PathBuf::from("/home/dev/node"))
        );
        // A normal installation keeps its prefix.
        assert_eq!(
            runtime_mount(Path::new("/home/dev/.nvm/versions/node/v22/bin/node"), home),
            Some(PathBuf::from("/home/dev/.nvm/versions/node/v22"))
        );
        assert!(exposes_home(Path::new("/"), home));
        assert!(exposes_home(Path::new("/home"), home));
        assert!(exposes_home(Path::new("/home/dev"), home));
        assert!(!exposes_home(Path::new("/home/dev/project"), home));

        // A dotfiles repository in `~`: the Git work tree would be `~`.
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cr-home-{}", std::process::id()));
        let harness = base.join("project/harness");
        std::fs::create_dir_all(&harness).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();
        assert_eq!(project_mount(&harness, Some(&base)), harness);
        std::fs::create_dir_all(base.join("project/.git")).unwrap();
        assert_eq!(project_mount(&harness, Some(&base)), base.join("project"));
        let (args, read_only) = bubblewrap_args(Path::new("/tmp/cr-r"), &harness, &[], Some(&base));
        assert!(!read_only.contains(&base), "{args:?}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn installation_dirs_and_project_roots() {
        assert_eq!(
            installation_dir(Path::new("/opt/node/bin/node")),
            Some(PathBuf::from("/opt/node"))
        );
        assert_eq!(
            installation_dir(Path::new("/opt/bun/bun")),
            Some(PathBuf::from("/opt/bun"))
        );
        let dir = std::env::temp_dir().join(format!("cr-project-{}", std::process::id()));
        let nested = dir.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        assert_eq!(project_root(&nested), dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn running_without_isolation_needs_the_explicit_opt_in() {
        let request = IsolationRequest {
            root: Path::new("/tmp/x"),
            harness_dir: Path::new("/tmp"),
            program: "definitely-not-a-program",
            allow_unisolated: true,
        };
        assert_eq!(prepare(&request).unwrap(), Isolation::None);
        if cfg!(target_os = "linux") {
            let strict = IsolationRequest {
                allow_unisolated: false,
                ..request
            };
            // Bubblewrap with a usable probe, or a refusal: never an
            // unisolated success.
            assert!(!matches!(prepare(&strict), Ok(Isolation::None)));
        }
    }
}
