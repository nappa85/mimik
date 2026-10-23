//! The model runtime (assisted sheet plan §1.4a; step 2c-2a): the pinned official
//! llama-server started as Mimik's child confined by bubblewrap (level 1, ASSIST-43),
//! reached only through a checked endpoint with a per-launch token, primed and calibrated
//! before it takes jobs.
//!
//! Linux `x86_64` only; the CPU path at level 1 (bubblewrap); level 3 (unconfined) is built
//! but not used (ASSIST-43), and returns later behind an owner opt-in. The GPU path
//! and level 2 (`Landlock`) are step 2c-2b; the machinery here already carries the path and
//! the level.
pub(crate) mod budgets;
pub(crate) mod bwrap;
pub(crate) mod calibration;
pub(crate) mod device;
pub(crate) mod drain;
pub(crate) mod endpoint;
pub(crate) mod files;
pub(crate) mod hako;
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

/// Which sandbox builds level 1: bubblewrap, or (the Hakoniwa comparison spike, DEP-24, only
/// in builds with the `sandbox-hakoniwa` feature) Hakoniwa.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Backend {
    #[default]
    Bwrap,
    Hakoniwa,
}

impl Backend {
    /// The name `--assistant-sandbox` takes.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Bwrap => "bubblewrap",
            Self::Hakoniwa => "hakoniwa",
        }
    }

    /// `--assistant-sandbox`'s value.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text {
            "bwrap" | "bubblewrap" => Ok(Self::Bwrap),
            #[cfg(feature = "sandbox-hakoniwa")]
            "hakoniwa" => Ok(Self::Hakoniwa),
            #[cfg(not(feature = "sandbox-hakoniwa"))]
            "hakoniwa" => Err("this build has no Hakoniwa sandbox".into()),
            other => Err(format!(
                "--assistant-sandbox takes bwrap or hakoniwa, not {other}"
            )),
        }
    }
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
