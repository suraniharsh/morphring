//! Error types shared by every engine, plus the stable keys the CLI and UI show.

use std::fmt;

use thiserror::Error;

/// Stable identifier of a failure, shown by the CLI and UI next to a human detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKey {
    /// The source file's type is not known.
    UnsupportedSource,
    /// No chain of engines can produce the target, even with every engine installed.
    NoRoute,
    /// A route exists, but one of its engines is not installed.
    EngineMissing,
    /// An engine ran and failed.
    ConversionFailed,
}

impl ErrorKey {
    /// The stable string form, e.g. `error.no_route`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedSource => "error.unsupported_source",
            Self::NoRoute => "error.no_route",
            Self::EngineMissing => "error.engine_missing",
            Self::ConversionFailed => "error.conversion_failed",
        }
    }
}

impl fmt::Display for ErrorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a single conversion step failed.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The cancel token fired; partial output has been removed.
    #[error("the conversion was cancelled")]
    Cancelled,
    /// The engine's program is not installed.
    #[error("{tool} is not installed: {hint}")]
    NotInstalled { tool: &'static str, hint: String },
    /// The engine ran but produced no usable output.
    #[error("{0}")]
    Failed(String),
    /// Reading or writing a file failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl EngineError {
    /// The stable key a file that failed with this error is reported under.
    pub fn key(&self) -> ErrorKey {
        match self {
            Self::NotInstalled { .. } => ErrorKey::EngineMissing,
            Self::Cancelled | Self::Failed(_) | Self::Io(_) => ErrorKey::ConversionFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable() {
        assert_eq!(
            ErrorKey::UnsupportedSource.as_str(),
            "error.unsupported_source"
        );
        assert_eq!(ErrorKey::NoRoute.as_str(), "error.no_route");
        assert_eq!(ErrorKey::EngineMissing.as_str(), "error.engine_missing");
        assert_eq!(
            ErrorKey::ConversionFailed.as_str(),
            "error.conversion_failed"
        );
        assert_eq!(ErrorKey::NoRoute.to_string(), "error.no_route");
    }

    #[test]
    fn engine_errors_map_to_keys() {
        let missing = EngineError::NotInstalled {
            tool: "ffmpeg",
            hint: "pacman -S ffmpeg".into(),
        };
        assert_eq!(missing.key(), ErrorKey::EngineMissing);
        assert_eq!(
            EngineError::Failed("boom".into()).key(),
            ErrorKey::ConversionFailed
        );
    }
}
