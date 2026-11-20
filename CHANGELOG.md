# Changelog

All notable changes to this project are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- `cargo run` in a checkout starts the app again (the workspace also
  builds the engine's `ravyn` tool, so cargo could not choose).

## [0.1.0] - 2026-11-19

First release: the `ravyl` app and the `ravyn` engine on crates.io, with
prebuilt `ravyl` binaries for macOS and Linux.

### Added
- A terminal UI for Crossplay positions: enter the board, racks and
  scores, and see every legal play ranked by score and leave value.
- Simulation of every candidate (plays, exchanges and passing), racing
  them on spread and win rate, with results shown live.
- Exact endgames, and an exact pre-endgame with one or two tiles in the
  bag.
- Inference of the opponent's rack from their last play.
- Leave values trained by self-play.
- Named games saved as you play, a game picker, and GCG game records:
  export, import and a move-by-move review.
- `ravyl --fetch-data` (or `ravyn fetch-data`) downloads the word list
  and leave values, each checked against a checksum built into the program.
- Themes, mouse support, and layouts down to an 80×24 terminal.
- The engine as a library and a command-line tool, `ravyn`, which also
  plays standard Scrabble (`--rules scrabble`).

## Development history (before 0.1.0)

Changes made while the project was developed, before its first release
(as cpsolver2 until shortly before it). Kept for reference; the entries
are not in strict order.

### Added
- `ravyl --fetch-data` downloads the word list and leave values, so a
  prebuilt `ravyl` needs nothing else. The download code moved from the
  `ravyn` command-line tool into the engine's library (`ravyn::fetch`);
  `ravyn fetch-data` is unchanged.

### Changed
- README: install from crates.io or a prebuilt binary.
- CI builds the documentation with warnings as errors, tests on macOS 26,
  stops jobs after 30 minutes; releases
  build both macOS targets on macOS 26 and contain only `ravyl`.
- `ravyn fetch-data` downloads from the public data repository with no
  token, falling back to the GitHub API with a token for a private copy.
  A token is only ever sent to the GitHub API, never to `$RAVYN_DATA_URL`.
- Both crates are set up for crates.io (RAVYN 0.1.0); issue templates,
  a security policy and Dependabot configuration were added.

### Fixed
- Two-digit counts in the unseen-tile grid ran into the next letter
  (`E:12F:2`).
- The word list now matches Crossplay's: the 182 words Crossplay removes
  from the tournament list (almost all trademarks, such as BRILLO, VELCRO
  and TASER) are gone from `data/words.fst` and the app's other word-list
  files, so they are never suggested or played by the simulated
  opponent. The list is in `data/crossplay-removed.txt` (196,419 words
  remain).

### Changed
- cpsolver2 is now **RAVYL**, RAVYN's companion: the package, the
  `ravyl` binary, and the settings, saved-game and log directories
  (`ravyl/`). On first start, settings and saved games move from the old
  `cpsolver2/` directories; `$RAVYL_DATA_DIR` replaces
  `$CPSOLVER_DATA_DIR`, which is still read; games saved before keep
  loading (their `cpsolver.final` opcodes are read, `ravyl.final` written).
- The word list is no longer in the repository: `ravyn fetch-data`
  installs it (see README).

### Security
- GCG files holding control characters are refused: a terminal escape
  sequence in a player's name or note (say, an OSC 52 clipboard write)
  could otherwise reach the screen through `ravyn gcg` or a pasted import.
- `ravyn fetch-data` checks each file against a SHA-256 checksum built
  into the program, not one fetched with it, and hands its token to curl
  on standard input instead of the command line.
- CI runs with a read-only token and every action pinned to a commit.

### Added
- `ravyn fetch-data`: the data files (word list, leave values) live in
  their own repository, in versioned directories with a `SHA256SUMS`, and
  this downloads the version the engine pins into `~/.ravyn/data` (or
  `--data DIR`), installing nothing whose checksum does not match. The
  engine and cpsolver2 look there after their other data locations. The
  engine's crate package excludes the word list. (Downloads use `curl`;
  checksums are SHA-256, implemented in the CLI.)
- `Dictionary::from_fst_bytes`: load a word list from bytes, without a
  file system.
- The Scrabble endgame searches in parallel: after the previous depth's
  best move, the remaining root moves run across all cores, sharing one
  transposition table. With the earlier speedups (positions one ply from
  the depth limit answered by one bounded search, the best move kept per
  position and tried before generating the rest, board analysis made only
  where it is used), seven of eight self-played 7-tile endgames are solved
  within 20 s, up from four; the ones solved before are 1.8–10.8x faster.
- Exact pre-endgame with two tiles in the bag (Crossplay): every play is
  solved against each way the last two tiles can come out (a play of two
  or more tiles empties the bag; a one-tile play leaves the opponent one to
  draw), within the simulation's time budget, saying whether the best play
  is proven. Passing is not evaluated with two in the bag. On ten
  self-played positions the median solve takes about 3 s and the slowest
  38 s. In the app (M) and `ravyn preendgame` (`--seconds`). The engine
  gains `preendgame::solve_two_in_bag` and `proven`,
  `EndgameSolver::best_value_with` and `Solver::top_moves_by_score`;
  `DrawOutcome::tile` became `tiles` (a string).
- The one-tile pre-endgame got the same machinery (solved best-first with
  per-case abandonment, the pass's cases sharing the opponent's replies)
  and a deadline: the app's simulation budget, `--seconds` on the
  command line. On six self-played positions it is 25–56% faster; the
  slowest went from 79 s to 39 s. `solve_one_in_bag` takes a `deadline`.
- `ravyn match` shows its progress on stderr: games played, time elapsed
  and left, and the running score and spread with their confidence
  intervals (rewritten in place on a terminal, a line a minute when
  redirected). The engine gains `mcts::self_play_match_with_progress`.
- Game records (GCG). The app records every move, ours as applied and the
  opponent's as auto-scored (a turn that puts nothing down is recorded as
  a pass, noted "passed or exchanged"); `G` in the suggestions pane copies
  it as GCG, and it is saved with the game and undone with Undo. Pasting a
  GCG file imports it: its final position and scores, with the side to
  move as you. The engine gains `ravyn::gcg` (read, write, replay and
  review GCG, including exchanges, withdrawn phonies, challenge bonuses
  and end-of-game rack points) and `ravyn gcg FILE`, which reviews a game
  move by move against the best by static equity, or prints any of its
  positions as CGP (`--at N`).
- Named saved games: one file per game (`games/<name>.toml` in the user
  data directory), saved as you play (at most about once a second) and on
  exit, including when the terminal closes (SIGHUP) or the app is killed
  (SIGTERM, SIGINT). Each game keeps its position, scores, final turn and
  the opponent-strength measurement. New games are `Untitled …` until
  named with Save Game As (the title bar shows the name); quitting an
  untitled game offers to name it.
- A game picker (Ctrl+O, the Esc menu, and at startup when there are saved
  games): newest first, with score, tiles in the bag and when each was last
  played; open, new, rename and delete. A game open in one window is locked
  against the others; a lock left by a crash is taken over.
- Esc menu: Open Game, Save Game As, New Game, Reset Board, Quit Game.
- `--game <NAME>` opens or creates a game; `--list` prints the saved games.
- A `session.cgp` from earlier versions is imported as the game "Restored
  session".
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
- CGP paste, and copying the selected move
  or the position to the clipboard (OSC 52).
- Themes (dark, light, ansi16, no-color) chosen by `display.theme`,
  honoring `NO_COLOR` and `COLORTERM`, with ASCII fallbacks.
- Stacked layout for narrow terminals and a "terminal too small" message.
- A data-directory search order (`--data-dir`, `$CPSOLVER_DATA_DIR`, the
  user data directory, `<exe>/../share/cpsolver2`, the working directory)
  and `--help` / `--version`.
- Criterion benchmarks, a self-play strength harness
  (`examples/selfplay.rs`), property tests, a random-position move
  generation comparison with a reference engine, and cargo-fuzz targets.
- CI on Linux and macOS, a build without the reference engine, an MSRV check, and
  cargo-deny; a release workflow (dist) that runs only by hand.

### Changed
- The engine (move generation, leaves, simulation, endgame) is now its
  own crate, RAVYN, in `crates/ravyn`, MIT licensed and free of the reference engine.
  It logs through the `log` facade; the app forwards those lines to its
  log. The app re-exports the engine's modules at their old paths.
- The game's rules are data: board layout, start square, letter values,
  tile distribution, bingo bonus and ending live in `ravyn::rules::Rules`,
  carried by every board, with `CROSSPLAY` as the preset. Scoring, move
  generation, tile counts and leave values read them from the position
  instead of constants (move generation measured 2-6% faster).
- Simulation keeps every core busy: jobs run as a continuous pipeline
  instead of rounds that waited for their slowest job (workers had been
  idle about 30% of the time). Simulating every play of a 40,503-play
  position went from 4,478 to 5,497 playouts a second, and twice as many
  plays (3,274 against 1,692) reached the playout floor in 12 seconds.
  Each playout is still the same deal in every run.
- The interface stays responsive while tens of thousands of plays are
  simulated: on a 40,504-play list a live update now takes 4.6 ms instead
  of 32 ms, a redraw 0.28 ms instead of 3.6 ms, and a filtered redraw
  1.3 ms instead of 8.4 ms. Sorting pairs each play with its result once,
  the table reuses that pairing, and the filter no longer builds strings.
- cpsolver2 is now MIT licensed, like its engine. It was GPL-3.0 because
  it built on a GPL-3.0 reference engine, none of which remains.
- Our own leave values, trained by self-play: `ravyn train-leaves` plays
  greedy self-play games, fits every leave's value from what keeping it
  was worth a turn later (with a smooth model for rare leaves) and repeats
  with the new values. Three generations of 500,000 games produced
  `data/leaves.rlv` (all 921,298 leaves, 1.8 MB), which beat the
  per-letter heuristic by 16.3 +- 2.4 points of spread per game over
  20,000 paired games.
  `ravyn match` compares leave tables (`--sim C,P` with both sides
  choosing moves by simulation, where the gain held: +2.2 +- 2.0 over
  4,600 games).
- The last of the reference engine is gone: its leave table, that table's
  reader and Git LFS (nothing is stored in LFS any more). Only a record of
  its move generation remains, as test fixtures.
- Scrabble rules in the engine: a `SCRABBLE` preset (standard board,
  values and tiles, 50-point bingo) and the go-out ending. Playouts play
  the bag-empty finish until someone goes out (twice the opponent's rack)
  or six scoreless turns pass; `ravyn::negamax` solves the endgame
  exactly (negamax with alpha-beta, a transposition table and iterative
  deepening), reporting an estimate when the time budget runs out.
  `ravyn --rules scrabble` uses all of it.
- The board analysis (anchors and cross-checks) can be updated after a
  move instead of recomputed; the endgame search does so at each node.
- A command-line tool for the engine, `ravyn`, with commands
  on CGP positions: `gen` (moves by static equity), `sim` (simulation with
  spread and win%), `endgame`, `preendgame` and `infer`. Scores and a
  known opponent rack are read from the CGP; `--data` or `$RAVYN_DATA`
  locate the data files.
- The reference engine is out of the repository: its source submodule,
  the build script, the C bindings, its cargo feature and its unused data files
  (lexicon, layouts, letter distribution) are gone, so cloning needs no
  submodule and building no C compiler. Its answers on the reference
  positions (move generation on 32 positions, static equity, rack
  inference) are recorded as fixtures, and the engine is still checked
  against them. Only its leave table remains, as data.
- The engine's integration, property and reference-engine tests and its
  fuzz targets moved into `crates/ravyn` with the benchmarks.
- Exact pre-endgame with one tile in the bag: M solves every play against
  each tile that could be the last one (the opponent's best turn, then
  our best final play, with perfect information), giving each play's
  exact win probability and expected spread. Plays are capped cheaply
  first and solved best-first, so a rack with a blank (7,354 plays) takes
  about 9 s and most racks under a second; the best play is always
  solved exactly (a test checks it against solving every play). Passing,
  which leaves the bag for the opponent, is solved too: each of their
  replies, then our turn, then their last play (their passing back
  counts as ending the game, so a pass is never overrated).
- Best-play searches prune by anchor: each anchor's plays are bounded
  (the rack's most valuable tiles on the most multiplied squares, plus the
  best leave of that size), anchors are searched best-first, and the
  search stops when no remaining anchor can change the answer. Playouts
  run 1.4-2.3x faster, the endgame solver 1.6x, and simulating every play
  of a 40,503-play position went from 5,497 to 9,700 playouts a second.
  Equal plays now follow one canonical order in every search and in the
  move list. A test checks the pruned searches against the full list on
  160 random racks.
- Opponent rack inference is the engine's own (`ravyn::inference`)
  instead of the reference engine's. Every leave the opponent could have kept is tried
  and kept if their play was within 5 points of that rack's best, weighted
  by how likely it was to be drawn; it agrees with the reference engine to within 0.06
  tiles per letter (a test checks it) and is as fast or faster. The app
  no longer starts the reference engine (68 ms less at launch), builds
  without it now infer too, and its cargo feature is off by default: it only
  serves as a reference in tests, so building needs no C compiler.
- Works on an 80×24 terminal: from 80 columns a mini board keeps a
  compact sidebar beside it (rack, score and bag, the sort row and the
  move list). Before, only the board was drawn and Tab focused panes that
  were not on screen. Between 80 and 111 columns this also replaces the
  stacked layout, whose list had four rows at 100×30.
- The stats pane labels every line (`You 42 – Opp 57`, `3 vowels ·
  3 consonants · 1 blank · 6 pts`, `Bag ~77 · Early · unseen ?1 S4`,
  `Final move: opponent? (assumed, ^E to change)`, power tiles grouped
  by where they are, the unseen grid under a heading with its total), is
  as tall as its content so the list gets the rest, and no longer goes
  blank while the rack is empty.
- During a simulation the selection stays on the leader until you move
  it, instead of following an early leader down the list.
- The sorted column is marked in the table header and the sort row is
  labelled; a candidate dropped from the race reads `out @n` (was
  `stopped @n`), and the title says `· simulated` (was `*`).
- Help (F1) explains the table columns and the stats pane, and scrolls
  at every size with a hint while there is more below.
- With no moves yet, the list says how to start; the hint bar always
  keeps F1 Help; dialogs fit their text (the quit prompt's Esc hint, the
  score prompt, and confirmations naming a long game).
- Licensed under GPL-3.0-or-later (was MIT), since the binary links
  a GPL-3.0 reference engine; copyright holder corrected.
- `--data-dir` now takes precedence over `$CPSOLVER_DATA_DIR`.
- The reference engine is compiled from vendored source (a pinned
  submodule) and linked statically, behind a default cargo feature;
  without it the
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
- Concurrent reference-engine calls from two handles could crash.
