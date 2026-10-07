# cpsolver2

A terminal UI for analyzing [NYT Crossplay](https://www.nytimes.com/games/crossplay) word game positions.

Built with [ratatui](https://ratatui.rs), cpsolver2 provides real-time move generation, Monte Carlo simulation, and an exhaustive endgame solver — all tuned for Crossplay's rules rather than standard Scrabble.

## Features

- **Interactive 15x15 board** with cursor navigation, tile preview, and premium square rendering
- **Anchor-based move generation** with cross-word validation and FST-backed dictionary lookup
- **Crossplay-rules Monte Carlo playouts** for every phase of the game, including exchanges, with expected final spread, win probability and bingo rates per candidate (see [Crossplay vs Scrabble](#crossplay-vs-scrabble))
- **Crossplay-native endgame solver** using exhaustive 2-ply search (including passing) with no leftover tile penalties
- **Move generation checked against RefEngine** play for play, with leave values from RefEngine's fitted leave table corrected for the 40-point bingo
- **Opponent modeling**: rack inference via FFI to the RefEngine C engine biases how the opponent's hidden tiles are dealt in playouts, and their measured error rate (each scored move compared against their best alternatives) sets how strongly the simulated opponent plays
- **Mouse support** with clickable board squares, suggestions (double-click applies), dialog buttons and menu options
- **Fluid layout** that adapts to any terminal size, down to a borderless grid on small terminals
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

The endgame difference is the most strategically significant. In Scrabble, emptying your rack is paramount. In Crossplay, the player who draws the last tile gets the last word: the other player takes one more turn, then the bag-emptier plays the final move, and the game ends even if both still hold tiles. So when the bag is empty on your turn, the opponent only gets a reply if *they* emptied the bag. The solver tracks this (`last:YOU`/`last:OPP` in the stats pane, Ctrl+E to override for positions entered by hand): with a reply coming it searches every play, and passing, against the opponent's best response; without one it ranks by score.

With tiles still in the bag the app plays each candidate out many times under these rules (both sides may exchange while the bag holds 7 or more tiles), reports the expected final spread, and estimates the win probability from a variance model fitted on Crossplay self-play. RefEngine's own simulation is not used for ranking because it plays the ending under Scrabble rules; it only infers what the opponent probably kept, and those per-letter probabilities (shown in the stats pane as `Opp kept:`) tilt the deal of the opponent's hidden rack inside the playouts. The app also grades every opponent move it auto-scores against the best plays available from sampled completions of their rack; the running error rate (shown as `Opp err:`, calibrated on self-play so a perfect opponent reads zero) makes the simulated opponent in playouts err at the same rate instead of always punishing maximally.

## Building

### Prerequisites

- Rust 1.88+ and a C compiler (RefEngine is compiled from source by `build.rs`)
- Git LFS: the lexicon files in `external/refengine/data/lexica/` are stored in LFS
- The RefEngine source submodule in `external/refengine-src/`
- FST dictionary file at `data/words.fst`

```bash
git clone --recurse-submodules https://github.com/OneTrueJASH/cpsolver2
cd cpsolver2
git lfs install && git lfs pull   # if the NWL23.* files are ~130-byte pointer stubs
```

`NWL23.klv2` also supplies the leave values; a per-letter heuristic is used if it is missing or still an LFS pointer.

### Build and run

```bash
cargo build --release
cargo run --release
```

RefEngine is linked statically, so the binary has no runtime library dependency. Run it from the repository root (it looks for `data/` and `external/refengine/data/` next to the executable or in the working directory).

### Building without RefEngine

RefEngine sits behind the `refengine` cargo feature (on by default). Without it the pure-Rust solver, playouts and endgame solver work as usual; only opponent-rack inference is unavailable (the stats pane says so):

```bash
cargo build --release --no-default-features
```

## Usage

### Keyboard shortcuts

| Key | Action |
|-----|--------|
| **Tab** | Cycle focus: Board → Rack → Suggestions |
| **Arrow keys** | Move cursor (board) or selection (suggestions) |
| **A-Z** | Place letter on board or add to rack (depending on focus) |
| **?** | Toggle blank designation on tile at cursor |
| **Backspace** | Remove tile at cursor / from rack |
| **Delete** | Clear entire rack (when rack is focused) |
| **Space** | Toggle typing direction (horizontal/vertical) |
| **Ctrl+S** | Generate all legal moves (solve) and focus suggestions |
| **Enter** | Solve (from rack) or apply selected suggestion (from suggestions) |
| **M** | Evaluate: Crossplay-rules playouts, or the endgame solver when the bag is empty (from suggestions) |
| **S** | Cycle sort mode (from suggestions: Equity → Sim → Win% → Score → Leave → Length → A-Z) |
| **Ctrl+Z** | Undo last applied move |
| **Ctrl+R** | Reset board |
| **Ctrl+E** | Toggle who has the final turn (you / opponent) once the bag is empty |
| **F1** | Toggle help dialog |
| **Esc** | Open menu (Quit / Reset / Cancel) or return focus to board |
| **`** (backtick) | Toggle board/logs tab |
| **\\** (backslash) | Toggle sidebar visibility |

### Workflow

1. **Set up the position**: Focus the board (Tab), type letters to place tiles. Press `?` on a tile to mark it as a blank.
2. **Set your rack**: Tab to the rack, type your tile letters (`?` for blanks).
3. **Solve**: Press `Ctrl+S` (or Enter from rack) to generate all legal moves, ranked by static equity.
4. **Simulate**: Press `M`. With tiles in the bag this plays the candidates out under Crossplay rules (deeper as the bag empties); with the bag empty it runs the endgame solver (or, if your move ends the game, sorts by score). Check the `last:` indicator in the stats pane — press Ctrl+E if the app has the wrong idea about who emptied the bag. Exchanges appear as `EXCH` rows; applying one clears those tiles so you can type the replacements.
5. **Apply**: Select a move with arrow keys and press Enter (or double-click it) to apply it to the board. Changing the position while an evaluation runs cancels it, so results never land on a different position.
6. **Repeat**: After your opponent plays, update the board with their tiles and solve again.

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

Missing keys take their defaults. If the file cannot be parsed, it is moved aside to `settings.toml.bak`, the defaults are used, and the message bar says so.

### Logs

Each run logs to `cpsolver2-<pid>.log` in `~/.local/state/cpsolver2/` on Linux, or `~/Library/Caches/cpsolver2/` on macOS. The file is created with mode 0600. Logs older than a week are pruned at startup, and at most 20 are kept. The Logs tab (`` ` ``) shows the current solve's lines from memory.

## License

[MIT](LICENSE)
