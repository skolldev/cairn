//! The Budget rules' statistics: p95 over at least 20 samples per run, worst of three runs,
//! worst sample recorded. Launch defines its own statistic in its row.

/// Runs a Budget needs, and samples each run needs, before its result counts.
pub const MIN_RUNS: usize = 3;
pub const MIN_SAMPLES: usize = 20;

/// The numbers from the plan's Budget table, which is the only place they are decided.
/// `the_harness_budgets_are_the_plans` keeps these in step with it.
pub const EXTERNAL_CHANGE: Budget = Budget::Latency { limit_ms: 100.0 };
pub const LOCAL_WRITE: Budget = Budget::Latency { limit_ms: 100.0 };
pub const LAUNCH: Budget = Budget::Launch {
    median_limit_ms: 300.0,
    none_over_ms: 400.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Budget {
    /// Passes when each run's p95 is under the limit.
    Latency { limit_ms: f64 },
    /// Passes when each run's median is under the limit and no launch is over the cap.
    Launch {
        median_limit_ms: f64,
        none_over_ms: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Pass,
    Miss(String),
    /// The numbers don't meet the Budget rules, so they say nothing either way.
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub verdict: Verdict,
    /// The Budget's statistic, from its worst run.
    pub statistic_ms: f64,
    /// The p95 of the worst run, recorded for every Budget, Launch included.
    pub p95_ms: f64,
    pub worst_sample_ms: f64,
    pub samples_per_run: Vec<usize>,
}

/// The sample at `fraction` of the way up, by nearest rank. 0 for no samples.
fn percentile(samples: &[f64], fraction: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (sorted.len() as f64 * fraction).ceil() as usize;
    rank.checked_sub(1)
        .and_then(|index| sorted.get(index))
        .copied()
        .unwrap_or(0.0)
}

/// The 95th percentile, by nearest rank: one of the samples, never an interpolation.
pub fn p95(samples: &[f64]) -> f64 {
    percentile(samples, 0.95)
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

/// Judges a Budget on its runs. The plan's limits are all "under", so a result exactly at a
/// limit misses, and "none over" allows one exactly at the cap.
pub fn judge(budget: &Budget, runs: &[Vec<f64>]) -> Summary {
    let worst_of = |statistic: fn(&[f64]) -> f64| {
        runs.iter()
            .map(|run| statistic(run))
            .fold(0.0_f64, f64::max)
    };
    let worst_sample_ms = runs.iter().flatten().copied().fold(0.0_f64, f64::max);
    let p95_ms = worst_of(p95);
    let samples_per_run: Vec<usize> = runs.iter().map(Vec::len).collect();

    let statistic_ms = match budget {
        Budget::Latency { .. } => p95_ms,
        Budget::Launch { .. } => worst_of(median),
    };
    let verdict = if runs.len() < MIN_RUNS {
        Verdict::Invalid(format!(
            "{} run(s), and the rules need {MIN_RUNS}",
            runs.len()
        ))
    } else if let Some(short) = samples_per_run.iter().find(|n| **n < MIN_SAMPLES) {
        Verdict::Invalid(format!(
            "a run has {short} sample(s), and the rules need {MIN_SAMPLES}"
        ))
    } else {
        match budget {
            Budget::Latency { limit_ms } if statistic_ms >= *limit_ms => Verdict::Miss(format!(
                "p95 {statistic_ms:.1} ms, limit under {limit_ms} ms"
            )),
            Budget::Launch {
                median_limit_ms, ..
            } if statistic_ms >= *median_limit_ms => Verdict::Miss(format!(
                "median {statistic_ms:.1} ms, limit under {median_limit_ms} ms"
            )),
            Budget::Launch { none_over_ms, .. } if worst_sample_ms > *none_over_ms => {
                Verdict::Miss(format!(
                    "a launch took {worst_sample_ms:.1} ms, and none may be over {none_over_ms} ms"
                ))
            }
            _ => Verdict::Pass,
        }
    };
    Summary {
        verdict,
        statistic_ms,
        p95_ms,
        worst_sample_ms,
        samples_per_run,
    }
}

/// #8's Key input Floor: no presented gap over this. The plan's table has no held-key row.
pub const HELD_KEY_GAP_FLOOR_MS: f64 = 50.0;
/// zed#61469 was reported after 5 to 30 held keys, so a run needs at least this many.
pub const MIN_HELD_KEYS: usize = 30;

/// One run of the held-key test.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldKeyRun {
    /// Key-downs the OS flagged as repeats.
    pub held_keys: usize,
    /// Status answers the window showed during the hold.
    pub notifies: u64,
    /// The longest the screen went without a presented frame during the hold.
    pub largest_gap_ms: f64,
}

/// Judges the held-key test: worst of three runs, as the Floor says.
pub fn judge_held_key(runs: &[HeldKeyRun]) -> Summary {
    let largest = runs
        .iter()
        .map(|r| r.largest_gap_ms)
        .fold(0.0_f64, f64::max);
    let verdict = if runs.len() < MIN_RUNS {
        Verdict::Invalid(format!(
            "{} run(s), and the rules need {MIN_RUNS}",
            runs.len()
        ))
    } else if let Some(run) = runs.iter().find(|r| r.held_keys < MIN_HELD_KEYS) {
        Verdict::Invalid(format!(
            "a run saw {} repeated keys, and needs {MIN_HELD_KEYS} real ones",
            run.held_keys
        ))
    } else if runs.iter().any(|r| r.notifies == 0) {
        Verdict::Invalid("a run showed no background notifies".to_owned())
    } else if largest > HELD_KEY_GAP_FLOOR_MS {
        Verdict::Miss(format!(
            "a {largest:.1} ms gap between presented frames, floor {HELD_KEY_GAP_FLOOR_MS} ms"
        ))
    } else {
        Verdict::Pass
    };
    Summary {
        verdict,
        statistic_ms: largest,
        p95_ms: largest,
        worst_sample_ms: largest,
        samples_per_run: runs.iter().map(|r| r.held_keys).collect(),
    }
}
