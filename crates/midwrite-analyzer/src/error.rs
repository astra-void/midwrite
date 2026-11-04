use std::path::PathBuf;

/// Errors that prevent a scan from starting.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("path `{}` does not exist", .0.display())]
    NotFound(PathBuf),
    #[error("`{}` is not a TypeScript source file (.ts, .tsx, .mts, .cts)", .0.display())]
    Unsupported(PathBuf),
    #[error("failed to read directory `{}`: {source}", path.display())]
    Walk {
        path: PathBuf,
        #[source]
        source: walkdir::Error,
    },
}

/// A file that could not be parsed. It is reported, never analyzed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ParseFailure {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}
