//! Internal PDF worker operations and untrusted-output validation. No sandbox launch here.
mod entry;
mod geometry;
mod operations;
pub mod records;
pub use entry::{RenderSettings, Spec, run_entry};
pub use geometry::pixel_size;
pub use operations::{Input, inspect, render};
pub use records::{Inspection, Page, RenderedPage};

/// Fixed, non-sensitive reasons shared with the parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
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

#[cfg(test)]
mod tests;
