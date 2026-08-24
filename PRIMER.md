# cpsolver2 — Project Primer

Complete TUI redesign of `crossplay-solver`.

## Module Layout

| Module | Purpose |
|---|---|
| `tile` | `Tile` struct, `Rack` (7-slot), `letter_value()` with Crossplay scoring; constants: `RACK_SIZE` |
| `board` | 15x15 `Board`, `Cell`, `Premium` enum, `Direction` enum, Crossplay premium layout; constants: `BOARD_SIZE` |
| `dictionary` | FST-backed `Dictionary`: node-level walking (`root()`, `child()`) for the move generator, `leave_value()` (fitted table or heuristic, phase-scaled), `load_default_dictionary()` which also attaches the leave table when `external/refengine/data/lexica/NWL23.klv2` is present |
| `leaves` | `LeaveValues` — parser and lookup for RefEngine `.klv2` leave tables (DAWG + one value per 1–6 tile leave); `bingo_probability()` / `bingo_bonus_correction()` — self-play-fitted logistic model that converts the table's 50-point-bingo values to Crossplay's 40 |
| `game` | `SolverState` (Board + Rack), tile distribution (100 tiles), `unseen_tiles()`, `bag_size()`, endgame detection, `from_cgp()`, `get_suggestions()` (plays plus exchanges when the bag allows), `exchange_moves()`, `full_tile_set()`, `observed_move_loss()` (equity an observed opponent play gave up, over sampled completions of their rack); constants: `TOTAL_TILES`, `BLANK_COUNT` |
| `solver` | `Move` with `MoveKind` (`Play` / `Exchange` / `Pass`), `coords()` (`8D`/`D8`, `-` for non-plays), anchor-based move generation over FST nodes with bitmask cross-checks and incremental scoring, `leave_value_with_phase()` (per-letter fallback), `SolverConfig`, `Solver`; constants: `BINGO_BONUS` (40), `EXCHANGE_MIN_BAG` (7), `PASS_WORD`. Verified play-for-play against RefEngine in `test_movegen_matches_refengine` |
| `stats` | `GameStats`, `GamePhase`, `TileLocation`, power tile tracking, rack analysis |
| `settings` | `Settings` with `DisplaySettings` (`default_sort`, `sidebar_visible`), TOML persistence at `~/.config/cpsolver2/settings.toml`; constant: `LEXICON = "NWL23"` |
| `logging` | Instance-specific logging to `/tmp/crossplay-<pid>.log` |
| `refengine_ffi` | FFI bindings to RefEngine C library (`librefengine.dylib`), `RefEngine` safe wrapper |
| `refengine_mcts` | `RefSimulator` (RefEngine handle configured with `-bb 40`), `InferenceResult` (played tiles, top leaves, `LetterMarginal` per-letter expected counts), `infer_opponent_rack()`, CGP conversion. RefEngine is used only to infer the opponent's likely leave and as the reference move generator in tests; its simulation and endgame search (Scrabble ending rules) are not used |
| `mcts` | `MctsEvaluator`, `MctsResult`, `MctsConfig` (`for_bag`), `TilePool`, `TracePoint` — Monte Carlo playouts under Crossplay rules for every phase with the bag non-empty: greedy static-equity play (including exchanges) until the bag empties, then exact 2-ply last turn and score-only final move; cut-off positions valued as spread + leaves + tempo (±21); win probability from a normal model with sd² = 159 + 80 × bag; common random numbers across candidates, paired-difference pruning, wall-clock budget, Rayon parallelism. `OpponentModel` biases the opponent's dealt rack with inference marginals and weakens their simulated play to a measured softmax temperature (see key behavior 12). Also hosts the self-play studies (`study_spread_variance`, `study_bingo_probability`, `study_loss_calibration`, ignored tests) that produced those constants |
| `endgame` | `EndgameSolver`, `EndgameMove`, `PvMove`, `PASS_WORD` — Crossplay-native endgame solver: exact 2-ply (including passing) when the opponent replies, score-only when our move ends the game |
| `app` | `App` (central state), `Focus`, `SortMode`, `FinalTurn` (who plays the last move once the bag is empty), `SimResult` (display record for playout/endgame results), `DialogState`, `BoardTab`, `handle_key()`/`handle_mouse()` event handlers, undo support, engine routing by game phase, background RefEngine inference, opponent-strength measurement (`opp_temperature()`, `opp_error_estimate()`) |
| `ui::theme` | Centralized color/style constants — all UI colors defined here |
| `ui::hit` | Thread-local hit map for mouse click resolution (`HitTarget`, `register_hit()`, `resolve_click()`) |
| `ui::events` | `AppAction` enum (`Quit`, `Redraw`, `Nothing`) |
| `ui::layout` | Fluid `AppLayout` computed from terminal size (no breakpoints); the board wins over the sidebar when both cannot fit |
| `ui::board_view` | Board widget with premium squares, cursor, preview tiles; `BoardGeometry` picks full (4-row cells), compact (3-row) or borderless mini cells to fit the pane |
| `ui::stats_view` | Rack tiles + game stats + opponent inference widget (likely kept letters with the blank promoted, played tiles, measured error rate) |
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
9. **Engine routing by phase** — `App::run_mcts_evaluation`: our move ends the game → sort by score; bag empty and the opponent replies → `EndgameSolver`; otherwise native playouts (`mcts.rs`) with depth by bag size (2 plies with 22+ in the bag, 4 with 8–21, to the end with 1–7). Every engine applies Crossplay's ending; RefEngine's simulation and endgame solver apply Scrabble rules and are never used for ranking
10. **Leave values from RefEngine's table, corrected** — static equity reads `NWL23.klv2` (fitted for standard Scrabble with a 50-point bingo) and subtracts 10 × (next-turn bingo probability of the leave − that of an empty leave), with the probability from a logistic model fitted on Crossplay self-play (`leaves.rs`). Falls back to the per-letter heuristic in `solver.rs` when the file is missing. Leave weight is scaled by phase: ×0.6 with one draw left, ×0.25 once the bag is empty
11. **Exchanges** — offered and played out whenever the bag holds at least `EXCHANGE_MIN_BAG` (7) tiles; the threshold is the common word-game convention and has not been confirmed from NYT's own rules text
12. **Opponent model in playouts** — `OpponentModel` (`mcts.rs`) feeds two measured facts into the playouts. (a) RefEngine's rack inference is parsed into per-letter marginals (expected count of each letter among the tiles the opponent kept after their last play); the playout deal samples their kept tiles by weight (inferred expectation over the uniform expectation, clamped to [0.05, 20]) so inferred tiles land in their rack, never in our draw. The evaluation thread waits up to 3 s for a fresh inference and otherwise uses the stored one, which stays valid until the opponent moves again. (b) Every auto-scored opponent move is measured against the best play/exchange from 10 sampled completions of their rack (`observed_move_loss` in `game.rs`); the running mean maps to a softmax temperature for the simulated opponent through a fitted inversion (`study_loss_calibration`: measured ≈ 7.78 + 0.557·τ — the baseline exists because a sampled completion usually admits a better play than their true rack), shrunk by n/(n+2) and capped at 12. Temperature affects only the opponent's greedy-phase moves; both players stay exact on the final two turns, and our own side is always greedy

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
- A turn may also be used to pass, or to exchange any number of tiles (the turn is forfeited; assumed to require at least 7 tiles in the bag)
- Consequence for the solver: with the bag empty, the opponent only replies to our play if *they* emptied the bag. `App.final_turn` tracks this (inferred from moves applied through the app; Ctrl+E overrides it for hand-entered positions; the stats pane shows `last:OPP?` while it is only assumed)
