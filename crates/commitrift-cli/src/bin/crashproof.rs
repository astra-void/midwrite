//! `crashproof`: the pre-rename name of the `commitrift` executable, kept so
//! that existing automation keeps working. Output and exit statuses are the
//! same as `commitrift`'s; a deprecation note goes to standard error.

use std::process::ExitCode;

fn main() -> ExitCode {
    commitrift_cli::run(commitrift_cli::Invocation::LegacyCrashproof)
}
