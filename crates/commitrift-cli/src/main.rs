//! `commitrift` command-line interface.

use std::process::ExitCode;

fn main() -> ExitCode {
    commitrift_cli::run(commitrift_cli::Invocation::Commitrift)
}
