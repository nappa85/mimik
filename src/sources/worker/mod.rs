//! Confined PDF worker operations and untrusted-output validation.
mod availability;
mod launch;
pub use availability::{Availability, availability};
#[cfg(test)]
mod live_tests;
pub use launch::{Cancel, Reason, inspect, inspect_cancellable, render};
mod entry;
mod filter;
mod geometry;
mod operations;
mod profile;
pub mod records;
pub use entry::{RenderSettings, Spec, run_entry};
pub use geometry::pixel_size;
use operations::Input;
pub use records::{Inspection, Page, RenderedPage};

/// Fixed, non-sensitive reasons shared with the parent.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    #[error("could not be read")]
    Unreadable,
    #[error("took too long")]
    Timeout,
    #[error("too large")]
    TooLarge,
    #[error("too many pages")]
    TooManyPages,
    #[error("password-protected")]
    PasswordProtected,
    #[error("unsupported")]
    Unsupported,
}

impl Error {
    /// Stable process status for errors before a terminal frame can be written.
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Unreadable => 20,
            Self::Timeout => 21,
            Self::TooLarge => 22,
            Self::TooManyPages => 23,
            Self::PasswordProtected => 24,
            Self::Unsupported => 25,
        }
    }
    /// Decode a worker status when stdout framing could not start.
    #[must_use]
    pub const fn from_exit_code(code: i32) -> Option<Self> {
        match code {
            // Only the parent's deadline can establish a timeout.
            20 | 21 => Some(Self::Unreadable),
            22 => Some(Self::TooLarge),
            23 => Some(Self::TooManyPages),
            24 => Some(Self::PasswordProtected),
            25 => Some(Self::Unsupported),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
