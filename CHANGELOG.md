# Changelog

All notable changes to this project are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- Two-digit counts in the unseen-tile grid ran into the next letter
  (`E:12F:2`).
- The word list now matches Crossplay's: the 182 words Crossplay removes
  from the tournament list (almost all trademarks, such as BRILLO, VELCRO
  and TASER) are gone from `data/words.fst`, `AMERICAN.kwg` and
  `AMERICAN.wmp`, so they are never suggested or played by the simulated
  opponent. The list is in `data/crossplay-removed.txt` (196,419 words
  remain).

### Added
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
  generation comparison with RefEngine, and cargo-fuzz targets.
- CI on Linux and macOS, a no-RefEngine build, an MSRV check, and
  cargo-deny; a release workflow (dist) that runs only by hand.

### Changed
- The engine (move generation, leaves, simulation, endgame) is now its
  own crate, RAVYN, in `crates/ravyn`, MIT licensed and free of RefEngine.
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
