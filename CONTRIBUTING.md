# Contributing

## Setup

```bash
git clone --recurse-submodules https://github.com/OneTrueJASH/cpsolver2
cd cpsolver2
git lfs install && git lfs pull   # the NWL23.* lexicon files are in Git LFS
```

You need Rust 1.88 or newer and a C compiler (`build.rs` compiles the
RefEngine submodule). `--no-default-features` builds without RefEngine.

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
  in `src/mcts/studies.rs`; each says how to run it.

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
