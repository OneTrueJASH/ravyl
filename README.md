# cpsolver2

A terminal UI for analyzing [NYT Crossplay](https://www.nytimes.com/games/crossplay) word game positions.

Built with [ratatui](https://ratatui.rs), cpsolver2 provides real-time move generation, Monte Carlo simulation, and an exhaustive endgame solver — all tuned for Crossplay's rules rather than standard Scrabble.

## Features

- **Interactive 15x15 board** with cursor navigation, tile preview, and premium square rendering
- **Anchor-based move generation** with cross-word validation and FST-backed dictionary lookup
- **RefEngine integration** for Monte Carlo simulation, adaptive game-phase evaluation, and opponent rack inference via FFI to the RefEngine C engine
- **Crossplay-native endgame solver** using exhaustive 2-ply search with no leftover tile penalties (see [Crossplay vs Scrabble](#crossplay-vs-scrabble))
- **Mouse support** with clickable board squares, suggestions, and UI elements
- **Fluid layout** that adapts to any terminal size
- **TOML-persisted settings** with in-app editor (lexicon, threads, display preferences)

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

The endgame difference is the most strategically significant. In Scrabble, emptying your rack is paramount. In Crossplay, the optimal endgame strategy is to maximize your own score while minimizing your opponent's best response — leftover tiles are irrelevant.

## Building

### Prerequisites

- Rust 1.82+
- The RefEngine C library (`librefengine.dylib`) in `external/refengine/`
- RefEngine data files (lexicon, letter distributions, strategy) in `external/refengine/data/`
- FST dictionary file at `data/words.fst`

### Build and run

```bash
cargo build --release
cargo run --release
```

On macOS, the `build.rs` script sets the rpath so the binary can find `librefengine.dylib` relative to the project directory. If you move the binary elsewhere, set `DYLD_LIBRARY_PATH` to point at the directory containing the dylib.

## Usage

### Keyboard shortcuts

| Key | Action |
|-----|--------|
| **Tab** | Cycle focus: Board → Rack → Suggestions |
| **Arrow keys** | Move cursor (board) or selection (suggestions) |
| **A-Z** | Place letter on board or add to rack (depending on focus) |
| **?** | Toggle blank designation on tile at cursor |
| **Backspace** | Remove tile at cursor / from rack |
| **Space** | Toggle typing direction (horizontal/vertical) |
| **S** | Generate all legal moves (solve) |
| **M** | Run RefEngine simulation / endgame solver |
| **Enter** | Apply selected suggestion to the board |
| **O** | Cycle sort mode (Equity → RefEngine → Score → Leave → Length → A-Z) |
| **Ctrl+Z** | Undo last applied move |
| **F1 / H** | Toggle help dialog |
| **F2** | Open settings |
| **Esc** | Open menu (Quit / Reset / Cancel) |
| **L** | Toggle board/logs tab |
| **B** | Toggle sidebar visibility |

### Workflow

1. **Set up the position**: Focus the board (Tab), type letters to place tiles. Press `?` on a tile to mark it as a blank.
2. **Set your rack**: Tab to the rack, type your tile letters (`?` for blanks).
3. **Solve**: Press `S` to generate all legal moves, ranked by static equity.
4. **Simulate**: Press `M` to run RefEngine Monte Carlo simulation (midgame) or the Crossplay endgame solver (when the bag is empty).
5. **Apply**: Select a move with arrow keys and press Enter to apply it to the board.
6. **Repeat**: After your opponent plays, update the board with their tiles and solve again.

## Architecture

See [PRIMER.md](PRIMER.md) for the full module layout and design decisions.

Key architectural choices:
- **No async** — synchronous main loop with `std::thread::spawn` + `mpsc::channel` for background work
- **Centralized theme** — all colors defined in `ui/theme.rs`
- **Thread-local hit map** — UI widgets register clickable regions during render; mouse events resolve against the map
- **Fluid layout** — `AppLayout::compute()` adapts to any terminal size without breakpoints

## Configuration

Settings are stored at `~/.config/cpsolver2/settings.toml` and can be edited in-app (F2):

- **Lexicon**: NWL23 (default) or SOWPODS
- **Thread count**: Number of threads for RefEngine simulation (0 = auto-detect)
- **Display**: Default sort mode, sidebar visibility

## License

[MIT](LICENSE)
