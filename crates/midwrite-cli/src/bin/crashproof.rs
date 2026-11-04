//! `crashproof`: the pre-rename name of the `midwrite` executable, kept so
//! that existing automation keeps working. Output and exit statuses are the
//! same as `midwrite`'s; a deprecation note goes to standard error.

use std::process::ExitCode;

fn main() -> ExitCode {
    midwrite_cli::run(midwrite_cli::Invocation::LegacyCrashproof)
}
