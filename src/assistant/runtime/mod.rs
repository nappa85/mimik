//! The model runtime (assisted sheet plan §1.4a; step 2c-2a): the pinned official
//! llama-server started as Mimik's child confined by bubblewrap (level 1, ASSIST-43),
//! reached only through a checked endpoint with a per-launch token, primed and calibrated
//! before it takes jobs.
//!
//! Linux `x86_64` only; the CPU path at level 1 (bubblewrap); level 3 (unconfined) is built
//! but not used (ASSIST-43), and returns later behind an owner opt-in. There is no level 2
//! (ASSIST-48); the level numbers stay 1 and 3.
pub(crate) mod budgets;
pub(crate) mod bwrap;
pub(crate) mod calibration;
pub(crate) mod device;
pub(crate) mod drain;
pub(crate) mod endpoint;
pub(crate) mod files;
pub(crate) mod launch;
pub(crate) mod manager;
pub(crate) mod manifest;
pub(crate) mod probe;
pub(crate) mod rundir;
pub(crate) mod runner;
pub(crate) mod seccomp;
pub(crate) mod server;
#[cfg(test)]
pub(crate) mod test_helper;
pub(crate) mod threads;
pub(crate) mod token;

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
    /// seccomp only, on `127.0.0.1`.
    Unconfined,
}

impl Level {
    /// The number shown beside the level.
    pub(crate) fn number(self) -> u8 {
        match self {
            Self::Bubblewrap => 1,
            Self::Unconfined => 3,
        }
    }

    /// Whether the child listens on TCP (`127.0.0.1`) rather than a Unix socket.
    pub(crate) fn tcp(self) -> bool {
        self != Self::Bubblewrap
    }
}
