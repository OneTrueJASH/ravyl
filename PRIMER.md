# cpsolver2 — Project Primer

Complete TUI redesign of `crossplay-solver`.

## Module Layout

| Module | Purpose |
|---|---|
| `tile` | `Tile` struct, `Rack` (7-slot), `letter_value()` with Crossplay scoring |
| `board` | 15x15 `Board`, `Cell`, `Premium` enum, `Direction` enum, Crossplay premium layout |
| `dictionary` | FST-backed `Dictionary` with `is_valid()`, `has_prefix()` |
| `game` | `SolverState` (Board + Rack), tile distribution (100 tiles), `unseen_tiles()` |
| `solver` | `Move`, anchor-based move generation, `leave_value()`, `SolverConfig` (40pt bingo) |
| `stats` | `GameStats`, `GamePhase`, `TileLocation`, power tile tracking |
| `settings` | `Settings` with TOML persistence at `~/.config/cpsolver2/settings.toml` |
| `logging` | Instance-specific logging to `/tmp/crossplay-<pid>.log` |
| `refengine_ffi` | FFI bindings to RefEngine C library (`librefengine.dylib`) |
| `refengine_mcts` | `RefSimulator`, sim results, CGP conversion, adaptive simulation |
| `mcts` | `MctsEvaluator`, `MctsResult`, `TilePool` with Fisher-Yates shuffle |
| `endgame` | Crossplay-native 2-ply endgame solver (no leftover tile penalties) |
| `app` | `App` (central state), `Focus`, `SortMode`, `DialogState`, `BoardTab`, event handlers |
| `ui::theme` | Centralized color/style constants — all UI colors defined here |
| `ui::hit` | Thread-local hit map for mouse click resolution |
| `ui::events` | `AppAction` enum (quit, redraw, nothing) |
| `ui::layout` | Fluid `AppLayout` computed from terminal size (no breakpoints) |
| `ui::board_view` | Board widget with premium squares, cursor, preview tiles |
| `ui::stats_view` | Rack tiles + game stats + opponent inference widget |
| `ui::suggestions` | Suggestions table with `SimEquityLookup` |
| `ui::log_viewer` | Tab bar + log viewer |
| `ui::status_bar` | Title bar, message bar, context-sensitive hint bar |
| `ui::popup_frame` | Shared popup rendering (clear + border + title) |
| `ui::help_popup` | Help dialog with keyboard shortcuts |
| `ui::confirm_popup` | Yes/No confirmation dialog |
| `ui::menu_popup` | Escape menu (Quit/Reset/Cancel) |
| `ui::settings_popup` | Settings editor with tabs |

## Key Design Differences from crossplay-solver

1. **Mouse support** — thread-local hit map populated during render, resolved on click
2. **Centralized theme** — all colors in `ui::theme`, no inline `Color::Rgb(...)` in widgets
3. **Fluid layout** — `AppLayout::compute()` replaces the 4-mode `LayoutMode` enum
4. **Event handlers in App** — `handle_key()`, `handle_mouse()` methods on `App` instead of free functions in `main.rs`
5. **Shared popup frame** — `popup_frame::render_popup_frame()` used by all dialogs
6. **Clean event loop** — main loop dispatches `Event::Key` / `Event::Mouse` → `AppAction`

## Data

- `external/refengine/` — RefEngine C library + lexicon/strategy data files
- `data/` — symlink to reference project's FST dictionary files

## Crossplay-Specific Rules

- 100 tiles (3 blanks, 5 S's)
- 40-point bingo bonus (not 50)
- Unique premium square layout (not standard Scrabble)
