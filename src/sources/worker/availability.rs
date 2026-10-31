//! Lazy, process-wide real-worker probe. Upload callers share the same fail-closed result.
use super::{Inspection, Reason, launch};
use crate::sandbox::host;
use std::{path::Path, sync::OnceLock};

// Synthetic one-page PDF, no owner content or active resources. MuPDF repairs its xref.
pub(super) const PDF: &[u8] = b"%PDF-1.7\n1 0 obj <</Type /Catalog /Pages 2 0 R>> endobj\n2 0 obj <</Type /Pages /Count 1 /Kids [3 0 R]>> endobj\n3 0 obj <</Type /Page /Parent 2 0 R /MediaBox [0 0 100 200]>> endobj\ntrailer <</Root 1 0 R>>\n%%EOF\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable { reason: String, how_to_fix: String },
}

/// Runs once, lazily at the first upload operation; never tries an unconfined parser.
#[must_use]
pub fn availability() -> &'static Availability {
    static RESULT: OnceLock<Availability> = OnceLock::new();
    RESULT.get_or_init(|| {
        let result = launch::probe();
        map(
            result,
            host::namespace_cause(Path::new("/proc")),
            &std::env::current_exe().unwrap_or_default(),
        )
    })
}

fn map(result: Result<Inspection, Reason>, cause: Option<host::Cause>, exe: &Path) -> Availability {
    match result {
        Ok(report) if report.document.pages == 1 && report.pages.len() == 1 => Availability::Available,
        result => Availability::Unavailable {
            reason: match result {
                Err(Reason::Unavailable(reason)) => reason,
                _ => "PDF worker probe failed".into(),
            },
            how_to_fix: cause.map_or_else(
                || "Enable unprivileged user namespaces (user.max_user_namespaces must be greater than zero), then restart Mimik.".into(),
                |cause| host::remedy(cause, exe),
            ),
        },
    }
}

pub(super) fn require() -> Result<(), Reason> {
    match availability() {
        Availability::Available => Ok(()),
        Availability::Unavailable { reason, .. } => Err(Reason::Unavailable(reason.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_failure_retains_fixed_reason_and_shared_assistant_remedy() {
        for cause in [host::Cause::AppArmor, host::Cause::UsernsClone] {
            let exe = Path::new("/opt/mimik");
            assert_eq!(
                map(
                    Err(Reason::Unavailable("sandbox step unshare failed".into())),
                    Some(cause),
                    exe
                ),
                Availability::Unavailable {
                    reason: "sandbox step unshare failed".into(),
                    how_to_fix: host::remedy(cause, exe),
                }
            );
        }
        let mapped = map(
            Err(super::super::Error::Unreadable.into()),
            None,
            Path::new("/mimik"),
        );
        assert!(
            matches!(mapped, Availability::Unavailable { reason, how_to_fix } if reason == "PDF worker probe failed" && how_to_fix.contains("user.max_user_namespaces"))
        );
    }
}
