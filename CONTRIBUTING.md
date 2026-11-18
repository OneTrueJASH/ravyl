# Contributing

## Setup

```bash
git clone https://github.com/OneTrueJASH/ravyl
cd ravyl
```

You need Rust 1.88 or newer; nothing else is compiled.

## Layout

The repository is a Cargo workspace:

- `crates/ravyn/`: **RAVYN**, the engine (rules, move generation,
  leaves, simulation, inference, endgame and pre-endgame). MIT licensed,
  with no dependency on the app; it logs through the `log` facade. Its
  integration, property and reference-engine tests, benchmarks and fuzz
  targets live with it.
- The root package `ravyl`: the TUI, saved games, settings and data
  file discovery (MIT, like the engine). It re-exports the engine's modules at their
  old paths (`ravyl::solver`, ...), so either path works.

Engine changes go in `crates/ravyn`; anything about the terminal or files
stays in the root package.

RAVYN also has its own repository, [OneTrueJASH/ravyn](https://github.com/OneTrueJASH/ravyn)
(private for now), with the engine's full history. Until it is public,
`crates/ravyn` here is where the engine is changed; after engine changes
land, mirror them there with `scripts/sync-ravyn.sh [path to ravyn]`, which
copies the crate and its data and shows what changed for you to commit.
The engine finds `data/` in its own directory when it has one (the
standalone repository) and in the workspace root otherwise, so the same
files work in both.

## Before every push

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --release
```

CI runs the same on Linux and macOS, checks the MSRV, and runs
`cargo deny check`. Data-dependent tests skip cleanly when a data file is
missing.

Keep `README.md` and `PRIMER.md` true in the same change as any behavior,
key binding or constant change, and add a line to `CHANGELOG.md`.

## Tests

- Unit tests sit next to the code; `app/tests.rs` drives the app with key
  and click scripts.
- UI snapshots (`src/ui/snapshots/`) use [insta](https://insta.rs). After
  an intended UI change, review them with `cargo insta review` (or rerun
  with `INSTA_UPDATE=always` and read the diff).
- `crates/ravyn/tests/` holds the integration and property tests, and
  `reference_movegen.rs`: move generation against a reference engine's
  answers, recorded in `fixtures/`. App tests that start background work hold
  `datafiles::test_data::app_lock()` so their time budgets hold.
- Self-play studies that fitted the engine's constants are ignored tests
  in `crates/ravyn/src/mcts/studies.rs`; each says how to run it.

## Measuring

Claims about speed or strength need numbers:

```bash
cargo bench --bench engine                               # criterion
cargo run --release --example selfplay -- phase 2000     # A vs B over paired games
```

## Fuzzing

```bash
cargo install cargo-fuzz
cd crates/ravyn/fuzz && cargo +nightly fuzz run from_cgp   # or leaves_parse
```

## Commits

Small, focused commits with imperative subject lines.
