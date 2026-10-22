//! The circuit breaker (assisted sheet plan §1.4, "Circuit breaker"): one per endpoint
//! configuration, in memory, driven by an injectable clock.
//!
//! It counts only transport errors and invalid replies while the child is alive. Three
//! consecutive counted failures open it for two minutes; then one half-open probe job
//! decides. A completed, bounded 2xx exchange is a success whatever its content.
//! Supersession, cancellation and deadline expiries are neutral. A counted failure is
//! provisional while its generation is alive: the generation's exit within two seconds
//! withdraws it, and a breaker that only withdrawn counts opened closes again.
use std::time::{Duration, Instant};

/// How long an opened breaker refuses before a probe.
pub(crate) const OPEN_FOR: Duration = Duration::from_secs(120);
/// Consecutive counted failures that open it.
pub(crate) const THRESHOLD: usize = 3;
/// A child exit this soon after a counted failure of its generation withdraws the count.
pub(crate) const WITHDRAW_WITHIN: Duration = Duration::from_secs(2);

/// A job's identity as the breaker knows it.
pub(crate) type Ticket = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Count {
    generation: u64,
    at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Closed,
    Open {
        until: Instant,
    },
    /// Waiting for a probe; `probe` is the job that is it, while in flight.
    HalfOpen {
        probe: Option<Ticket>,
    },
}

/// What a start or a run may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Closed: go.
    Go,
    /// Half-open: go as the probe.
    Probe,
    /// Open, or a probe is in flight: "the assistant is unavailable right now".
    Refused,
}

/// How an exchange ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A completed, bounded 2xx exchange, whatever its content.
    Success,
    /// A transport error or an invalid reply, in this generation.
    Failure { generation: u64 },
    /// Superseded, cancelled, expired in the queue or past its run deadline, or ended by a
    /// generation change: never counted.
    Neutral,
}

/// What opened the breaker, so a withdrawal can undo exactly that.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Opening {
    /// The third consecutive count: these counts.
    Threshold(Vec<Count>),
    /// A failed probe: its count.
    Probe(Count),
}

#[derive(Debug)]
pub(crate) struct Breaker {
    state: State,
    counts: Vec<Count>,
    /// What opened it, while it is open or half-open.
    opening: Option<Opening>,
}

impl Default for Breaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Breaker {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Closed,
            counts: Vec::new(),
            opening: None,
        }
    }

    /// Whether it refuses everything right now (open and not yet due for a probe, or a
    /// probe in flight).
    pub(crate) fn refuses(&self, now: Instant) -> bool {
        match self.state {
            State::Closed => false,
            State::Open { until } => now < until,
            State::HalfOpen { probe } => probe.is_some(),
        }
    }

    /// Admits `ticket` at `now`: closed goes; open past its time or half-open without a
    /// probe makes `ticket` the probe; otherwise refused. `superseded` is the account's job
    /// this start replaces: admission is computed as if it were already gone, so if it is
    /// the probe, the newcomer takes its place.
    pub(crate) fn admit(
        &mut self,
        ticket: Ticket,
        now: Instant,
        superseded: Option<Ticket>,
    ) -> Admission {
        match self.state {
            State::Closed => Admission::Go,
            State::Open { until } if now < until => Admission::Refused,
            State::HalfOpen { probe: Some(probe) } if probe == ticket => Admission::Probe,
            State::HalfOpen { probe: Some(probe) } if Some(probe) != superseded => {
                Admission::Refused
            }
            State::Open { .. } | State::HalfOpen { .. } => {
                self.state = State::HalfOpen {
                    probe: Some(ticket),
                };
                Admission::Probe
            }
        }
    }

    /// Whether `ticket` may run now, at dequeue: closed, or it is the probe. A job queued
    /// while closed finds an opened breaker here and ends "unavailable".
    pub(crate) fn may_run(&self, ticket: Ticket) -> bool {
        match self.state {
            State::Closed => true,
            State::HalfOpen { probe } => probe == Some(ticket),
            State::Open { .. } => false,
        }
    }

    /// Whether `ticket` is the probe in flight.
    pub(crate) fn is_probe(&self, ticket: Ticket) -> bool {
        self.state
            == State::HalfOpen {
                probe: Some(ticket),
            }
    }

    /// Records how `ticket`'s exchange ended at `now`.
    pub(crate) fn record(&mut self, ticket: Ticket, outcome: Outcome, now: Instant) {
        let probe = matches!(self.state, State::HalfOpen { probe: Some(p) } if p == ticket);
        match outcome {
            Outcome::Success => {
                self.counts.clear();
                if matches!(self.state, State::HalfOpen { .. }) && probe {
                    self.state = State::Closed;
                    self.opening = None;
                }
            }
            Outcome::Failure { generation } => {
                let count = Count {
                    generation,
                    at: now,
                };
                if probe {
                    self.opening = Some(Opening::Probe(count));
                    self.state = State::Open {
                        until: now + OPEN_FOR,
                    };
                } else if self.state == State::Closed {
                    self.counts.push(count);
                    if self.counts.len() >= THRESHOLD {
                        self.opening = Some(Opening::Threshold(std::mem::take(&mut self.counts)));
                        self.state = State::Open {
                            until: now + OPEN_FOR,
                        };
                    }
                }
            }
            Outcome::Neutral => {
                if probe {
                    self.state = State::HalfOpen { probe: None };
                }
            }
        }
    }

    /// The child of `generation` exited at `now`: its counted failures of the last
    /// [`WITHDRAW_WITHIN`] are withdrawn. A breaker that a withdrawn count opened goes back
    /// to where it was: closed with the counts that remain, or half-open after a probe.
    pub(crate) fn generation_ended(&mut self, generation: u64, now: Instant) {
        let withdrawn = |count: &Count| {
            count.generation == generation
                && now.saturating_duration_since(count.at) <= WITHDRAW_WITHIN
        };
        self.counts.retain(|count| !withdrawn(count));
        match self.opening.take() {
            Some(Opening::Threshold(counts)) if counts.iter().any(withdrawn) => {
                self.counts = counts
                    .into_iter()
                    .filter(|count| !withdrawn(count))
                    .collect();
                self.state = State::Closed;
            }
            Some(Opening::Probe(count)) if withdrawn(&count) => {
                self.state = State::HalfOpen { probe: None };
            }
            other => self.opening = other,
        }
    }

    /// The owner's Restart or the app's start.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.state == State::Closed
    }

    #[cfg(test)]
    pub(crate) fn counted(&self) -> usize {
        self.counts.len()
    }
}

#[cfg(test)]
#[path = "breaker_tests.rs"]
mod tests;
