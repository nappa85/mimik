//! The model runtime (assisted sheet plan §1.4a; step 2c-2a): the pinned official
//! llama-server started as Mimik's child under the highest confinement level that works,
//! reached only through a checked endpoint with a per-launch token, primed and calibrated
//! before it takes jobs.
//!
//! Linux x86_64 only; the CPU path at levels 1 (bubblewrap) and 3 (unconfined). The GPU path
//! and level 2 (Landlock) are step 2c-2b; the machinery here already carries the path and
//! the level.
pub(crate) mod budgets;
pub(crate) mod endpoint;
pub(crate) mod files;
pub(crate) mod manifest;
pub(crate) mod seccomp;
#[cfg(test)]
pub(crate) mod test_helper;

/// Which build runs, and so which budgets' caps apply (ASSIST-27, ASSIST-31).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Path {
    Cpu,
    Gpu,
}

/// The confinement levels (ASSIST-34), tried in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Level {
    /// bubblewrap: private namespaces, an allowlist of read-only paths, a Unix socket.
    Bubblewrap,
    /// Landlock with seccomp on `127.0.0.1` (step 2c-2b).
    Landlock,
    /// seccomp only, on `127.0.0.1`.
    Unconfined,
}

impl Level {
    /// The number shown beside the level.
    pub(crate) fn number(self) -> u8 {
        match self {
            Self::Bubblewrap => 1,
            Self::Landlock => 2,
            Self::Unconfined => 3,
        }
    }

    /// Whether the child listens on TCP (`127.0.0.1`) rather than a Unix socket.
    pub(crate) fn tcp(self) -> bool {
        self != Self::Bubblewrap
    }
}
