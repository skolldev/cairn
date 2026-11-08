//! The Budget rules: p95 over at least 20 samples per run, worst of three runs, worst sample
//! recorded. Launch has its own statistic. Expected values are worked by hand.

use cairn_budget::stats::{Budget, EXTERNAL_CHANGE, LAUNCH, LOCAL_WRITE, Verdict, judge, p95};

/// 1, 2, ..., 20 ms, shuffled by reversing: p95 by nearest rank is the 19th, so 19 ms.
fn one_to_twenty() -> Vec<f64> {
    (1..=20).rev().map(f64::from).collect()
}

#[test]
fn p95_is_the_nearest_rank_sample() {
    assert_eq!(p95(&one_to_twenty()), 19.0);
    // 21 samples: rank is ceil(19.95) = 20.
    let mut samples: Vec<f64> = (1..=21).map(f64::from).collect();
    samples.reverse();
    assert_eq!(p95(&samples), 20.0);
}

#[test]
fn a_latency_budget_passes_on_the_worst_runs_p95_and_absorbs_one_outlier() {
    let budget = Budget::Latency { limit_ms: 100.0 };
    let mut calm = vec![50.0; 20];
    calm[0] = 900.0; // One outlier of 20 is the top 5%, which p95 absorbs.
    let runs = vec![vec![10.0; 20], calm, vec![30.0; 20]];
    let summary = judge(&budget, &runs);
    assert_eq!(summary.verdict, Verdict::Pass);
    assert_eq!(summary.statistic_ms, 50.0, "worst run's p95");
    assert_eq!(
        summary.worst_sample_ms, 900.0,
        "recorded even when absorbed"
    );
}

#[test]
fn a_latency_budget_misses_when_one_run_misses() {
    let budget = Budget::Latency { limit_ms: 100.0 };
    let runs = vec![vec![10.0; 20], vec![10.0; 20], vec![100.0; 20]];
    let summary = judge(&budget, &runs);
    // "< 100 ms" in the plan's table: exactly the limit misses.
    assert!(matches!(summary.verdict, Verdict::Miss(_)), "{summary:?}");
}

#[test]
fn launch_passes_on_its_own_statistic_median_and_none_over() {
    let budget = Budget::Launch {
        median_limit_ms: 300.0,
        none_over_ms: 400.0,
    };
    // p95 would be 399, but the row's statistic is the median: 250 ms.
    let mut samples = vec![250.0; 20];
    samples[0] = 399.0;
    let summary = judge(&budget, &[samples.clone(), samples.clone(), samples]);
    assert_eq!(summary.verdict, Verdict::Pass);
    assert_eq!(summary.statistic_ms, 250.0);
}

#[test]
fn launch_misses_on_a_single_launch_over_its_cap_in_any_run() {
    let budget = Budget::Launch {
        median_limit_ms: 300.0,
        none_over_ms: 400.0,
    };
    let mut slow = vec![250.0; 20];
    slow[7] = 401.0;
    let summary = judge(&budget, &[vec![250.0; 20], slow, vec![250.0; 20]]);
    assert!(matches!(summary.verdict, Verdict::Miss(_)), "{summary:?}");
    assert_eq!(summary.worst_sample_ms, 401.0);
}

#[test]
fn launch_misses_on_a_median_at_its_limit() {
    let budget = Budget::Launch {
        median_limit_ms: 300.0,
        none_over_ms: 400.0,
    };
    let run = vec![300.0; 20];
    let summary = judge(&budget, &[run.clone(), run.clone(), run]);
    assert!(matches!(summary.verdict, Verdict::Miss(_)), "{summary:?}");
}

#[test]
fn too_few_samples_or_runs_never_pass() {
    let budget = Budget::Latency { limit_ms: 100.0 };
    let two_runs = judge(&budget, &[vec![1.0; 20], vec![1.0; 20]]);
    assert!(
        matches!(two_runs.verdict, Verdict::Invalid(_)),
        "{two_runs:?}"
    );
    let short_run = judge(&budget, &[vec![1.0; 20], vec![1.0; 19], vec![1.0; 20]]);
    assert!(
        matches!(short_run.verdict, Verdict::Invalid(_)),
        "{short_run:?}"
    );
}

/// The plan's table is the only place Budgets are written down, so the harness's copy of each
/// number must follow it.
#[test]
fn the_harness_budgets_are_the_plans() {
    let plan = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Git Client (GPUI + gix) Project Plan.md"
    ))
    .unwrap();
    let row = |start: &str| {
        plan.lines()
            .find(|line| line.starts_with(start))
            .unwrap_or_else(|| panic!("no plan row starts with {start:?}"))
            .to_owned()
    };
    let Budget::Latency { limit_ms } = EXTERNAL_CHANGE else {
        panic!()
    };
    assert!(row("| External change").contains(&format!("| < {limit_ms} ms |")));
    let Budget::Latency { limit_ms } = LOCAL_WRITE else {
        panic!()
    };
    assert!(row("| Intent to UI").contains(&format!("| < {limit_ms} ms |")));
    let Budget::Launch {
        median_limit_ms,
        none_over_ms,
    } = LAUNCH
    else {
        panic!()
    };
    assert!(row("| Launch").contains(&format!(
        "median < {median_limit_ms} ms, none over {none_over_ms} ms, across 20 warm launches"
    )));
}

mod held_key {
    use cairn_budget::stats::{HELD_KEY_GAP_FLOOR_MS, HeldKeyRun, Verdict, judge_held_key};

    fn run(held_keys: usize, notifies: u64, largest_gap_ms: f64) -> HeldKeyRun {
        HeldKeyRun {
            held_keys,
            notifies,
            largest_gap_ms,
        }
    }

    #[test]
    fn a_held_key_passes_when_no_run_leaves_the_screen_still_past_the_floor() {
        let good = run(300, 500, 16.7);
        let summary = judge_held_key(&[good.clone(), run(290, 480, 33.3), good]);
        assert_eq!(summary.verdict, Verdict::Pass);
        assert_eq!(summary.statistic_ms, 33.3, "the worst run's gap");
        assert_eq!(HELD_KEY_GAP_FLOOR_MS, 50.0, "#8's Key input Floor");
    }

    #[test]
    fn a_freeze_in_any_run_misses() {
        let good = run(300, 500, 16.7);
        let summary = judge_held_key(&[good.clone(), run(300, 500, 2_000.0), good]);
        assert!(matches!(summary.verdict, Verdict::Miss(_)), "{summary:?}");
    }

    #[test]
    fn a_run_without_real_repeat_or_notifies_proves_nothing() {
        let good = run(300, 500, 16.7);
        // zed#61469 was reported after 5 to 30 held keys, so fewer than 30 hasn't tried it.
        for bad in [run(29, 500, 16.7), run(300, 0, 16.7)] {
            let summary = judge_held_key(&[good.clone(), bad, good.clone()]);
            assert!(
                matches!(summary.verdict, Verdict::Invalid(_)),
                "{summary:?}"
            );
        }
        let two = judge_held_key(&[good.clone(), good]);
        assert!(matches!(two.verdict, Verdict::Invalid(_)), "{two:?}");
    }
}
