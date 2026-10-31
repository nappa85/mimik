//! Lazy, process-wide real-worker probe. Upload callers share the same fail-closed result.
use super::{Inspection, Reason, launch};
use crate::sandbox::host;
use std::{
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

// Synthetic one-page PDF, no owner content or active resources. MuPDF repairs its xref.
pub(super) const PDF: &[u8] = b"%PDF-1.7\n1 0 obj <</Type /Catalog /Pages 2 0 R>> endobj\n2 0 obj <</Type /Pages /Count 1 /Kids [3 0 R]>> endobj\n3 0 obj <</Type /Page /Parent 2 0 R /MediaBox [0 0 100 200]>> endobj\ntrailer <</Root 1 0 R>>\n%%EOF\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable { reason: String, how_to_fix: String },
}

/// Lazy confined probe. Setup failures and success persist; other failures retry with
/// exponential backoff (one to thirty seconds), shared across concurrent callers.
#[must_use]
pub fn availability() -> Availability {
    static RESULT: OnceLock<Mutex<Cache>> = OnceLock::new();
    RESULT
        .get_or_init(|| Mutex::new(Cache::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(Instant::now(), launch::probe)
}

#[derive(Default)]
struct Cache {
    result: Option<Availability>,
    retry: Option<Instant>,
    failures: u32,
}

impl Cache {
    fn get(
        &mut self,
        now: Instant,
        probe: impl FnOnce() -> Result<Inspection, Reason>,
    ) -> Availability {
        if let Some(result) = &self.result
            && self.retry.is_none_or(|retry| now < retry)
        {
            return result.clone();
        }
        let result = probe();
        let permanent = matches!(&result, Ok(report) if report.document.pages == 1 && report.pages.len() == 1)
            || matches!(&result, Err(Reason::Unavailable(reason)) if reason.starts_with("sandbox step ") || reason.starts_with("unprivileged user namespaces are disabled"));
        let mapped = map(
            result,
            host::namespace_cause(Path::new("/proc")),
            &std::env::current_exe().unwrap_or_default(),
        );
        self.failures = self.failures.saturating_add(1);
        self.retry = (!permanent).then(|| {
            now + Duration::from_secs((1_u64 << self.failures.saturating_sub(1).min(5)).min(30))
        });
        self.result = Some(mapped.clone());
        mapped
    }
}

fn map(result: Result<Inspection, Reason>, cause: Option<host::Cause>, exe: &Path) -> Availability {
    match result {
        Ok(report) if report.document.pages == 1 && report.pages.len() == 1 => {
            Availability::Available
        }
        result => Availability::Unavailable {
            reason: match result {
                Err(Reason::Unavailable(reason)) => reason,
                _ => "PDF worker probe failed".into(),
            },
            how_to_fix: remedy(Path::new("/proc"), cause, exe),
        },
    }
}

fn remedy(proc: &Path, cause: Option<host::Cause>, exe: &Path) -> String {
    if host::namespaces(proc).is_err() {
        return "An administrator must enable unprivileged user namespaces (user.max_user_namespaces must be greater than zero), then restart Mimik.".into();
    }
    cause.map_or_else(
        || "Check the PDF worker sandbox configuration and system resources, then retry the upload.".into(),
        |cause| host::remedy(cause, exe).replace("private sandbox of the model server", "private sandbox of the PDF worker").replace("then press Restart", "then restart Mimik and retry the upload"),
    )
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
                    how_to_fix: remedy(Path::new("/proc"), Some(cause), exe),
                }
            );
        }
        let mapped = map(
            Err(super::super::Error::Unreadable.into()),
            None,
            Path::new("/mimik"),
        );
        assert!(
            matches!(mapped, Availability::Unavailable { reason, how_to_fix } if reason == "PDF worker probe failed" && !how_to_fix.contains("user.max_user_namespaces") && !how_to_fix.contains("press Restart"))
        );
    }

    #[test]
    fn transient_probe_failures_retry_with_bounded_backoff_but_setup_failures_persist() {
        let now = Instant::now();
        let mut cache = Cache::default();
        cache.get(now, || Err(super::super::Error::Unreadable.into()));
        cache.get(now, || panic!("backoff must prevent a probe"));
        assert_eq!(cache.retry, Some(now + Duration::from_secs(1)));
        for second in 1..=40 {
            cache.get(now + Duration::from_secs(second * 31), || {
                Err(Reason::Unavailable("probe file".into()))
            });
            assert!(cache.retry.unwrap() <= now + Duration::from_secs(second * 31 + 30));
        }
        cache.get(now + Duration::from_secs(2000), || {
            Err(Reason::Unavailable("sandbox step unshare failed".into()))
        });
        cache.get(now + Duration::from_secs(4000), || {
            panic!("setup failure must persist")
        });
        assert_eq!(cache.retry, None);
    }

    #[test]
    fn max_namespaces_remedy_requires_observed_zero() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            !remedy(root.path(), None, Path::new("/mimik")).contains("user.max_user_namespaces")
        );
        std::fs::create_dir_all(root.path().join("sys/user")).unwrap();
        std::fs::write(root.path().join("sys/user/max_user_namespaces"), "0").unwrap();
        assert!(
            remedy(root.path(), None, Path::new("/mimik")).contains("user.max_user_namespaces")
        );
    }
}
