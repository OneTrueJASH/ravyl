# Contributing

## Setup

```bash
git clone --recurse-submodules https://github.com/OneTrueJASH/cpsolver2
cd cpsolver2
git lfs install && git lfs pull   # the lexicon files are in Git LFS
```

You need Rust 1.88 or newer and a C compiler (`build.rs` compiles the
RefEngine submodule). `--no-default-features` builds without RefEngine.

## Layout

The repository is a Cargo workspace:

- `crates/ravyn/`: **RAVYN**, the engine (move generation, leaves,
  simulation, endgame). MIT licensed, with no dependency on the app or on
  RefEngine; it logs through the `log` facade.
- The root package `cpsolver2`: the TUI, saved games, settings and the
  RefEngine bindings (GPL-3.0). It re-exports the engine's modules at their
  old paths (`cpsolver2::solver`, ...), so either path works.

Engine changes go in `crates/ravyn`; anything about the terminal, files
or RefEngine stays in the root package.

## Before every push

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --release
```

CI runs the same on Linux and macOS, with and without the `refengine`
feature (the no-RefEngine job also runs without LFS data, so data-dependent
tests must skip cleanly when a file is missing or an LFS pointer), checks
the MSRV, and runs `cargo deny check`.

Keep `README.md` and `PRIMER.md` true in the same change as any behavior,
key binding or constant change, and add a line to `CHANGELOG.md`.

## Tests

- Unit tests sit next to the code; `app/tests.rs` drives the app with key
  and click scripts.
- UI snapshots (`src/ui/snapshots/`) use [insta](https://insta.rs). After
  an intended UI change, review them with `cargo insta review` (or rerun
  with `INSTA_UPDATE=always` and read the diff).
- `tests/properties.rs` holds the property tests.
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
cd fuzz && cargo +nightly fuzz run from_cgp   # or leaves_parse
```

## Commits

Small, focused commits with imperative subject lines.
