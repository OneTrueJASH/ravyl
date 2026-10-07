//! Self-play strength harness: plays two engine variants against each other
//! over paired, fixed-seed games and reports A's score and mean spread with
//! 95% confidence intervals.
//!
//! ```text
//! cargo run --release --example selfplay -- <experiment> [pairs] [seed]
//! ```
//!
//! Experiments (A is the first variant named):
//! - `phase`: smoothed leave phase scaling vs the stepped one
//! - `pass`: passing allowed in the greedy phase (bag 1-6) vs not
//! - `sim`: moves chosen by simulating the top 10 candidates (48 playouts
//!   each) vs static equity (slow: about a second per game)
//! - `same`: the default engine against itself (a sanity check: 0.5)

use std::path::Path;
use std::time::Instant;

use cpsolver2::dictionary::{load_dictionary_from, PhaseScaling};
use cpsolver2::mcts::{self_play_match, MatchEngine, SimChoice};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let experiment = args.first().map(String::as_str).unwrap_or("same");
    let pairs: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(500);
    let seed: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(2026);

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let load = |phase| {
        load_dictionary_from(root)
            .expect("dictionary")
            .with_phase_scaling(phase)
    };
    let stepped = load(PhaseScaling::Stepped);
    let smoothed = load(PhaseScaling::Smoothed);
    if !stepped.has_leave_table() {
        eprintln!("warning: no leave table (NWL23.klv2 missing or an LFS stub); heuristic leaves");
    }

    let (a, b) = match experiment {
        "phase" => (
            MatchEngine {
                name: "smoothed phase",
                dict: &smoothed,
                allow_pass: true,
                sim: None,
            },
            MatchEngine {
                name: "stepped phase",
                dict: &stepped,
                allow_pass: true,
                sim: None,
            },
        ),
        "pass" => (
            MatchEngine {
                name: "pass allowed",
                dict: &smoothed,
                allow_pass: true,
                sim: None,
            },
            MatchEngine {
                name: "no pass",
                dict: &smoothed,
                allow_pass: false,
                sim: None,
            },
        ),
        "same" => (
            MatchEngine {
                name: "default",
                dict: &smoothed,
                allow_pass: true,
                sim: None,
            },
            MatchEngine {
                name: "default",
                dict: &smoothed,
                allow_pass: true,
                sim: None,
            },
        ),
        "sim" => (
            MatchEngine {
                name: "simulation (top 10, 48 playouts)",
                dict: &smoothed,
                allow_pass: true,
                sim: Some(SimChoice {
                    candidates: 10,
                    playouts: 48,
                }),
            },
            MatchEngine {
                name: "static equity",
                dict: &smoothed,
                allow_pass: true,
                sim: None,
            },
        ),
        other => {
            eprintln!("unknown experiment '{other}' (phase, pass, sim, same)");
            std::process::exit(2);
        }
    };

    let started = Instant::now();
    let r = self_play_match(&a, &b, pairs, seed);
    println!(
        "{} vs {}: {} games, score {:.3} \u{00b1} {:.3}, spread {:+.2} \u{00b1} {:.2} ({:.1?})",
        a.name,
        b.name,
        r.games,
        r.score,
        r.score_ci95,
        r.spread,
        r.spread_ci95,
        started.elapsed()
    );
}
