//! The review before a download (`docs/model-downloads.md`): metadata and licence fetched in
//! the background, admitted strictly, and kept as one immutable snapshot under a random token.
//! Confirming a download carries the token, so what is recorded is exactly what was shown.
//! At most one review is live at a time and its data is bounded.
use super::client::{Client, Error as ClientError};
use super::hub::{self, Facts, MAX_LICENCE_BYTES, MAX_METADATA_BYTES};
use crate::assistant::model::{HfSpec, Pinned, pinned};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

/// A ready review stays confirmable this long.
pub(crate) const LIFE: Duration = Duration::from_mins(30);

/// The licence as shown and recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Licence {
    /// The card's `license` identifier, when it states one.
    pub id: Option<String>,
    /// The licence file's repository-relative path at the pinned revision.
    pub path: Option<String>,
    /// An external link the card names (text only, never fetched).
    pub link: Option<String>,
    /// The licence file's text (lossy UTF-8; escaped on render).
    pub text: Option<String>,
    /// The SHA-256 of the licence file's bytes.
    pub sha256: Option<String>,
}

/// Everything a review showed. Immutable once built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub token: String,
    pub spec: HfSpec,
    pub facts: Facts,
    pub licence: Licence,
    /// Set when the spec is one of the two pinned default files (checked against the
    /// hash and size compiled into Mimik).
    pub pinned: Option<Pinned>,
}

#[derive(Clone, Debug)]
pub(crate) enum State {
    Idle,
    Running { spec: String },
    Ready(Arc<Snapshot>),
    Failed { spec: String, reason: String },
}

struct Inner {
    state: State,
    /// When a ready snapshot stops being confirmable.
    expires: Option<Instant>,
    /// Incremented per review, so a late finish of a replaced review is dropped.
    serial: u64,
}

/// The one review that may be live.
#[derive(Clone)]
pub(crate) struct Reviewer {
    client: Client,
    inner: Arc<Mutex<Inner>>,
    life: Duration,
}

/// A review is already running.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Busy;

fn token() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system provides randomness");
    hex::encode(bytes)
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Reviewer {
    pub(crate) fn new(client: Client) -> Self {
        Self::with_life(client, LIFE)
    }

    /// A reviewer whose ready snapshots stay confirmable for `life` (tests shorten it).
    pub(crate) fn with_life(client: Client, life: Duration) -> Self {
        Self {
            life,
            client,
            inner: Arc::new(Mutex::new(Inner {
                state: State::Idle,
                expires: None,
                serial: 0,
            })),
        }
    }

    /// The current state; a ready review past its life is idle.
    pub(crate) fn state(&self) -> State {
        let mut inner = lock(&self.inner);
        if inner.expires.is_some_and(|at| Instant::now() >= at) {
            inner.state = State::Idle;
            inner.expires = None;
        }
        inner.state.clone()
    }

    /// The ready snapshot `token` names, if it is the live one.
    pub(crate) fn snapshot(&self, token: &str) -> Option<Arc<Snapshot>> {
        match self.state() {
            State::Ready(snapshot) if snapshot.token == token => Some(snapshot),
            _ => None,
        }
    }

    /// Forgets the ready review (after it was confirmed).
    pub(crate) fn clear(&self) {
        let mut inner = lock(&self.inner);
        inner.state = State::Idle;
        inner.expires = None;
    }

    /// Starts the review of `spec` in the background.
    ///
    /// # Errors
    ///
    /// [`Busy`] while another review runs.
    pub(crate) fn start(&self, spec: HfSpec) -> Result<(), Busy> {
        let client = self.client.clone();
        let work_spec = spec.clone();
        self.start_with(spec, async move { review(&client, &work_spec).await })
    }

    /// [`Self::start`] over any work that produces the snapshot (a panic in it ends the
    /// review as failed, never leaves it running).
    pub(crate) fn start_with(
        &self,
        spec: HfSpec,
        work: impl std::future::Future<Output = Result<Snapshot, String>> + Send + 'static,
    ) -> Result<(), Busy> {
        let serial = {
            let mut inner = lock(&self.inner);
            if matches!(inner.state, State::Running { .. }) {
                return Err(Busy);
            }
            inner.serial += 1;
            inner.expires = None;
            inner.state = State::Running {
                spec: spec.canonical(),
            };
            inner.serial
        };
        let (inner, life) = (Arc::clone(&self.inner), self.life);
        let guard = Unfinished {
            inner: Arc::clone(&inner),
            serial,
            spec: spec.canonical(),
            armed: true,
        };
        tokio::spawn(async move {
            let outcome = work.await;
            guard.disarm();
            let mut inner = lock(&inner);
            if inner.serial != serial {
                return;
            }
            match outcome {
                Ok(snapshot) => {
                    inner.expires = Some(Instant::now() + life);
                    inner.state = State::Ready(Arc::new(snapshot));
                }
                Err(reason) => {
                    inner.state = State::Failed {
                        spec: spec.canonical(),
                        reason,
                    };
                }
            }
        });
        Ok(())
    }
}

/// Ends a review that never reached its outcome (the task panicked or was dropped) as failed.
struct Unfinished {
    inner: Arc<Mutex<Inner>>,
    serial: u64,
    spec: String,
    armed: bool,
}

impl Unfinished {
    /// The work reached its outcome: dropping does nothing.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut inner = lock(&self.inner);
        if inner.serial == self.serial && matches!(inner.state, State::Running { .. }) {
            inner.state = State::Failed {
                spec: std::mem::take(&mut self.spec),
                reason: "the review stopped unexpectedly: try again".to_owned(),
            };
        }
    }
}

fn client_reason(error: &ClientError) -> String {
    error.to_string()
}

/// Fetches and admits the metadata and the licence of `spec`.
async fn review(client: &Client, spec: &HfSpec) -> Result<Snapshot, String> {
    let body = client
        .api_json(client.metadata_url(spec), MAX_METADATA_BYTES)
        .await
        .map_err(|error| client_reason(&error))?;
    let facts = hub::admit(&body, spec).map_err(|refusal| refusal.0)?;
    let canonical = spec.canonical();
    let pin = pinned().into_iter().find(|pin| pin.hf == canonical);
    if let Some(pin) = pin
        && (facts.sha256 != pin.sha256 || facts.size != pin.size)
    {
        return Err("the file does not match the pinned file compiled into Mimik".to_owned());
    }
    let mut licence = Licence {
        id: facts.licence_id.clone(),
        path: facts.licence_file.clone(),
        link: facts.licence_link.clone(),
        text: None,
        sha256: None,
    };
    if let Some(path) = &facts.licence_file {
        let bytes = client
            .small_file(client.resolve_url(spec, path), MAX_LICENCE_BYTES)
            .await
            .map_err(|error| {
                format!("the licence file cannot be read: {}", client_reason(&error))
            })?;
        licence.sha256 = Some(hex::encode(Sha256::digest(&bytes)));
        licence.text = Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    Ok(Snapshot {
        token: token(),
        spec: spec.clone(),
        facts,
        licence,
        pinned: pin,
    })
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
