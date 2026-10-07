# Changelog

All notable changes to this project are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Simulation of every candidate (every legal play, every distinct
  exchange, and pass) with an anytime scheduler: a playout floor for every
  candidate, racing on spread and win rate, widening for blank racks, and
  live results in the table. Esc stops a run; M on the same position
  continues it.
- Table columns for playouts, spread and win rate with standard errors,
  the paired difference to the best play, bingo rates and a status.
  Clicking a column header sorts by it.
- Suggestions list navigation: paging, Home/End, j/k, 1–9, `/` filter by
  word or square with n/N, a persistent scroll position, and a `12/340`
  indicator.
- `:` coordinate entry (`8D QUIXOTIC`, lowercase for blanks) with a live
  legality and score preview; Shift+letter places a blank on the board.
- Board axes (A–O, 1–15) and a cursor readout.
- Redo (Ctrl+Y); reset and clearing the rack can be undone.
- Session autosave and restore, CGP paste, and copying the selected move
  or the position to the clipboard (OSC 52).
- Themes (dark, light, ansi16, no-color) chosen by `display.theme`,
  honoring `NO_COLOR` and `COLORTERM`, with ASCII fallbacks.
- Stacked layout for narrow terminals and a "terminal too small" message.
- A data-directory search order (`--data-dir`, `$CPSOLVER_DATA_DIR`, the
  user data directory, `<exe>/../share/cpsolver2`, the working directory)
  and `--help` / `--version`.
- Criterion benchmarks, a self-play strength harness
  (`examples/selfplay.rs`), property tests, a random-position move
  generation comparison with RefEngine, and cargo-fuzz targets.
- CI on Linux and macOS, a no-RefEngine build, an MSRV check, and
  cargo-deny; a release workflow (dist) that runs only by hand.

### Changed
- Licensed under GPL-3.0-or-later (was MIT), since the binary links
  GPL-3.0 RefEngine; copyright holder corrected to OneTrueJASH.
- `--data-dir` now takes precedence over `$CPSOLVER_DATA_DIR`.
- RefEngine is compiled from vendored source (a pinned submodule) and linked
  statically, behind the default `refengine` cargo feature; without it the
  pure-Rust solver builds on its own.
- Playouts are 3–5× faster: best-move searches that build only the
  winning moves, a per-rack subset leave table, a flat DAWG for move
  generation, and batch-level parallelism. The endgame solver is 3×
  faster.
- Common random numbers: every candidate sees the same opponent rack and
  bag order in each playout.
- Leave weights fall off linearly as the bag empties instead of in steps
  (+0.5 points of spread per game in self-play); leaves with three blanks
  are valued on the leave table's scale.
- Logs moved from `/tmp` to the user state (or cache) directory, created
  0600, pruned at startup; the log viewer reads from memory.
- Settings tolerate missing keys, back up an unreadable file, and are
  written atomically.
- The binary is a thin event loop over the library; `app` and `mcts` are
  split into submodules.

### Fixed
- Opponent rack inference returned the uniform prior whatever the
  opponent played: the opponent is now on turn in the inference position,
  our rack is excluded, the real score is passed, and played blanks are
  sent as `?`.
- Results of an evaluation started for an older position could overwrite
  the current one and swallow the opponent's newly entered tiles; jobs are
  now stamped and cancelled when the position changes.
- Unsimulated candidates showed made-up numbers.
- A single click on the selected suggestion applied it.
- Clicks passed through dialogs; dialog buttons were not clickable.
- Focus could land on hidden panes.
- Stale inference after an opponent pass or exchange.
- A worker-thread panic restored the terminal under the running UI.
- The leave-table parser could allocate from an untrusted count.
- Concurrent RefEngine calls from two handles could crash (SIGSEGV).
