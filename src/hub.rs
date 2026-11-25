//! The in-process notification hub of the Master view (CAMP-14): small typed invalidation hints,
//! published after a commit, on one bounded `tokio::sync::broadcast` ring owned by the [`Store`]
//! (shared across its clones, separate between stores). A hint says that something changed; it
//! carries no snapshot, note, session or page content, is never ordered or durable, and a
//! consumer always reads the authoritative state from SQL. Publication never blocks: a slow
//! subscriber loses its oldest hints (`RecvError::Lagged`) and must rebuild from a fresh read.
use tokio::sync::broadcast;

/// Capacity of the ring.
pub(crate) const RING: usize = 256;

/// One invalidation hint (IDs and revisions only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hint {
    /// Sheet `sheet` of `campaign` was committed at `revision` (a save, a freeze, a creation).
    Sheet {
        campaign: i64,
        sheet: i64,
        revision: i64,
    },
    /// The membership or role of `account` in `campaign` changed (join, promotion, demotion,
    /// departure, removal).
    Role { campaign: i64, account: i64 },
    /// The seen mark of Master `recipient` for `sheet` of `campaign` changed. Only that Master
    /// may be told.
    Seen {
        campaign: i64,
        recipient: i64,
        sheet: i64,
    },
}

impl Hint {
    /// Whether `account`, watching `campaign`, may see this hint: it must concern the campaign,
    /// and a seen hint only its recipient.
    pub(crate) fn visible_to(&self, campaign: i64, account: i64) -> bool {
        match *self {
            Self::Sheet { campaign: c, .. } | Self::Role { campaign: c, .. } => c == campaign,
            Self::Seen {
                campaign: c,
                recipient,
                ..
            } => c == campaign && recipient == account,
        }
    }
}

/// The publishing side; cloning shares the ring.
#[derive(Clone)]
pub(crate) struct Hub(broadcast::Sender<Hint>);

impl Default for Hub {
    fn default() -> Self {
        Self(broadcast::channel(RING).0)
    }
}

impl Hub {
    /// Publishes `hints` in order. Having no subscriber is not an error.
    pub(crate) fn publish(&self, hints: impl IntoIterator<Item = Hint>) {
        for hint in hints {
            let _ = self.0.send(hint);
        }
    }

    /// A receiver of every hint published from now on. Subscribe before reading a snapshot, so
    /// no commit between the read and the first `recv` is missed.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Hint> {
        self.0.subscribe()
    }
}

/// A subscription filtered by campaign and, for seen hints, by recipient: only what
/// [`Hint::visible_to`] allows. It is **not** an authorization: the caller must already have
/// admitted `account` as a current Master of `campaign` (sheet hints reveal sheet IDs and
/// revisions, NPC sheets included). The global ring is never handed on.
pub(crate) struct Feed {
    receiver: broadcast::Receiver<Hint>,
    campaign: i64,
    account: i64,
}

impl Feed {
    pub(crate) fn new(receiver: broadcast::Receiver<Hint>, campaign: i64, account: i64) -> Self {
        Self {
            receiver,
            campaign,
            account,
        }
    }

    /// The next visible hint; `Lagged` and `Closed` pass through.
    ///
    /// # Errors
    /// `Lagged(n)` when the subscriber fell more than the ring behind, `Closed` when every
    /// sender is gone.
    pub(crate) async fn recv(&mut self) -> Result<Hint, broadcast::error::RecvError> {
        loop {
            let hint = self.receiver.recv().await?;
            if hint.visible_to(self.campaign, self.account) {
                return Ok(hint);
            }
        }
    }
}
