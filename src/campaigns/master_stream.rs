//! The Master event stream (CAMP-14, ROAD-8): `GET /campaigns/{id}/master/events`.
//!
//! The handshake (read-only admission, current-Master check, slot, subscription, first
//! projection) runs inside the request's 5 s boundary on ordinary request capacity and returns
//! a lazy body. From then on one producer task per stream owns the slot and does the work on
//! the separate stream-worker pool (`crate::streams`): a recheck of the session binding and the
//! Master role before every projection, every `RECHECK` while idle and at the session's
//! absolute expiry, with coalesced projections, a single-slot output queue and fail-closed
//! closing. The producer never waits on the body: it revalidates on its own clock.
use super::master::{MasterPage, read_workspace};
use super::{campaign, role};
use crate::{
    AppError, Store,
    auth::RequestAuth,
    hub::Feed,
    streams::{Outbox, Refused, Slot, Status, Streams, WorkError, raised, timing, with_retry},
};
use axum::{
    http::{HeaderValue, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use sha2::{Digest, Sha256};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};

/// Test-only: a handshake waits here, after taking its slot, until the gate has a permit.
#[cfg(test)]
pub(super) static AFTER_ADMIT: std::sync::Mutex<Option<Arc<tokio::sync::Semaphore>>> =
    std::sync::Mutex::new(None);

/// A successful projection: the snapshot JSON (`None` when a row or the body exceeded its
/// limit) and the authentication clock at its final recheck.
struct Projected {
    json: Option<String>,
    now: i64,
}

/// The current-Master check: the session binding, the campaign and the active Master role in
/// one read transaction, with no sheet metadata. It admits a handshake before anything is
/// subscribed, and it is the data-free recheck of an idle stream and of session expiry.
fn check_master(
    db: &mut rusqlite::Connection,
    auth: &RequestAuth,
    id: i64,
) -> Result<i64, AppError> {
    let tx = db.transaction()?;
    let now = auth.recheck(&tx)?;
    campaign(&tx, id)?;
    if role(&tx, id, auth.account_id())?.as_deref() != Some("master") {
        return Err(AppError::NotFound);
    }
    tx.commit()?;
    Ok(now)
}

/// The projection: both pages in one read transaction that rechecks at its start and final
/// boundary, then once more after the commit, immediately before the data can be admitted as
/// output.
fn project(
    db: &mut rusqlite::Connection,
    auth: &RequestAuth,
    id: i64,
    page: MasterPage,
) -> Result<Projected, AppError> {
    let workspace = read_workspace(db, auth, id, page)?;
    let json = workspace.snapshot_json();
    let now = auth.recheck(db)?;
    Ok(Projected { json, now })
}

fn snapshot_event(json: String) -> Event {
    Event::default().event("snapshot").data(json)
}

fn digest(json: &str) -> [u8; 32] {
    Sha256::digest(json.as_bytes()).into()
}

/// The stop signals of a stream; each is sticky.
struct Signals {
    shutdown: watch::Receiver<bool>,
    evicted: watch::Receiver<bool>,
    gone: watch::Receiver<bool>,
}

impl Signals {
    /// The notice (if any) a stopped stream leaves: shutdown and replacement are announced, a
    /// vanished reader needs none.
    async fn stopped(&mut self) -> Option<Status> {
        tokio::select! {
            biased;
            () = raised(&mut self.shutdown) => Some(Status::Closing),
            () = raised(&mut self.evicted) => Some(Status::Replaced),
            () = raised(&mut self.gone) => None,
        }
    }
}

/// Handshake: admits the request as a stream and returns the lazy SSE response.
pub(crate) async fn open(
    store: Store,
    auth: RequestAuth,
    id: i64,
    page: MasterPage,
) -> Result<Response, StatusCode> {
    let error = super::web::error;
    // Only a current Master may take a slot or a subscription (sheet hints reveal IDs).
    let admit = auth.clone();
    store
        .run_campaign(auth.clone(), move |db| check_master(db, &admit, id))
        .await
        .map_err(error)?;
    // Subscribe before the first read, so no commit in between is missed. The slot is taken
    // only after the read succeeds: a failed handshake must not evict the Master's older stream.
    let streams = store.streams();
    let feed = store.feed(id, auth.account_id());
    let first = auth.clone();
    let projected = store
        .run_campaign(auth.clone(), move |db| project(db, &first, id, page))
        .await
        .map_err(error)?;
    let json = projected.json.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let admission = streams
        .admit(id, auth.account_id())
        .map_err(|refused| match refused {
            Refused::Campaign => StatusCode::TOO_MANY_REQUESTS,
            Refused::Instance => StatusCode::SERVICE_UNAVAILABLE,
        })?;
    #[cfg(test)]
    {
        let gate = AFTER_ADMIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(gate) = gate {
            drop(gate.acquire().await);
        }
    }
    let last = digest(&json);
    let (outbox, reader) = Outbox::new([
        with_retry(Status::Live.event(Some(&admission.token))),
        snapshot_event(json),
    ]);
    let producer = Producer {
        store,
        streams: streams.clone(),
        auth,
        campaign: id,
        page,
        feed,
        signals: Signals {
            shutdown: streams.shutdown_signal(),
            evicted: admission.evicted,
            gone: outbox.gone(),
        },
        outbox,
        _slot: admission.slot,
        last,
    };
    tokio::spawn(producer.run(projected.now));
    let sse = Sse::new(reader.into_stream())
        .keep_alive(KeepAlive::new().interval(timing::HEARTBEAT).text(""));
    let mut response = sse.into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

struct Producer {
    store: Store,
    streams: Arc<Streams>,
    auth: RequestAuth,
    campaign: i64,
    page: MasterPage,
    feed: Feed,
    outbox: Outbox,
    signals: Signals,
    /// Held until the producer ends: the connection slot.
    _slot: Slot,
    /// Digest of the last snapshot queued, so an unchanged projection sends nothing.
    last: [u8; 32],
}

/// When the session's absolute expiry falls, from the authentication clock `now` (at least one
/// second ahead, so a coarse clock cannot spin the loop).
fn expiry(now: i64, deadline: i64) -> Instant {
    let seconds = u64::try_from(deadline.saturating_sub(now))
        .unwrap_or(0)
        .max(1);
    Instant::now() + Duration::from_secs(seconds)
}

impl Producer {
    /// One database operation on the stream-worker pool, interruptible by the stop signals.
    async fn guarded<T>(
        &mut self,
        operation: impl Future<Output = Result<T, WorkError>>,
    ) -> Result<Result<T, WorkError>, Option<Status>> {
        tokio::select! {
            biased;
            notice = self.signals.stopped() => Err(notice),
            result = operation => Ok(result),
        }
    }

    fn finish(self, notice: Option<Status>) {
        self.outbox.close(notice.map(|status| status.event(None)));
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one state machine, read top to bottom"
    )]
    async fn run(mut self, first_now: i64) {
        let born = Instant::now();
        let deadline = self.auth.deadline();
        let mut last_ok = born;
        let mut expires = expiry(first_now, deadline);
        let mut last_project = born;
        let mut last_reconcile = born;
        let mut retry_at = born;
        let mut dirty = false;
        let mut blocked_since: Option<Instant> = None;
        loop {
            let now = Instant::now();
            if now >= last_ok + timing::GRACE {
                return self.finish(None);
            }
            if now >= born + timing::MAX_AGE
                || blocked_since.is_some_and(|since| now >= since + timing::OUTPUT_BLOCK)
            {
                return self.finish(Some(Status::Closing));
            }
            if now >= last_reconcile + timing::RECONCILE {
                dirty = true;
                last_reconcile = now;
            }
            let project_at = (last_project + timing::REFRESH).max(retry_at);
            let recheck_at = expires.min(last_ok + timing::RECHECK).max(retry_at);
            let full = self.outbox.is_full();
            // Output is blocked while events wait for the body, whether or not a newer snapshot
            // is wanted: a reader that stops taking them is closed after OUTPUT_BLOCK (the
            // handshake's two seed events count as queued until taken). Any progress by the
            // body restarts the count.
            if full {
                blocked_since.get_or_insert(now);
            } else {
                blocked_since = None;
            }
            if dirty && now >= project_at && !full {
                {
                    last_project = now;
                    let (store, streams, auth) =
                        (self.store.clone(), self.streams.clone(), self.auth.clone());
                    let (id, page) = (self.campaign, self.page);
                    let operation = async move {
                        let admitted = auth.clone();
                        streams
                            .work(&store, auth, move |db| project(db, &admitted, id, page))
                            .await
                    };
                    match self.guarded(operation).await {
                        Err(notice) => return self.finish(notice),
                        Ok(Err(WorkError::Busy)) => retry_at = Instant::now() + timing::RETRY_DELAY,
                        Ok(Err(WorkError::Failed(AppError::NotFound))) => return self.finish(None),
                        Ok(Err(WorkError::Failed(_))) => {
                            return self.finish(Some(Status::Error));
                        }
                        Ok(Ok(projected)) => {
                            last_ok = Instant::now();
                            expires = expiry(projected.now, deadline);
                            let Some(json) = projected.json else {
                                return self.finish(Some(Status::Error));
                            };
                            dirty = false;
                            let changed = digest(&json);
                            if changed != self.last {
                                self.last = changed;
                                self.outbox.push(snapshot_event(json));
                            }
                        }
                    }
                    continue;
                }
            }
            if now >= recheck_at {
                let (store, streams, auth) =
                    (self.store.clone(), self.streams.clone(), self.auth.clone());
                let id = self.campaign;
                let operation = async move {
                    let admitted = auth.clone();
                    streams
                        .work(&store, auth, move |db| check_master(db, &admitted, id))
                        .await
                };
                match self.guarded(operation).await {
                    Err(notice) => return self.finish(notice),
                    Ok(Err(WorkError::Busy)) => retry_at = Instant::now() + timing::RETRY_DELAY,
                    Ok(Err(WorkError::Failed(AppError::NotFound))) => return self.finish(None),
                    Ok(Err(WorkError::Failed(_))) => return self.finish(Some(Status::Error)),
                    Ok(Ok(seconds)) => {
                        last_ok = Instant::now();
                        expires = expiry(seconds, deadline);
                    }
                }
                continue;
            }
            // Sleep until the next due time, or until a hint, a free output slot or a stop.
            let mut due = (last_ok + timing::GRACE)
                .min(born + timing::MAX_AGE)
                .min(last_reconcile + timing::RECONCILE)
                .min(recheck_at);
            if let Some(since) = blocked_since {
                due = due.min(since + timing::OUTPUT_BLOCK);
            } else if dirty {
                due = due.min(project_at);
            }
            let waiting_for_space = blocked_since.is_some();
            tokio::select! {
                biased;
                notice = self.signals.stopped() => return self.finish(notice),
                () = tokio::time::sleep_until(due) => {}
                () = self.outbox.space(), if waiting_for_space => blocked_since = None,
                hint = self.feed.recv() => match hint {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => dirty = true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return self.finish(Some(Status::Closing));
                    }
                },
            }
        }
    }
}
