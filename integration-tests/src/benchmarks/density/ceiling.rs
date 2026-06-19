// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Ceiling-detection state machine (golemcloud/golem#3519, Module 3).
//!
//! Consumes a stream of timestamped latency samples plus executor-state
//! observations and emits ceiling-crossing events. Used by all density
//! sections to locate the soft / hard / catastrophic ceilings on a per-pod
//! basis.
//!
//! For v1 (cloud-perf showed a single driver node never became the bottleneck
//! before the platform did) the coordinator/driver gRPC protocol and the ramp
//! controller are not implemented. The agent-density driver feeds this state
//! machine in-process. The machine is shaped so that the union-of-drivers
//! aggregate the spec describes is a drop-in: feed it the merged sample stream
//! instead of one driver's.
//!
//! State and transitions follow the spec exactly:
//!
//! - `BaselineCollecting` → `Measuring` after [`BASELINE_SAMPLE_COUNT`]
//!   post-warmup samples. `baseline_p50` is the median of those samples.
//! - Soft crossed: rolling p99 over the last [`ROLLING_WINDOW`] samples exceeds
//!   [`SOFT_CEILING_MULTIPLIER`]× baseline. Emits [`CeilingEvent::SoftCrossed`]
//!   once; flag set; state stays `Measuring`.
//! - Hard crossed: a single sample exceeds [`HARD_CEILING_THRESHOLD`]. Emits
//!   [`CeilingEvent::HardCrossed`] once and raises an
//!   escalate-timeout-to-5-minutes signal; flag set; state stays `Measuring`.
//! - Catastrophic: any of (5-minute timeout fires; pod-restart count increased;
//!   connection lost; a sustained run of overloaded (503) responses; schedule-only
//!   queue-depth has not decreased for 60 consecutive seconds). Transitions to
//!   `Catastrophic`; emits [`CeilingEvent::Catastrophic`].

use std::collections::VecDeque;
use std::time::Duration;

/// Number of post-warmup samples collected before the baseline is fixed.
pub const BASELINE_SAMPLE_COUNT: usize = 100;

/// Size of the rolling window over which the soft-ceiling p99 is computed.
pub const ROLLING_WINDOW: usize = 10;

/// Rolling p99 must exceed this multiple of the baseline to cross the soft
/// ceiling.
pub const SOFT_CEILING_MULTIPLIER: u32 = 5;

/// Absolute rolling-p99 latency above which a trivial invocation is considered
/// too slow for a customer to tolerate — the usability ceiling.
///
/// Unlike the soft ceiling (a multiple of the empty-pod baseline), this is an
/// absolute SLO: a customer waiting on a simple counter call leaves once
/// responses routinely take this long, regardless of how fast the idle pod
/// was. Sits between the soft ceiling and the 30s hard ceiling.
pub const USABILITY_CEILING_P99: Duration = Duration::from_secs(1);

/// A single sample exceeding this duration crosses the hard ceiling.
pub const HARD_CEILING_THRESHOLD: Duration = Duration::from_secs(30);

/// Timeout the coordinator escalates to once the hard ceiling is crossed; if a
/// sample then exceeds this, the catastrophic ceiling is crossed.
pub const ESCALATED_TIMEOUT: Duration = Duration::from_secs(300);

/// Schedule-density only: how long the queue depth may stay non-decreasing
/// before the queue-depth-no-drain catastrophic condition fires.
pub const QUEUE_NO_DRAIN_TIMEOUT: Duration = Duration::from_secs(60);

/// How many consecutive overloaded (HTTP 503) responses constitute the platform
/// shedding load rather than an isolated blip. The executor returns 503 when it
/// cannot admit more work; a handful is tolerable, but a sustained run of them
/// means the platform can no longer serve the offered load — the overloaded
/// catastrophic condition.
pub const OVERLOAD_RUN_LENGTH: u32 = 200;

/// Why a cell stopped. The integer encoding is part of the result schema
/// (golemcloud/golem#3516) and must not be reordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminatedReason {
    OomKill = 1,
    PodRestart = 2,
    ConnectionLost = 3,
    UpperBoundHit = 4,
    /// Schedule-density only.
    LagRunaway = 5,
    /// The platform shed load: a sustained run of HTTP 503 responses.
    Overloaded = 6,
}

impl TerminatedReason {
    pub fn code(self) -> u64 {
        self as u64
    }

    /// Whether the cell stopped because the platform broke (as opposed to
    /// reaching its ramp upper bound healthily).
    pub fn is_catastrophic(self) -> bool {
        !matches!(self, TerminatedReason::UpperBoundHit)
    }
}

/// The ramp axis coordinate at which a crossing happened. `Agents` for
/// agent-density, `RatePerSec` for schedule/promise density.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SampleCoord {
    Agents(u32),
    RatePerSec(f64),
}

/// Snapshot of cross-axis executor state captured at a crossing, so the result
/// file is self-contained for interpreting the soft/catastrophic envelope.
///
/// Fields are optional because not every observation is available in every
/// section (e.g. queue depth is schedule-density only). The driver fills in
/// whatever it sampled most recently from the executor `/metrics` scrape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrossAxisSnapshot {
    pub worker_memory_pool_total_bytes: Option<u64>,
    pub worker_memory_pool_used_bytes: Option<u64>,
    pub active_workers_running: Option<u64>,
    pub active_workers_unloaded: Option<u64>,
    pub active_workers_waiting_for_permit: Option<u64>,
    pub active_workers_stopping: Option<u64>,
    pub scheduler_queue_depth: Option<u64>,
}

/// One observation fed into the state machine.
#[derive(Debug, Clone)]
pub struct Sample {
    /// Wall-clock latency of one attempt.
    pub latency: Duration,
    /// Ramp axis coordinate this sample was taken at.
    pub coord: SampleCoord,
    /// Executor pod restart count observed when the sample was taken.
    pub pod_restart_count: u64,
    /// Whether the connection to the executor is still alive.
    pub connection_alive: bool,
    /// Whether this attempt was rejected with an overloaded (HTTP 503) response.
    pub overloaded: bool,
    /// Most recent cross-axis snapshot from the metrics scrape.
    pub snapshot: CrossAxisSnapshot,
    /// Schedule-density only: scheduler queue depth observed for this sample.
    /// `None` for sections that do not track queue depth.
    pub queue_depth: Option<u64>,
}

/// Events the state machine emits as it consumes samples.
#[derive(Debug, Clone, PartialEq)]
pub enum CeilingEvent {
    /// The hard-ceiling client timeout should be escalated to
    /// [`ESCALATED_TIMEOUT`]. Raised once, alongside [`Self::HardCrossed`].
    EscalateTimeout,
    SoftCrossed {
        at: SampleCoord,
        snapshot: CrossAxisSnapshot,
    },
    /// Rolling p99 exceeded the absolute usability SLO
    /// ([`USABILITY_CEILING_P99`]). Emitted once; informational.
    UsabilityCrossed {
        at: SampleCoord,
        snapshot: CrossAxisSnapshot,
    },
    HardCrossed {
        at: SampleCoord,
    },
    Catastrophic {
        at: SampleCoord,
        reason: TerminatedReason,
        snapshot: CrossAxisSnapshot,
    },
}

#[derive(Debug)]
enum CeilingState {
    BaselineCollecting {
        samples: VecDeque<Duration>,
    },
    Measuring {
        baseline_p50: Duration,
        soft_crossed: bool,
        usability_crossed: bool,
        hard_crossed: bool,
    },
    Catastrophic {
        #[allow(dead_code)]
        reason: TerminatedReason,
    },
}

/// Tracks how long the scheduler queue depth has been non-decreasing, firing
/// the catastrophic no-drain condition after [`QUEUE_NO_DRAIN_TIMEOUT`].
///
/// Time is supplied by the caller (monotonic seconds since cell start) so the
/// machine stays deterministic and testable without a real clock.
#[derive(Debug, Default)]
struct QueueDrainTracker {
    last_depth: Option<u64>,
    non_decreasing_since_secs: Option<f64>,
}

impl QueueDrainTracker {
    /// Returns true if the queue has not drained for [`QUEUE_NO_DRAIN_TIMEOUT`].
    ///
    /// The no-drain timer anchors at the first observation of a non-decreasing
    /// run: as long as each observed depth is >= the previous one, the queue is
    /// considered "not draining". Any decrease resets the anchor.
    fn observe(&mut self, depth: u64, now_secs: f64) -> bool {
        let non_decreasing = match self.last_depth {
            Some(prev) => depth >= prev,
            None => true,
        };
        self.last_depth = Some(depth);

        if non_decreasing {
            let since = self.non_decreasing_since_secs.get_or_insert(now_secs);
            (now_secs - *since) >= QUEUE_NO_DRAIN_TIMEOUT.as_secs_f64()
        } else {
            // The queue drained: reset the anchor to this observation.
            self.non_decreasing_since_secs = Some(now_secs);
            false
        }
    }
}

/// The ceiling-detection state machine. Feed it samples via
/// [`Self::observe`]; it returns the events triggered by that sample (usually
/// empty).
#[derive(Debug)]
pub struct CeilingDetector {
    state: CeilingState,
    rolling: VecDeque<Duration>,
    queue_tracker: QueueDrainTracker,
    /// Monotonic seconds since cell start, for the queue-no-drain timer.
    elapsed_secs: f64,
    /// Length of the current uninterrupted run of overloaded responses; any
    /// non-overloaded sample resets it.
    overload_run: u32,
}

impl Default for CeilingDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CeilingDetector {
    pub fn new() -> Self {
        Self {
            state: CeilingState::BaselineCollecting {
                samples: VecDeque::with_capacity(BASELINE_SAMPLE_COUNT),
            },
            rolling: VecDeque::with_capacity(ROLLING_WINDOW),
            queue_tracker: QueueDrainTracker::default(),
            elapsed_secs: 0.0,
            overload_run: 0,
        }
    }

    /// True once a catastrophic condition has been reached and the cell should
    /// terminate.
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, CeilingState::Catastrophic { .. })
    }

    /// Advances the internal monotonic clock used by the queue-no-drain timer.
    /// Drivers call this with the seconds elapsed since the cell started before
    /// (or as part of) observing the corresponding sample.
    pub fn set_elapsed_secs(&mut self, elapsed_secs: f64) {
        self.elapsed_secs = elapsed_secs;
    }

    /// Consumes one sample, returning the events it triggered.
    pub fn observe(&mut self, sample: &Sample) -> Vec<CeilingEvent> {
        let mut events = Vec::new();

        // Catastrophic conditions are checked first and independently of the
        // latency-based ceilings: they can fire from any non-terminal state.
        if !self.is_terminal()
            && let Some(reason) = self.catastrophic_reason(sample)
        {
            self.state = CeilingState::Catastrophic { reason };
            events.push(CeilingEvent::Catastrophic {
                at: sample.coord,
                reason,
                snapshot: sample.snapshot.clone(),
            });
            return events;
        }

        match &mut self.state {
            CeilingState::BaselineCollecting { samples } => {
                samples.push_back(sample.latency);
                if samples.len() >= BASELINE_SAMPLE_COUNT {
                    let baseline_p50 = median(samples.iter().copied());
                    self.state = CeilingState::Measuring {
                        baseline_p50,
                        soft_crossed: false,
                        usability_crossed: false,
                        hard_crossed: false,
                    };
                }
            }
            CeilingState::Measuring { .. } => {
                self.push_rolling(sample.latency);
                self.check_measuring(sample, &mut events);
            }
            CeilingState::Catastrophic { .. } => {}
        }

        events
    }

    /// Returns a catastrophic [`TerminatedReason`] if `sample` trips one of the
    /// non-latency conditions, or the escalated-timeout latency condition.
    fn catastrophic_reason(&mut self, sample: &Sample) -> Option<TerminatedReason> {
        if !sample.connection_alive {
            return Some(TerminatedReason::ConnectionLost);
        }
        if sample.pod_restart_count > 0 {
            return Some(TerminatedReason::PodRestart);
        }
        // A sustained run of overloaded (503) responses means the platform is
        // shedding the offered load. A handful is tolerable; OVERLOAD_RUN_LENGTH
        // consecutive ones is catastrophic.
        if sample.overloaded {
            self.overload_run += 1;
            if self.overload_run >= OVERLOAD_RUN_LENGTH {
                return Some(TerminatedReason::Overloaded);
            }
        } else {
            self.overload_run = 0;
        }
        // The escalated 5-minute timeout firing is modelled as a sample whose
        // latency reaches it. Only meaningful once the hard ceiling escalated
        // the timeout, but a sample at/above 5 minutes is catastrophic
        // regardless.
        if sample.latency >= ESCALATED_TIMEOUT {
            return Some(TerminatedReason::OomKill);
        }
        // Schedule-density only: queue depth has not drained for 60s.
        if let Some(depth) = sample.queue_depth
            && self.queue_tracker.observe(depth, self.elapsed_secs)
        {
            return Some(TerminatedReason::LagRunaway);
        }
        None
    }

    fn check_measuring(&mut self, sample: &Sample, events: &mut Vec<CeilingEvent>) {
        let rolling_p99 = percentile(99.0, &self.rolling);
        if let CeilingState::Measuring {
            baseline_p50,
            soft_crossed,
            usability_crossed,
            hard_crossed,
        } = &mut self.state
        {
            let window_full = self.rolling.len() >= ROLLING_WINDOW;

            // Soft ceiling: rolling p99 over the window exceeds 5× baseline.
            if !*soft_crossed
                && window_full
                && rolling_p99 > *baseline_p50 * SOFT_CEILING_MULTIPLIER
            {
                *soft_crossed = true;
                events.push(CeilingEvent::SoftCrossed {
                    at: sample.coord,
                    snapshot: sample.snapshot.clone(),
                });
            }

            // Usability ceiling: rolling p99 exceeds the absolute SLO.
            if !*usability_crossed && window_full && rolling_p99 > USABILITY_CEILING_P99 {
                *usability_crossed = true;
                events.push(CeilingEvent::UsabilityCrossed {
                    at: sample.coord,
                    snapshot: sample.snapshot.clone(),
                });
            }

            // Hard ceiling: a single sample exceeds 30s. Escalate the timeout.
            if !*hard_crossed && sample.latency > HARD_CEILING_THRESHOLD {
                *hard_crossed = true;
                events.push(CeilingEvent::HardCrossed { at: sample.coord });
                events.push(CeilingEvent::EscalateTimeout);
            }
        }
    }

    fn push_rolling(&mut self, latency: Duration) {
        if self.rolling.len() == ROLLING_WINDOW {
            self.rolling.pop_front();
        }
        self.rolling.push_back(latency);
    }
}

/// Median of an iterator of durations. Empty input returns zero.
fn median(values: impl Iterator<Item = Duration>) -> Duration {
    let mut sorted: Vec<Duration> = values.collect();
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    sorted.sort_unstable();
    percentile_sorted(50.0, &sorted)
}

/// Nearest-rank-with-linear-interpolation percentile over an unsorted window.
fn percentile(k: f64, values: &VecDeque<Duration>) -> Duration {
    if values.is_empty() {
        return Duration::ZERO;
    }
    let mut sorted: Vec<Duration> = values.iter().copied().collect();
    sorted.sort_unstable();
    percentile_sorted(k, &sorted)
}

fn percentile_sorted(k: f64, sorted: &[Duration]) -> Duration {
    debug_assert!(!sorted.is_empty());
    debug_assert!((0.0..=100.0).contains(&k));
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let p = (k / 100.0) * (n as f64 - 1.0);
    let lo = p.floor() as usize;
    let hi = p.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let frac = p - lo as f64;
        let lo_v = sorted[lo].as_secs_f64();
        let hi_v = sorted[hi].as_secs_f64();
        Duration::from_secs_f64(lo_v + (hi_v - lo_v) * frac)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A healthy sample at the given latency and agent count.
    fn ok_sample(latency: Duration, agents: u32) -> Sample {
        Sample {
            latency,
            coord: SampleCoord::Agents(agents),
            pod_restart_count: 0,
            connection_alive: true,
            overloaded: false,
            snapshot: CrossAxisSnapshot::default(),
            queue_depth: None,
        }
    }

    /// Feeds 100 baseline samples of `baseline` latency, asserting no events
    /// fire and the machine has transitioned out of baseline collection.
    fn feed_baseline(detector: &mut CeilingDetector, baseline: Duration) {
        for i in 0..BASELINE_SAMPLE_COUNT {
            let events = detector.observe(&ok_sample(baseline, i as u32));
            assert!(events.is_empty(), "baseline samples must not emit events");
        }
    }

    #[test]
    fn baseline_collection_ends_after_100_samples() {
        let mut d = CeilingDetector::new();
        // 99 samples: still collecting.
        for i in 0..(BASELINE_SAMPLE_COUNT - 1) {
            assert!(d.observe(&ok_sample(ms(10), i as u32)).is_empty());
        }
        assert!(matches!(d.state, CeilingState::BaselineCollecting { .. }));
        // 100th sample flips to Measuring.
        assert!(d.observe(&ok_sample(ms(10), 99)).is_empty());
        assert!(matches!(d.state, CeilingState::Measuring { .. }));
    }

    #[test]
    fn soft_ceiling_fires_when_rolling_p99_exceeds_5x_baseline() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10)); // baseline p50 = 10ms

        // Stay under 5× (50ms): no soft crossing.
        for i in 0..ROLLING_WINDOW {
            let events = d.observe(&ok_sample(ms(40), 100 + i as u32));
            assert!(events.is_empty(), "40ms < 50ms threshold");
        }

        // Fill the rolling window with samples well above 5× baseline.
        let mut soft_fired = false;
        for i in 0..ROLLING_WINDOW {
            for ev in d.observe(&ok_sample(ms(100), 200 + i as u32)) {
                if let CeilingEvent::SoftCrossed { at, .. } = ev {
                    soft_fired = true;
                    assert!(matches!(at, SampleCoord::Agents(_)));
                }
            }
        }
        assert!(soft_fired, "soft ceiling must fire above 5× baseline");
    }

    #[test]
    fn soft_ceiling_fires_only_once() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        let mut soft_count = 0;
        for i in 0..(ROLLING_WINDOW * 3) {
            for ev in d.observe(&ok_sample(ms(200), 200 + i as u32)) {
                if matches!(ev, CeilingEvent::SoftCrossed { .. }) {
                    soft_count += 1;
                }
            }
        }
        assert_eq!(soft_count, 1, "soft ceiling must only emit once");
    }

    #[test]
    fn usability_ceiling_fires_when_rolling_p99_exceeds_absolute_slo() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10)); // baseline p50 = 10ms; soft fires at 50ms

        // Fill the window with samples above the 1s usability SLO but below the
        // 30s hard ceiling. Both soft (>5× baseline) and usability (>1s) fire.
        let mut usability_fired = false;
        let mut soft_fired = false;
        for i in 0..ROLLING_WINDOW {
            for ev in d.observe(&ok_sample(ms(2000), 300 + i as u32)) {
                match ev {
                    CeilingEvent::UsabilityCrossed { .. } => usability_fired = true,
                    CeilingEvent::SoftCrossed { .. } => soft_fired = true,
                    _ => {}
                }
            }
        }
        assert!(usability_fired, "usability ceiling must fire above the SLO");
        assert!(soft_fired, "soft ceiling also fires");
    }

    #[test]
    fn usability_ceiling_does_not_fire_below_slo() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        // 100ms is above 5× baseline (soft) but below the 1s usability SLO.
        for i in 0..(ROLLING_WINDOW * 2) {
            for ev in d.observe(&ok_sample(ms(100), 300 + i as u32)) {
                assert!(
                    !matches!(ev, CeilingEvent::UsabilityCrossed { .. }),
                    "usability must not fire below the SLO"
                );
            }
        }
    }

    #[test]
    fn hard_ceiling_triggers_timeout_escalation() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));

        let events = d.observe(&ok_sample(HARD_CEILING_THRESHOLD + ms(1), 500));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, CeilingEvent::HardCrossed { .. })),
            "hard ceiling must fire above 30s"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, CeilingEvent::EscalateTimeout)),
            "hard ceiling must raise timeout escalation"
        );
    }

    #[test]
    fn catastrophic_fires_on_pod_restart() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        let mut s = ok_sample(ms(10), 700);
        s.pod_restart_count = 1;
        let events = d.observe(&s);
        assert!(matches!(
            events.as_slice(),
            [CeilingEvent::Catastrophic {
                reason: TerminatedReason::PodRestart,
                ..
            }]
        ));
        assert!(d.is_terminal());
    }

    #[test]
    fn catastrophic_fires_on_connection_lost() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        let mut s = ok_sample(ms(10), 700);
        s.connection_alive = false;
        let events = d.observe(&s);
        assert!(matches!(
            events.as_slice(),
            [CeilingEvent::Catastrophic {
                reason: TerminatedReason::ConnectionLost,
                ..
            }]
        ));
        assert!(d.is_terminal());
    }

    #[test]
    fn catastrophic_fires_on_five_minute_timeout() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        let events = d.observe(&ok_sample(ESCALATED_TIMEOUT, 800));
        assert!(matches!(
            events.as_slice(),
            [CeilingEvent::Catastrophic {
                reason: TerminatedReason::OomKill,
                ..
            }]
        ));
        assert!(d.is_terminal());
    }

    #[test]
    fn catastrophic_fires_on_queue_no_drain() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));

        // Queue depth non-decreasing for just under the timeout: no fire.
        let mut s = ok_sample(ms(10), 900);
        s.queue_depth = Some(100);
        d.set_elapsed_secs(0.0);
        assert!(d.observe(&s).is_empty());

        d.set_elapsed_secs(QUEUE_NO_DRAIN_TIMEOUT.as_secs_f64() - 1.0);
        let mut s = ok_sample(ms(10), 901);
        s.queue_depth = Some(100);
        assert!(d.observe(&s).is_empty());

        // At/after the timeout, still non-decreasing: catastrophic lag-runaway.
        d.set_elapsed_secs(QUEUE_NO_DRAIN_TIMEOUT.as_secs_f64() + 1.0);
        let mut s = ok_sample(ms(10), 902);
        s.queue_depth = Some(101);
        let events = d.observe(&s);
        assert!(matches!(
            events.as_slice(),
            [CeilingEvent::Catastrophic {
                reason: TerminatedReason::LagRunaway,
                ..
            }]
        ));
    }

    #[test]
    fn queue_drain_resets_timer() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));

        let mut s = ok_sample(ms(10), 900);
        s.queue_depth = Some(100);
        d.set_elapsed_secs(0.0);
        assert!(d.observe(&s).is_empty());

        // Depth decreased: timer resets.
        d.set_elapsed_secs(30.0);
        let mut s = ok_sample(ms(10), 901);
        s.queue_depth = Some(50);
        assert!(d.observe(&s).is_empty());

        // Even well past the original 60s window, no fire because it reset.
        d.set_elapsed_secs(70.0);
        let mut s = ok_sample(ms(10), 902);
        s.queue_depth = Some(60);
        assert!(d.observe(&s).is_empty());
    }

    #[test]
    fn no_events_after_terminal() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        let mut s = ok_sample(ms(10), 700);
        s.pod_restart_count = 1;
        assert!(!d.observe(&s).is_empty());
        // Subsequent samples produce nothing.
        assert!(d.observe(&ok_sample(ESCALATED_TIMEOUT, 701)).is_empty());
        assert!(d.is_terminal());
    }

    #[test]
    fn terminated_reason_codes_match_schema() {
        assert_eq!(TerminatedReason::OomKill.code(), 1);
        assert_eq!(TerminatedReason::PodRestart.code(), 2);
        assert_eq!(TerminatedReason::ConnectionLost.code(), 3);
        assert_eq!(TerminatedReason::UpperBoundHit.code(), 4);
        assert_eq!(TerminatedReason::LagRunaway.code(), 5);
        assert_eq!(TerminatedReason::Overloaded.code(), 6);
    }

    #[test]
    fn occasional_overloads_do_not_fire() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        // A short run of 503s, broken by a healthy sample, never reaches the
        // run length and resets each time.
        for i in 0..(OVERLOAD_RUN_LENGTH * 4) {
            let mut s = ok_sample(ms(10), 700 + i);
            s.overloaded = i % (OVERLOAD_RUN_LENGTH / 2) != 0;
            assert!(d.observe(&s).iter().all(|e| !matches!(
                e,
                CeilingEvent::Catastrophic {
                    reason: TerminatedReason::Overloaded,
                    ..
                }
            )));
        }
        assert!(!d.is_terminal());
    }

    #[test]
    fn catastrophic_fires_on_sustained_overload() {
        let mut d = CeilingDetector::new();
        feed_baseline(&mut d, ms(10));
        let mut events = Vec::new();
        for i in 0..OVERLOAD_RUN_LENGTH {
            let mut s = ok_sample(ms(10), 700 + i);
            s.overloaded = true;
            events = d.observe(&s);
        }
        assert!(matches!(
            events.as_slice(),
            [CeilingEvent::Catastrophic {
                reason: TerminatedReason::Overloaded,
                ..
            }]
        ));
        assert!(d.is_terminal());
    }
}
