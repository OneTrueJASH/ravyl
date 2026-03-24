# cpsolver2 — Project Primer

Complete TUI redesign of `crossplay-solver`.

## Module Layout

| Module | Purpose |
|---|---|
| `tile` | `Tile` struct, `Rack` (7-slot), `letter_value()` with Crossplay scoring; constants: `RACK_SIZE` |
| `board` | 15x15 `Board`, `Cell`, `Premium` enum, `Direction` enum, Crossplay premium layout; constants: `BOARD_SIZE` |
| `dictionary` | FST-backed `Dictionary` with `is_valid()`, `has_prefix()`, `load_default_dictionary()` |
| `game` | `SolverState` (Board + Rack), tile distribution (100 tiles), `unseen_tiles()`, endgame detection; constants: `TOTAL_TILES`, `BLANK_COUNT` |
| `solver` | `Move`, `ExternalMoveData`, anchor-based move generation, `leave_value()`, `SolverConfig`, `Solver`; constants: `BINGO_BONUS` (40) |
| `stats` | `GameStats`, `GamePhase`, `TileLocation`, power tile tracking, rack analysis |
| `settings` | `Settings` with `DisplaySettings` (`default_sort`, `sidebar_visible`), TOML persistence at `~/.config/cpsolver2/settings.toml`; constant: `LEXICON = "NWL23"` |
| `logging` | Instance-specific logging to `/tmp/crossplay-<pid>.log` |
| `refengine_ffi` | FFI bindings to RefEngine C library (`librefengine.dylib`), `RefEngine` safe wrapper |
| `refengine_mcts` | `RefSimulator`, `RefSimConfig`, `RefSimResult`, `InferenceResult`, CGP conversion, adaptive simulation based on game phase |
| `mcts` | `MctsEvaluator`, `MctsResult`, `MctsConfig`, `TilePool` with Fisher-Yates shuffle, 2-ply lookahead via Rayon parallelism |
| `endgame` | `EndgameSolver`, `EndgameMove`, `PvMove` — Crossplay-native 2-ply endgame solver (no leftover tile penalties, no go-out bonus) |
| `app` | `App` (central state), `Focus`, `SortMode`, `DialogState`, `BoardTab`, `handle_key()`/`handle_mouse()` event handlers, undo support |
| `ui::theme` | Centralized color/style constants — all UI colors defined here |
| `ui::hit` | Thread-local hit map for mouse click resolution (`HitTarget`, `register_hit()`, `resolve_click()`) |
| `ui::events` | `AppAction` enum (`Quit`, `Redraw`, `Nothing`) |
| `ui::layout` | Fluid `AppLayout` computed from terminal size (no breakpoints) |
| `ui::board_view` | Board widget with premium squares, cursor, preview tiles |
| `ui::stats_view` | Rack tiles + game stats + opponent inference widget |
| `ui::suggestions` | Suggestions table with sort-mode tabs and simulation equity lookup |
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
- Unique premium square layout (not standard Scrabble)
- Endgame: when the bag empties, each player gets one final turn (equal turn rule)
- No leftover tile penalty — unplayed rack tiles don't affect scoring
- No go-out bonus — emptying your rack gives no extra points
