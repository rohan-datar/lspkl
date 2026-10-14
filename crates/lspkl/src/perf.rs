//! Opt-in Phase 0 timings; all durations are monotonic microseconds.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tracing::Span;

static NEXT_HANDLER_ID: AtomicU64 = AtomicU64::new(1);
static ORDERING_VIOLATIONS: AtomicU64 = AtomicU64::new(0);
static CLOCK: LazyLock<Instant> = LazyLock::new(Instant::now);

fn timestamp_us(at: Instant) -> u64 {
    at.saturating_duration_since(*CLOCK).as_micros() as u64
}

pub(super) fn ordering_violation() -> u64 {
    ORDERING_VIOLATIONS.fetch_add(1, Ordering::Relaxed) + 1
}

pub(super) struct Handler(Option<(Span, Instant)>);

impl Handler {
    pub(super) fn enter() -> Self {
        if !tracing::enabled!(target: "lspkl::perf", tracing::Level::DEBUG) {
            return Self(None);
        }
        LazyLock::force(&CLOCK);
        let start = Instant::now();
        let span = Span::current();
        span.record(
            "handler_id",
            NEXT_HANDLER_ID.fetch_add(1, Ordering::Relaxed),
        );
        tracing::debug!(target: "lspkl::perf", parent: &span,
            event = "handler_entry", timestamp_us = timestamp_us(start),
            ordering_violations = ORDERING_VIOLATIONS.load(Ordering::Relaxed));
        Self(Some((span, start)))
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        if let Some((span, start)) = &self.0 {
            tracing::debug!(target: "lspkl::perf", parent: span,
                timestamp_us = timestamp_us(Instant::now()),
                event = "handler_end", handler_us = start.elapsed().as_micros() as u64,
                ordering_violations = ORDERING_VIOLATIONS.load(Ordering::Relaxed));
        }
    }
}

pub(super) struct Compute {
    timing: Option<(Span, Instant)>,
    stage: &'static str,
    execution: &'static str,
}

pub(super) fn cancelled(stage: &'static str) {
    tracing::debug!(target: "lspkl::perf", event = "request_cancelled", stage);
}

impl Compute {
    pub(super) fn runtime(stage: &'static str) -> Self {
        Self::begin(stage, "runtime", None)
    }

    pub(super) fn queued(stage: &'static str, queued: Instant) -> Self {
        Self::begin(stage, "blocking", Some(queued))
    }

    pub(super) fn start(stage: &'static str) -> Self {
        Self::begin(stage, "blocking", None)
    }

    fn begin(stage: &'static str, execution: &'static str, queued: Option<Instant>) -> Self {
        let timing = if tracing::enabled!(target: "lspkl::perf", tracing::Level::DEBUG) {
            let span = Span::current();
            let start = Instant::now();
            tracing::debug!(target: "lspkl::perf", parent: &span,
                event = "compute_start", stage, execution,
                timestamp_us = timestamp_us(start),
                worker_queue_us = queued.map(|queued| start.duration_since(queued).as_micros() as u64));
            Some((span, start))
        } else {
            None
        };
        Self {
            timing,
            stage,
            execution,
        }
    }
}

impl Drop for Compute {
    fn drop(&mut self) {
        if let Some((span, start)) = &self.timing {
            tracing::debug!(target: "lspkl::perf", parent: span,
                event = "compute_end", stage = self.stage, execution = self.execution,
                timestamp_us = timestamp_us(Instant::now()),
                compute_us = start.elapsed().as_micros() as u64);
        }
    }
}
