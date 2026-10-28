//! What the Downloads panel of `/admin/assistant` shows (step 2d-2), built from the download
//! state; plain strings for the template, which escapes them.
use super::assistant_downloads::signature;
use crate::assistant::AssistantHandle;
use crate::assistant::download::disk::Partial;
use crate::assistant::download::review::State as ReviewState;
use crate::assistant::download::rows::{Row, State as RowState};
use crate::assistant::download::task::{Phase, Stop};
use crate::assistant::model::pinned;
use std::sync::Arc;

pub(super) struct ReadyView {
    pub token: String,
    pub repo: String,
    pub file: String,
    pub revision: String,
    pub size: String,
    pub sha256: String,
    pub licence_id: String,
    pub licence_path: String,
    pub licence_link: String,
    pub licence_text: String,
    pub licence_sha256: String,
    pub no_licence: bool,
    /// What the hash was checked against.
    pub trust: &'static str,
}

pub(super) struct ReviewView {
    pub state: &'static str,
    pub spec: String,
    pub reason: String,
    pub ready: Option<ReadyView>,
}

pub(super) struct TransferView {
    pub id: i64,
    pub spec: String,
    pub state: &'static str,
    pub detail: String,
    pub received: u64,
    pub total: u64,
    pub running: bool,
    pub can_resume: bool,
}

pub(super) struct FileView {
    pub id: i64,
    pub spec: String,
    pub file_name: String,
    pub size: String,
    pub licence_id: String,
    pub present: bool,
    pub deletable: bool,
}

pub(super) struct PinnedView {
    pub hf: &'static str,
    pub file_name: &'static str,
    pub size: String,
    pub status: &'static str,
    pub can_review: bool,
}

pub(super) struct DownloadsView {
    /// Why downloads are not available, when they are not.
    pub unavailable: Option<String>,
    pub review: Option<ReviewView>,
    pub transfers: Vec<TransferView>,
    pub files: Vec<FileView>,
    pub pinned: Vec<PinnedView>,
    /// The state the polling script compares to decide whether to reload.
    pub signature: String,
    pub polling: bool,
}

fn mib(bytes: u64) -> String {
    format!("{} MiB", bytes.div_ceil(1 << 20))
}

pub(super) fn unavailable(note: Option<&str>) -> DownloadsView {
    DownloadsView {
        unavailable: Some(
            note.unwrap_or("downloads are not available with this assistant")
                .to_owned(),
        ),
        review: None,
        transfers: Vec::new(),
        files: Vec::new(),
        pinned: Vec::new(),
        signature: String::new(),
        polling: false,
    }
}

/// The panel for `handle`; `active` names the files of the active and previous model.
pub(super) async fn view(handle: &AssistantHandle, active: &[String]) -> DownloadsView {
    let Some(downloads) = handle.downloads.clone() else {
        return unavailable(handle.downloads_note.as_deref());
    };
    let review_state = downloads.reviewer.state();
    let status = downloads.downloader.status();
    let rows = downloads.rows.list().await.unwrap_or_default();
    // The disk, off the executor: the sizes of the partials and of the final files.
    let probe = {
        let (disk, rows, names) = (
            Arc::clone(&downloads.disk),
            rows.clone(),
            pinned().map(|pin| pin.file_name()),
        );
        tokio::task::spawn_blocking(move || {
            let partials: Vec<Option<Partial>> = rows
                .iter()
                .map(|row| disk.partial_state(&row.sha256).ok().flatten())
                .collect();
            let finals: Vec<Option<Partial>> = rows
                .iter()
                .map(|row| disk.final_state(&row.file_name).ok().flatten())
                .collect();
            let pinned_files: Vec<Result<Option<Partial>, ()>> = names
                .iter()
                .map(|name| disk.final_state(name).map_err(|_| ()))
                .collect();
            (partials, finals, pinned_files)
        })
        .await
        .unwrap_or_default()
    };
    let (partials, finals, pinned_files) = probe;
    let review = review_view(&review_state);
    let mut transfers = Vec::new();
    let mut files = Vec::new();
    let protect: Vec<String> = pinned()
        .iter()
        .map(|pin| pin.file_name().to_owned())
        .chain(active.iter().cloned())
        .collect();
    for (index, row) in rows.iter().enumerate() {
        match row.state {
            RowState::Confirmed => {
                transfers.push(transfer_view(row, &status, partials[index]));
            }
            RowState::Complete => files.push(FileView {
                id: row.id,
                spec: row.spec.clone(),
                file_name: row.file_name.clone(),
                size: mib(row.size),
                licence_id: row.licence_id.clone().unwrap_or_default(),
                present: finals[index].is_some_and(|file| file.len == row.size),
                deletable: !protect.contains(&row.file_name),
            }),
        }
    }
    let pinned_view = pinned_views(&rows, pinned_files);
    let polling = matches!(review_state, ReviewState::Running { .. })
        || matches!(status.phase, Phase::Running { .. } | Phase::Verifying);
    DownloadsView {
        unavailable: None,
        review,
        transfers,
        files,
        pinned: pinned_view,
        signature: signature(&review_state, &status),
        polling,
    }
}

/// The pinned defaults with what is known of each in the models folder.
fn pinned_views(rows: &[Row], found: Vec<Result<Option<Partial>, ()>>) -> Vec<PinnedView> {
    pinned()
        .iter()
        .zip(found)
        .map(|(pin, found)| {
            let known = rows
                .iter()
                .any(|row| row.spec == pin.hf && row.state == RowState::Complete);
            let (status, can_review) = match found {
                Ok(Some(file)) if file.len == pin.size => {
                    if known {
                        ("downloaded and verified by Mimik", false)
                    } else {
                        (
                            "present in the models folder (checked when it loads)",
                            false,
                        )
                    }
                }
                Ok(Some(_)) | Err(()) => (
                    "a different file has this name: remove it by hand to download the pinned one",
                    false,
                ),
                Ok(None) => ("not downloaded", !rows.iter().any(|row| row.spec == pin.hf)),
            };
            PinnedView {
                hf: pin.hf,
                file_name: pin.file_name(),
                size: mib(pin.size),
                status,
                can_review,
            }
        })
        .collect()
}

fn review_view(state: &ReviewState) -> Option<ReviewView> {
    match state {
        ReviewState::Idle => None,
        ReviewState::Running { spec } => Some(ReviewView {
            state: "running",
            spec: spec.clone(),
            reason: String::new(),
            ready: None,
        }),
        ReviewState::Failed { spec, reason } => Some(ReviewView {
            state: "failed",
            spec: spec.clone(),
            reason: reason.clone(),
            ready: None,
        }),
        ReviewState::Ready(snapshot) => Some(ReviewView {
            state: "ready",
            spec: snapshot.spec.canonical(),
            reason: String::new(),
            ready: Some(ReadyView {
                token: snapshot.token.clone(),
                repo: format!("{}/{}", snapshot.spec.org, snapshot.spec.repo),
                file: snapshot.spec.path.clone(),
                revision: snapshot.spec.revision.clone(),
                size: format!(
                    "{} bytes ({})",
                    snapshot.facts.size,
                    mib(snapshot.facts.size)
                ),
                sha256: snapshot.facts.sha256.clone(),
                licence_id: snapshot.licence.id.clone().unwrap_or_default(),
                licence_path: snapshot.licence.path.clone().unwrap_or_default(),
                licence_link: snapshot.licence.link.clone().unwrap_or_default(),
                licence_text: snapshot.licence.text.clone().unwrap_or_default(),
                licence_sha256: snapshot.licence.sha256.clone().unwrap_or_default(),
                no_licence: snapshot.licence.text.is_none(),
                trust: if snapshot.pinned.is_some() {
                    "checked against the SHA-256 and size compiled into Mimik"
                } else {
                    "third-party model: its hash comes from the same host as the file"
                },
            }),
        }),
    }
}

fn transfer_view(
    row: &Row,
    status: &crate::assistant::download::task::Status,
    partial: Option<Partial>,
) -> TransferView {
    let current = status.id == Some(row.id);
    let (state, detail, received, running) = match (&status.phase, current) {
        (Phase::Running { received, .. }, true) => ("downloading", String::new(), *received, true),
        (Phase::Verifying, true) => (
            "verifying",
            "checking the whole file's SHA-256".to_owned(),
            row.size,
            true,
        ),
        (Phase::Stopped(Stop::Cancelled), true) => (
            "cancelled",
            "the partial file is kept; Resume continues it".to_owned(),
            partial.map_or(0, |p| p.len),
            false,
        ),
        (Phase::Stopped(Stop::Stalled), true) => (
            "stalled",
            "the server was too slow; Resume continues it".to_owned(),
            partial.map_or(0, |p| p.len),
            false,
        ),
        (Phase::Stopped(Stop::Failed(why)), true) => {
            ("failed", why.clone(), partial.map_or(0, |p| p.len), false)
        }
        _ => (
            "waiting",
            "waiting for Resume: nothing downloads without you".to_owned(),
            partial.map_or(0, |p| p.len),
            false,
        ),
    };
    TransferView {
        id: row.id,
        spec: row.spec.clone(),
        state,
        detail,
        received,
        total: row.size,
        running,
        can_resume: !running,
    }
}
