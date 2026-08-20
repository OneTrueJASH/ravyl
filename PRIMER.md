# cpsolver2 — Project Primer

Complete TUI redesign of `crossplay-solver`.

## Module Layout

| Module | Purpose |
|---|---|
| `tile` | `Tile` struct, `Rack` (7-slot), `letter_value()` with Crossplay scoring; constants: `RACK_SIZE` |
| `board` | 15x15 `Board`, `Cell`, `Premium` enum, `Direction` enum, Crossplay premium layout; constants: `BOARD_SIZE` |
| `dictionary` | FST-backed `Dictionary`: node-level walking (`root()`, `child()`) for the move generator, `leave_value()` (fitted table or heuristic, phase-scaled), `load_default_dictionary()` which also attaches the leave table when `external/refengine/data/lexica/NWL23.klv2` is present |
| `leaves` | `LeaveValues` — parser and lookup for RefEngine `.klv2` leave tables (DAWG + one value per 1–6 tile leave) |
| `game` | `SolverState` (Board + Rack), tile distribution (100 tiles), `unseen_tiles()`, endgame detection, `from_cgp()`; constants: `TOTAL_TILES`, `BLANK_COUNT` |
| `solver` | `Move` (with `coords()` → `8D`/`D8`), `ExternalMoveData`, anchor-based move generation over FST nodes with bitmask cross-checks and incremental scoring, `leave_value_with_phase()` (per-letter fallback), `SolverConfig`, `Solver`; constants: `BINGO_BONUS` (40). Verified play-for-play against RefEngine in `test_movegen_matches_refengine` |
| `stats` | `GameStats`, `GamePhase`, `TileLocation`, power tile tracking, rack analysis |
| `settings` | `Settings` with `DisplaySettings` (`default_sort`, `sidebar_visible`), TOML persistence at `~/.config/cpsolver2/settings.toml`; constant: `LEXICON = "NWL23"` |
| `logging` | Instance-specific logging to `/tmp/crossplay-<pid>.log` |
| `refengine_ffi` | FFI bindings to RefEngine C library (`librefengine.dylib`), `RefEngine` safe wrapper |
| `refengine_mcts` | `RefSimulator`, `RefSimConfig`, `RefSimResult`, `InferenceResult`, CGP conversion, adaptive simulation based on game phase. Configured with `-bb 40` at start-up so every simulated ply scores Crossplay bingos. Used only with 8+ tiles in the bag |
| `mcts` | `MctsEvaluator`, `MctsResult`, `MctsConfig`, `TilePool` — Monte Carlo playouts under Crossplay's equal-turn ending (exact 2-ply on the last turn, score-only final move, no leftover penalties). The pre-endgame engine (1–7 tiles in the bag) and the fallback when RefEngine is unavailable; common random numbers across candidates, paired-difference pruning, Rayon parallelism |
| `endgame` | `EndgameSolver`, `EndgameMove`, `PvMove`, `PASS_WORD` — Crossplay-native endgame solver: exact 2-ply (including passing) when the opponent replies, score-only when our move ends the game |
| `app` | `App` (central state), `Focus`, `SortMode`, `FinalTurn` (who plays the last move once the bag is empty), `DialogState`, `BoardTab`, `handle_key()`/`handle_mouse()` event handlers, undo support, engine routing by game phase |
| `ui::theme` | Centralized color/style constants — all UI colors defined here |
| `ui::hit` | Thread-local hit map for mouse click resolution (`HitTarget`, `register_hit()`, `resolve_click()`) |
| `ui::events` | `AppAction` enum (`Quit`, `Redraw`, `Nothing`) |
| `ui::layout` | Fluid `AppLayout` computed from terminal size (no breakpoints); the board wins over the sidebar when both cannot fit |
| `ui::board_view` | Board widget with premium squares, cursor, preview tiles; `BoardGeometry` picks full (4-row cells), compact (3-row) or borderless mini cells to fit the pane |
| `ui::stats_view` | Rack tiles + game stats + opponent inference widget |
| `ui::suggestions` | Suggestions table (position, word, score, equity, simulation columns, leave) with sort-mode tabs and simulation lookup |
| `ui::log_viewer` | Tab bar + log viewer |
| `ui::status_bar` | Title bar, message bar, context-sensitive hint bar |
| `ui::popup_frame` | Shared popup rendering (clear + border + title) |
| `ui::help_popup` | Help dialog with keyboard shortcuts |
| `ui::confirm_popup` | Yes/No confirmation dialog |
| `ui::menu_popup` | Escape menu (Quit/Reset/Cancel) |

## Key Design Differences from crossplay-solver

1. **Mouse support** — thread-local hit map populated during render, resolved on click
2. **Centralized theme** — all colors in `ui::theme`, no inline `Color::Rgb(...)` in widgets
3. **Fluid layout** — `AppLayout::compute()` replaces the 4-mode `LayoutMode` enum
4. **Event handlers in App** — `handle_key()`, `handle_mouse()` methods on `App` instead of free functions in `main.rs`
5. **Shared popup frame** — `popup_frame::render_popup_frame()` used by all dialogs
6. **Clean event loop** — main loop dispatches `Event::Key` / `Event::Mouse` to `App`, which returns `AppAction`
7. **Settings auto-persisted via keybindings** — no settings dialog; sort mode and sidebar visibility are saved on toggle
8. **Native endgame solver** — `endgame.rs` implements Crossplay-specific 2-ply endgame (equal turn rule, no tile penalties)
9. **Engine routing by phase** — `App::run_mcts_evaluation`: our move ends the game → sort by score; bag empty and the opponent replies → `EndgameSolver`; 1–7 tiles in the bag → native playout (`mcts.rs`); otherwise RefEngine simulation. RefEngine's own endgame solver applies Scrabble rules and is never used for ranking
10. **Leave values from RefEngine's table** — static equity reads `NWL23.klv2` (fitted for standard Scrabble; blank/bingo-heavy leaves are somewhat overvalued for Crossplay) and falls back to the per-letter heuristic in `solver.rs` when the file is missing. Leave weight is scaled by phase: ×0.6 with one draw left, ×0.25 once the bag is empty

## Named Constants

| Constant | Module | Value | Meaning |
|---|---|---|---|
| `BOARD_SIZE` | `board` | 15 | Side length of the square board |
| `RACK_SIZE` | `tile` | 7 | Number of tile slots in a player's rack |
| `TOTAL_TILES` | `game` | 100 | Total tiles in the Crossplay distribution |
| `BLANK_COUNT` | `game` | 3 | Number of blank (wildcard) tiles |
| `BINGO_BONUS` | `solver` | 40 | Bonus points for using all 7 rack tiles |
| `LEXICON` | `settings` | `"NWL23"` | Fixed lexicon used by Crossplay |

## Settings

Settings are stored in `~/.config/cpsolver2/settings.toml` and auto-persisted when changed via keybindings (no settings dialog).

The `Settings` struct contains only `DisplaySettings`:
- `default_sort` — default sort mode for suggestions (`Equity`, `Mcts`, `Score`, `Leave`, `Length`, `Alpha`)
- `sidebar_visible` — whether the sidebar is shown by default

## Data

- `external/refengine/` — RefEngine C library + lexicon/strategy data files
- `data/` — symlink to reference project's FST dictionary files

## Crossplay-Specific Rules

- 100 tiles (3 blanks, 5 S's)
- 40-point bingo bonus (not 50)
- Unique premium square layout (not standard Scrabble); `external/refengine/data/layouts/standard15.txt` and `letterdistributions/english.csv` have been customised to match `board.rs` and `tile.rs`
- Endgame: when a player draws the last tile from the bag, the *other* player gets one last turn and then the player who emptied the bag plays the final move (equal turn rule). The game then ends even if both still hold tiles
- No leftover tile penalty — unplayed rack tiles don't affect scoring
- No go-out bonus — emptying your rack gives no extra points
- Consequence for the solver: with the bag empty, the opponent only replies to our play if *they* emptied the bag. `App.final_turn` tracks this (inferred from moves applied through the app; Ctrl+E overrides it for hand-entered positions; the stats pane shows `last:OPP?` while it is only assumed)
