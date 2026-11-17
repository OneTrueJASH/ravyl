# cpsolver2

A terminal UI for analyzing [NYT Crossplay](https://www.nytimes.com/games/crossplay) word game positions.

Built with [ratatui](https://ratatui.rs), cpsolver2 provides real-time move generation, Monte Carlo simulation, and an exhaustive endgame solver — all tuned for Crossplay's rules rather than standard Scrabble.

## Features

- **Interactive 15x15 board** with cursor navigation, tile preview, and premium square rendering
- **Anchor-based move generation** with cross-word validation and FST-backed dictionary lookup
- **Simulates every candidate**: every legal play, every distinct exchange and passing are played out under Crossplay rules, with an anytime scheduler (every candidate gets a playout floor, then the contenders race) that streams results into the table live. For each candidate the table shows playouts, mean final spread ± standard error, win% ± standard error, the paired difference to the best candidate, bingo rates and a status (see [Crossplay vs Scrabble](#crossplay-vs-scrabble))
- **Crossplay-native endgame solver** using exhaustive 2-ply search (including passing) with no leftover tile penalties
- **Move generation checked against RefEngine** play for play (its answers on 32 reference positions are kept as test fixtures), with leave values from RefEngine's fitted leave table corrected for the 40-point bingo
- **Opponent modeling**: rack inference (what the opponent probably kept, given their play; checked against RefEngine's) biases how the opponent's hidden tiles are dealt in playouts, and their measured error rate (each scored move compared against their best alternatives) sets how strongly the simulated opponent plays
- **Mouse support** with clickable board squares, suggestions (double-click applies), dialog buttons and menu options
- **Fluid layout** that adapts to any terminal size: board and sidebar side by side; from 80 columns (the classic 80×24 included) a mini board with a compact sidebar of rack, score and the move list; a stacked board, one-line rack and short list on narrower but tall terminals; and a clear message below the minimum size (50×22)
- **Preferences persisted to TOML** (sort mode, sidebar visibility)

## Crossplay vs Scrabble

Crossplay differs from Scrabble in several important ways that affect optimal strategy:

| Rule | Scrabble | Crossplay |
|------|----------|-----------|
| Tile count | 104 (2 blanks, 4 S's) | 100 (3 blanks, 5 S's) |
| Bingo bonus | 50 points | 40 points |
| Premium layout | Standard | Unique to Crossplay |
| Letter values | Standard | Different (e.g., K=6, V=6, W=5) |
| Endgame | Play until one player empties rack; leftover tiles penalized and added to opponent | **Equal Turn Rule**: bag empty triggers one final turn each; no leftover tile penalty |
| Game end | Player who goes out gets opponent's rack value | Highest cumulative score wins, no bonus |

The endgame difference is the most strategically significant. In Scrabble, emptying your rack is paramount. In Crossplay, the player who draws the last tile gets the last word: the other player takes one more turn, then the bag-emptier plays the final move, and the game ends even if both still hold tiles. So when the bag is empty on your turn, the opponent only gets a reply if *they* emptied the bag. The solver tracks this (`Final move: you`/`opponent` in the stats pane, with `?` while it is only assumed; Ctrl+E overrides it for positions entered by hand): with a reply coming it searches every play, and passing, against the opponent's best response; without one it ranks by score.

With exactly one tile in the bag, every play empties it, so the app solves the rest of the game exactly instead: for each play and each tile that could be the last one, it finds the opponent's best turn given your best final play, and reports each play's exact win probability and expected spread over the possible last tiles (shown as exact). Passing, which leaves the bag for the opponent to empty, is solved the same way; if they pass back, that is counted as ending the game, which can only make passing look worse, never better.

With more tiles in the bag the app plays each candidate out many times under these rules (both sides may exchange while the bag holds 7 or more tiles), reports the expected final spread, and estimates the win probability from a variance model fitted on Crossplay self-play. After each opponent play the app also infers what they probably kept: every leave they could have held is tried, and kept if their play was within 5 points of equity of that rack's best play, weighted by how likely it was to be drawn. Those per-letter probabilities (shown in the stats pane as `Opp kept:`) tilt the deal of the opponent's hidden rack inside the playouts. The app also grades every opponent move it auto-scores against the best plays available from sampled completions of their rack; the running error rate (shown as `Opp error:`, calibrated on self-play so a perfect opponent reads zero) makes the simulated opponent in playouts err at the same rate instead of always punishing maximally.

## Building

### Prerequisites

- Rust 1.88+
- Git LFS: the leave table `external/refengine/data/lexica/AMERICAN.klv2` is stored in LFS
- FST dictionary file at `data/words.fst`

```bash
git clone https://github.com/OneTrueJASH/cpsolver2
cd cpsolver2
git lfs install && git lfs pull   # if the leave table is a ~130-byte pointer stub
```

`AMERICAN.klv2` supplies the leave values; a per-letter heuristic is used if it is missing or still an LFS pointer.

The word list is Crossplay's: 196,419 words, the 2023 American tournament list less the 182 words Crossplay removed (almost all trademarks, such as BRILLO and VELCRO). The removed words are listed in `data/crossplay-removed.txt`. `data/words.fst` is built without them; `AMERICAN.klv2` holds leave values only and is unchanged.

### Build and run

```bash
cargo build --release
cargo run --release
```

The binary is pure Rust with no runtime library dependency. It needs the data files at run time and looks for a directory holding `data/words.fst` and `external/refengine/data/` in this order:

1. `--data-dir <DIR>`
2. `$CPSOLVER_DATA_DIR`
3. the user data directory: `~/.local/share/cpsolver2` on Linux, `~/Library/Application Support/cpsolver2` on macOS
4. `<exe>/../share/cpsolver2`
5. the working directory (running from a checkout)

```text
cpsolver2 [--game <NAME>] [--data-dir <DIR>]
cpsolver2 --list   cpsolver2 --help   cpsolver2 --version
```

`--game <NAME>` opens the saved game called NAME (matched by its file name, so case and punctuation do not matter) or starts a new game with that name; `--list` prints the saved games (name, score, tiles in the bag, last played) and exits.

### Benchmarks

```bash
cargo bench --bench engine
```

Criterion benchmarks for move generation (EMPTY, MID, END and blank-heavy BEECH positions), playouts per second for a fixed candidate set, and the endgame solver.

### Self-play harness

```bash
cargo run --release --example selfplay -- <phase|pass|sim|same> [pairs] [seed]
```

Plays two engine variants against each other over paired, fixed-seed games (each seed twice, with the racks swapped) and reports engine A's score and mean spread with 95% confidence intervals.

## Usage

### Keyboard shortcuts

Everywhere:

| Key | Action |
|-----|--------|
| **Tab** / **Shift+Tab** | Next / previous pane (Board → Rack → Suggestions; hidden panes are skipped) |
| **Ctrl+S** | Find all moves (solve) and start simulating |
| **Ctrl+Z** / **Ctrl+Y** | Undo / redo (applied moves, auto-scored opponent moves, score edits, reset, clearing the rack, entered plays) |
| **Ctrl+R** | Reset the board (undoable) |
| **Ctrl+E** | Toggle who has the final turn (you / opponent) once the bag is empty |
| **:** | Enter a play by coordinates, e.g. `8D QUIXOTIC` across or `H4 ZAX` down (lowercase letters are blanks); the prompt checks it and previews the score |
| **=** / **+** | Set the opponent's / your score |
| **Esc** | In order: stop a running simulation, close a dialog, clear the list filter, return to the board, open the menu |
| **`** (backtick) | Board / log tab |
| **\\** (backslash) | Toggle the sidebar |
| **F1** | Help (two columns on wide terminals, scrollable on narrow ones) |
| **Ctrl+O** | Saved games (the game picker) |
| **Ctrl+C** | Quit (asks first; Ctrl+C again confirms). An untitled game asks for a name instead: Enter saves it and quits, Ctrl+C again quits keeping the automatic name, Esc cancels |

Board:

| Key | Action |
|-----|--------|
| **Arrows** | Move the cursor (the title shows the square and direction, e.g. `H8 →`) |
| **a–z** | Place a letter; the cursor then skips squares that already hold tiles, so typing only a word's new tiles fills the gaps. Typing the letter a square already shows keeps that tile |
| **Shift+A–Z** | Place a blank as that letter |
| **?** | Toggle the blank flag of the tile under the cursor (or place a blank) |
| **Space** | Typing direction across / down |
| **Backspace** / **Delete** | Delete here or step back / remove the tile at the cursor |
| **Enter** | Solve |

Rack: **A–Z** and **?** add tiles, **Backspace** removes the last, **Delete** clears the rack (undoable), **Enter** solves.

Suggestions:

| Key | Action |
|-----|--------|
| **↑/↓**, **j/k** | Select |
| **PgUp/PgDn**, **Home/End** | Page, first / last |
| **1–9** | Select that visible row |
| **/** | Filter by word or square as you type (Enter keeps it, Esc clears it); **n/N** next / previous row |
| **Enter** | Apply the selected move (also a double-click); after an exchange the rack gets the focus |
| **M** (or **m**) | Simulate every candidate (anytime, results stream in; the endgame solver when the bag is empty). M again on the same position continues the stopped or finished run |
| **s** / **S** | Cycle sort mode (Equity → Sim → Win% → n → Bingo → Score → Leave → Length → A-Z); clicking a column header sorts by it |
| **y** / **Y** | Copy the selected move (`8D QUIXOTIC`) / the whole position (CGP) to the clipboard (OSC 52) |

Esc menu: **Open Game…** (the picker), **Save Game As…** (name or rename the game), **New Game**, **Reset Board**, **Quit Game**, **Cancel**.

Game picker (Ctrl+O, the Esc menu, or at startup):

| Key | Action |
|-----|--------|
| **↑/↓**, **j/k**, **PgUp/PgDn**, **Home/End** | Select (also the wheel) |
| **Enter** | Open the game (also a double-click) |
| **n** | Start a new, untitled game |
| **r** | Rename the selected game |
| **d** | Delete the selected game (asks first) |
| **Esc** | Close (at startup: start a new game) |

Mouse: click to focus a pane, square or row; double-click a suggestion to apply it; click a column header or sort tab to sort; the wheel scrolls the pane under the pointer. Pasting a CGP line (bracketed paste) loads that position.

### Saved games

Every game is saved as you play, one file per game in `games/` under the user data directory (`~/.local/share/cpsolver2/games/` on Linux, `~/Library/Application Support/cpsolver2/games/` on macOS): the position, rack, scores, who has the final turn, and the opponent-strength measurement. Saves happen at most about once a second while the position changes, and on exit, including when the terminal window is closed (SIGHUP) or the app is killed with SIGTERM or SIGINT.

A new game is called `Untitled YYYY-MM-DD HH:MM` until you name it (Esc menu → Save Game As…, usually with the opponent's name, which the title bar then shows). Saving under the name of another saved game asks before replacing it. Quitting an untitled game offers to name it first, but never insists.

Several games can be open at once, each in its own terminal window, but a game is open in only one window at a time: the window holds a lock (`<name>.lock`, with its process id), and the others refuse to open it ("… is open in another window"; dimmed in the picker). A lock left behind by a crash is taken over.

At startup the app lists the saved games to pick from (unless `--game` is given, or none can be opened). A `session.cgp` written by an earlier version is imported at startup as the game "Restored session" (or "Restored session (2)", …) and renamed to `session.cgp.imported`; this also catches windows still running an older version that save their session when they close.

Ctrl+R clears the board of the open game (and can be undone); New Game starts another game and keeps this one.

### Workflow

1. **Set up the position**: Focus the board (Tab) and type letters to place tiles (Shift+letter for a blank), or press `:` and type a play like `8D QUIXOTIC`, or paste a CGP line.
2. **Set your rack**: Tab to the rack, type your tile letters (`?` for blanks).
3. **Solve**: Press `Ctrl+S` (or Enter from rack) to generate all legal moves, ranked by static equity.
4. **Simulate**: Press `M`. With tiles in the bag this simulates every candidate (plays, exchanges and pass) under Crossplay rules, deeper as the bag empties: each gets a floor of playouts, then racing keeps sampling only the candidates that could still be best on spread or on win%. The table updates live and the header shows coverage (`simulated 1,840/12,300 plays · 48,200 playouts · 12.4s`). Esc stops; M continues. With the bag empty it runs the exact endgame solver (or, if your move ends the game, sorts by score). Check the `last:` indicator in the stats pane — press Ctrl+E if the app has the wrong idea about who emptied the bag. Exchanges appear as `EXCH` rows; applying one clears those tiles so you can type the replacements.
5. **Apply**: Select a move with arrow keys and press Enter (or double-click it) to apply it to the board. Changing the position while an evaluation runs cancels it, so results never land on a different position.
6. **Repeat**: After your opponent plays, put their tiles on the board (typing only the new tiles, or `:` with the full play) and solve again; their move is scored automatically.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the checks to run before pushing, the tests and studies, benchmarks and fuzzing, and [CHANGELOG.md](CHANGELOG.md) for what changed.

## Architecture

See [PRIMER.md](PRIMER.md) for the full module layout and design decisions.

Key architectural choices:
- **No async** — synchronous main loop with `std::thread::spawn` + `mpsc::channel` for background work
- **Centralized theme** — all colors defined in `ui/theme.rs`
- **Thread-local hit map** — UI widgets register clickable regions during render; mouse events resolve against the map
- **Fluid layout** — `AppLayout::compute()` adapts to any terminal size without breakpoints

## Configuration

Preferences are automatically saved to `~/.config/cpsolver2/settings.toml` (written atomically):

- **Default sort** — persisted when you cycle sort mode with `S` in the suggestions pane or click a sort tab
- **Sidebar visibility** — persisted when you toggle with `\`
- **Theme** — `display.theme`: `auto` (the default: no colors when `NO_COLOR` is set, the dark truecolor theme when `COLORTERM` says truecolor, the 16-color theme otherwise), `dark`, `light`, `ansi16` or `nocolor`. Outside a UTF-8 locale the ★ and ♦ marks become `*` and `+`

`[simulation]` sets `min_playouts` (the playout floor every candidate gets, default 16) and `budget_secs` (how long one run may take, default 20).

Missing keys take their defaults. If the file cannot be parsed, it is moved aside to `settings.toml.bak`, the defaults are used, and the message bar says so.

### Logs

Each run logs to `cpsolver2-<pid>.log` in `~/.local/state/cpsolver2/` on Linux, or `~/Library/Caches/cpsolver2/` on macOS. The file is created with mode 0600. Logs older than a week are pruned at startup, and at most 20 are kept. The Logs tab (`` ` ``) shows the current solve's lines from memory.

## License

Copyright (C) 2026 OneTrueJASH.

cpsolver2 is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. See [LICENSE](LICENSE).

The leave table it reads (`external/refengine/data/lexica/AMERICAN.klv2`) is a data file from RefEngine (GPL-3.0); see [`external/refengine/NOTICE`](external/refengine/NOTICE). No RefEngine code is part of the repository. Anyone you give a binary to is entitled to the matching source.

The engine in `crates/ravyn` (RAVYN) is separately licensed under the [MIT License](crates/ravyn/LICENSE).
