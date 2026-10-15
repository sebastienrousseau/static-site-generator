// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The scaling gate's estimator, and the simulated machines that pin it.
//!
//! [`super::build_cost_per_page_does_not_grow_with_corpus_size`] times
//! real builds; everything here is the arithmetic that turns those
//! timings into one growth figure, kept apart so it can be tested on a
//! simulated clock without building anything.

use std::time::Duration;

use super::SAMPLES;

/// The small corpus for the scaling gate.
///
/// Corpus sizes for the scaling gate. Both are large enough that fixed
/// start-up cost has amortised, which is what makes the comparison
/// stable — see [`super::build_cost_per_page_does_not_grow_with_corpus_size`].
pub const SCALE_SMALL: usize = 100;
/// The large corpus; see [`SCALE_SMALL`].
pub const SCALE_LARGE: usize = 500;

/// Small builds per round: together they compile as many pages as the
/// one large build, so both halves of a round do the same work.
pub const SMALL_PER_ROUND: usize = SCALE_LARGE / SCALE_SMALL;
const _: () = assert!(SMALL_PER_ROUND * SCALE_SMALL == SCALE_LARGE);

/// Where in a round the large build runs: after this many small builds.
const LARGE_AFTER: usize = 2;

/// One round of the scaling gate: [`SMALL_PER_ROUND`] small builds and
/// one large build, which compile the same number of pages.
#[derive(Debug)]
pub struct Round {
    /// Each small build's time, in the order they ran.
    pub small: Vec<Duration>,
    /// The large build's time.
    pub large: Duration,
}

impl Round {
    /// Per-page cost at the large size over per-page cost at the small
    /// size, within this round. 1.0 for linear cost with no fixed cost.
    pub fn growth(&self) -> f64 {
        let small: Duration = self.small.iter().sum();
        self.large.as_secs_f64() / small.as_secs_f64()
    }
}

/// Runs one round, the large build in among the small ones.
fn scaling_round<F>(build: &mut F) -> Round
where
    F: FnMut(usize) -> Duration,
{
    let mut small = Vec::with_capacity(SMALL_PER_ROUND);
    small.extend((0..LARGE_AFTER).map(|_| build(SCALE_SMALL)));
    let large = build(SCALE_LARGE);
    small.extend((LARGE_AFTER..SMALL_PER_ROUND).map(|_| build(SCALE_SMALL)));
    Round { small, large }
}

/// [`SAMPLES`](super::SAMPLES) rounds of the scaling gate, after one warmup build of
/// each size.
///
/// Each round compares like with like. Its small half is
/// [`SMALL_PER_ROUND`] builds of the small corpus, the same number of
/// pages as its one large build, so on linear code the two halves take
/// about as long, and interference that comes and goes over a few
/// seconds is as likely to land on one half as on the other.
///
/// The previous estimator compared the fastest single small build with
/// the fastest large build, each picked separately. Interleaving the
/// sizes (#794) fixed a slowdown that persisted, but not interference in
/// bursts: a 0.4 s build can fit in a quiet moment between bursts and a
/// 2 s build cannot, so the best small sample was clean and every large
/// one was not. That reads as super-linear scaling, and it fits the
/// #805 failure, where the 500-page build took over 6.6 s in all five
/// rounds while one 100-page build took 0.4 s.
pub fn scaling_rounds<F>(mut build: F) -> Vec<Round>
where
    F: FnMut(usize) -> Duration,
{
    let _ = build(SCALE_SMALL);
    let _ = build(SCALE_LARGE);
    (0..SAMPLES).map(|_| scaling_round(&mut build)).collect()
}

/// The growth the scaling gate asserts on: the median of the rounds'
/// own growth figures.
///
/// A median, where `best_of` takes a minimum. Interference on one build
/// only ever slows it, so the fastest build is the cleanest estimate of
/// its cost. A ratio has no such direction: a slow large half inflates
/// it and a slow small half deflates it, so the smallest ratio would
/// let a real regression through on one unlucky round. The median of
/// five ignores up to two bad rounds either way, and every round of an
/// O(n^2) build still reads near 5.
pub fn growth_estimate(rounds: &[Round]) -> f64 {
    let mut growth: Vec<f64> = rounds.iter().map(Round::growth).collect();
    growth.sort_by(f64::total_cmp);
    growth[growth.len() / 2]
}

/// Per-page growth that the gate reports for a simulated machine, so
/// the estimator itself can be tested without a build.
fn simulated_growth<F>(mut cost: F) -> f64
where
    F: FnMut(usize, usize) -> f64,
{
    let mut call = 0_usize;
    let rounds = scaling_rounds(|n| {
        call += 1;
        Duration::from_secs_f64(cost(n, call))
    });
    growth_estimate(&rounds)
}

/// A runner that slows down part-way through the gate must not read as
/// super-linear scaling. This is the shape of the failure on #794
/// (3.52x on `windows-latest`, then a pass on the same commit): the
/// machine got slower after the first few builds, and only the later
/// measurements paid for it.
#[test]
fn a_runner_slowing_mid_gate_is_not_read_as_super_linear_scaling() {
    // Linear cost, 1 ms per page; from the seventh build on, the
    // machine runs 3.5x slower and stays that way.
    let growth = simulated_growth(|n, call| {
        let slow = if call >= 7 { 3.5 } else { 1.0 };
        n as f64 * 0.001 * slow
    });
    assert!(
        growth <= MAX_PER_PAGE_GROWTH,
        "a mid-gate slowdown read as {growth:.2}x growth"
    );
}

/// The gate must still catch the regression it exists for.
#[test]
fn quadratic_cost_still_fails_the_scaling_gate() {
    let growth = simulated_growth(|n, _| (n as f64).powi(2) * 1e-6);
    assert!(growth > MAX_PER_PAGE_GROWTH, "O(n^2) read as {growth:.2}x");
}

/// The gate must also catch a milder super-linear regression.
#[test]
fn n_to_the_one_and_a_half_still_fails_the_scaling_gate() {
    let growth = simulated_growth(|n, _| (n as f64).powf(1.5) * 1e-5);
    assert!(
        growth > MAX_PER_PAGE_GROWTH,
        "O(n^1.5) read as {growth:.2}x"
    );
}

/// A machine on a simulated clock that alternates quiet windows with
/// bursts of interference, such as a co-tenant on a shared runner or a
/// concurrent test in the same binary. A page is one millisecond of work
/// at full speed; during a burst the machine runs `slowdown` times
/// slower.
struct BurstyMachine {
    now_ms: u64,
    period_ms: u64,
    quiet_ms: u64,
    slowdown: u64,
}

impl BurstyMachine {
    /// Runs a build of `pages` pages and returns how long it took, in
    /// seconds of simulated time.
    fn build(&mut self, pages: usize) -> f64 {
        let start = self.now_ms;
        // In units of a burst millisecond's progress, so a page is
        // `slowdown` units and the arithmetic stays exact.
        let mut work = pages as u64 * self.slowdown;
        while work > 0 {
            let quiet = self.now_ms % self.period_ms < self.quiet_ms;
            work = work.saturating_sub(if quiet { self.slowdown } else { 1 });
            self.now_ms += 1;
        }
        (self.now_ms - start) as f64 / 1000.0
    }
}

/// Interference that leaves quiet windows long enough for a small build
/// but too short for a large one must not read as super-linear scaling.
///
/// This is a shape consistent with the failure that interleaving alone
/// did not stop (3.32x on `windows-latest` for #805, then a pass on the
/// rerun of the same commit): the fastest of five 100-page builds can
/// land in a quiet window, while no 500-page build can avoid a burst, so
/// a best-of over each size separately compares a clean small build
/// against a contaminated large one. Every machine in this family has
/// linear cost. The estimator before this change read up to 4.20x on
/// it.
#[test]
fn quiet_windows_that_fit_only_the_small_build_are_not_read_as_super_linear_scaling(
) {
    let mut worst = (0.0_f64, 0, 0, 0);
    for quiet_ms in [100, 150, 200, 300, 400] {
        for period_ms in [
            quiet_ms + 100,
            quiet_ms + 300,
            2 * quiet_ms,
            3 * quiet_ms,
            1000,
            2000,
        ] {
            for slowdown in [3, 5] {
                let mut machine = BurstyMachine {
                    now_ms: 0,
                    period_ms,
                    quiet_ms,
                    slowdown,
                };
                let growth = simulated_growth(|n, _| machine.build(n));
                if growth > worst.0 {
                    worst = (growth, quiet_ms, period_ms, slowdown);
                }
            }
        }
    }
    let (growth, quiet_ms, period_ms, slowdown) = worst;
    assert!(
        growth <= MAX_PER_PAGE_GROWTH,
        "bursty interference read as {growth:.2}x growth ({quiet_ms} ms \
         quiet every {period_ms} ms, {slowdown}x slower otherwise)"
    );
}

/// How much per-page cost may grow between the two sizes.
///
/// Measured growth across four runs, including two on a machine loaded
/// by a concurrent test suite, was 0.47, 0.50, 1.14 and 1.19 — at or
/// below parity, as linear scaling predicts once fixed cost amortises.
///
/// The paired-round median that replaced that estimator measured 0.81,
/// 0.83, 0.88, 0.89, 0.99 and 1.00 over six local runs on a six-core
/// machine under a concurrent fuzzing workload (load average 23 to
/// 119), with single rounds ranging from 0.42 to 1.26: the median is
/// what keeps one disturbed round from deciding the verdict.
///
/// The threshold sits well clear of both ends. Above it: O(n^2) puts
/// this at 5.0 and O(n^1.5) at 2.24, both caught; O(n log n) reaches
/// only 1.35, correctly allowed. Below it: 1.7x headroom over the worst
/// legitimate sample observed with the earlier estimator, and 2x over
/// the worst median observed with this one.
///
/// Noise is not one-sided across the whole gate, though. A runner that
/// slows down part-way through inflates whichever measurements come
/// after the slowdown; with all large samples taken last, that read as
/// 3.52x on `windows-latest` for code that passed on the next run
/// (#794). Interference in bursts did the same to the interleaved
/// estimator that replaced it, at 3.32x (#805). [`scaling_rounds`]
/// compares equal amounts of work within each round and
/// [`growth_estimate`] takes the median round, and the tests above pin
/// that estimator against simulated machines: a persistent slowdown,
/// bursty interference, and O(n^2) and O(n^1.5) costs that must still
/// fail.
pub const MAX_PER_PAGE_GROWTH: f64 = 2.0;
