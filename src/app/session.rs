//! The open game: autosaving it (throttled) under its name, its lock,
//! naming and renaming, switching games, the game picker and the quit
//! flow. Storage and locks themselves live in [`crate::games`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::board::{Direction, BOARD_SIZE};
use crate::games::{self, GameEntry, OppLosses, SavedGame};
use crate::logging::log;

use super::input::DOUBLE_CLICK;
use super::state::{App, DialogState, FinalTurn, Focus, MessageKind};

/// Autosaves are at most this often while the position keeps changing.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Rows PgUp/PgDn move in the picker.
pub const PICKER_PAGE: usize = 10;

/// Longest game name the prompt accepts.
const MAX_NAME_LEN: usize = 40;

/// What the "Save game as" prompt is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveAsPurpose {
    /// Name or rename the open game (Esc menu).
    Save,
    /// Name the game before quitting: Enter saves and quits, Esc cancels
    /// the quit.
    Quit,
    /// Rename a game from the picker (possibly the open one), then return
    /// to the picker.
    Rename { slug: String, startup: bool },
}

impl App {
    /// The open game's slug (file and lock name), once it has a name.
    pub fn game_slug(&self) -> Option<String> {
        self.game_name.as_deref().map(games::slug)
    }

    /// The position changed: the game needs saving.
    pub(super) fn mark_dirty(&mut self) {
        self.game_dirty = true;
    }

    /// Nothing entered yet: empty board and rack, no score.
    fn position_is_empty(&self) -> bool {
        self.state.rack.is_empty()
            && self.our_score == 0
            && self.opp_score == 0
            && (0..BOARD_SIZE).all(|r| (0..BOARD_SIZE).all(|c| self.state.get_tile(r, c).is_none()))
    }

    /// Give an unnamed game its automatic `Untitled …` name and take its
    /// lock. False if saving is off or the lock could not be taken.
    fn ensure_named(&mut self) -> bool {
        if self.game_name.is_some() {
            return true;
        }
        let Some(dir) = self.games_dir.clone() else {
            return false;
        };
        let name = games::unused_name(&dir, &games::untitled_name(games::now_secs()));
        match games::lock(&dir, &games::slug(&name)) {
            Ok(()) => {
                self.game_name = Some(name);
                true
            }
            Err(e) => {
                log(&format!("Could not lock a new game: {:?}", e));
                false
            }
        }
    }

    /// The open game as it would be saved now, with the directory to save
    /// it in. An unnamed game that has something in it gets its automatic
    /// name (and lock) here. `None` when saving is off or there is nothing
    /// to save yet. The binary keeps the latest one for the signal
    /// watchdog, which saves it if the event loop cannot.
    pub fn save_snapshot(&mut self) -> Option<(PathBuf, SavedGame)> {
        let dir = self.games_dir.clone()?;
        if self.game_name.is_none() && self.position_is_empty() {
            return None;
        }
        if !self.ensure_named() {
            return None;
        }
        let game = SavedGame {
            name: self.game_name.clone().unwrap_or_default(),
            cgp: self.position_cgp(),
            updated: games::now_secs(),
            opp_losses: OppLosses {
                total: self.opp_loss_total,
                moves: self.opp_moves_measured,
            },
        };
        Some((dir, game))
    }

    /// Save the open game now. An unnamed game gets its automatic name on
    /// its first save; one with nothing entered yet is not saved at all.
    /// Returns true if the game was written.
    pub fn autosave(&mut self) -> bool {
        let Some((dir, game)) = self.save_snapshot() else {
            if self.game_name.is_none() && self.position_is_empty() {
                self.game_dirty = false;
            }
            return false;
        };
        match games::save(&dir, &game) {
            Ok(()) => {
                self.game_dirty = false;
                self.last_autosave = Some(Instant::now());
                true
            }
            Err(e) => {
                log(&format!("Autosave of {} failed: {}", game.name, e));
                false
            }
        }
    }

    /// How long until a pending autosave is due (zero if it is due now),
    /// or `None` with nothing to save. The event loop wakes up for it.
    pub fn autosave_due_in(&self, now: Instant) -> Option<Duration> {
        if !self.game_dirty || self.games_dir.is_none() {
            return None;
        }
        Some(self.last_autosave.map_or(Duration::ZERO, |at| {
            (at + AUTOSAVE_INTERVAL).saturating_duration_since(now)
        }))
    }

    /// Save the game if it changed and the last save was at least
    /// [`AUTOSAVE_INTERVAL`] before `now`. Returns true if it saved.
    pub fn autosave_if_due(&mut self, now: Instant) -> bool {
        if self.autosave_due_in(now) != Some(Duration::ZERO) {
            return false;
        }
        let written = self.autosave();
        // A failed save is retried after the interval, not on every tick.
        self.last_autosave = Some(now);
        written
    }

    /// On exit: save the game and release its lock.
    pub fn shutdown(&mut self) {
        self.autosave();
        self.release_game();
    }

    /// Release the open game's lock (it stays open in this App).
    fn release_game(&mut self) {
        if let (Some(dir), Some(slug)) = (self.games_dir.as_ref(), self.game_slug()) {
            games::unlock(dir, &slug);
        }
    }

    /// At startup: import the old session file once, then open `--game`
    /// (or create it), or show the picker when there are saved games this
    /// window can open.
    pub fn start_session(&mut self, game: Option<&str>) -> Result<(), String> {
        let Some(dir) = self.games_dir.clone() else {
            return Ok(());
        };
        if games::migrate_session(&dir).is_some() {
            log("Imported the old session as a saved game");
        }
        if let Some(name) = game {
            let slug = games::slug(name);
            if games::exists(&dir, &slug) {
                return self.open_game(&slug);
            }
            self.start_new_game(Some(name.trim().to_string()))?;
            self.set_message(MessageKind::Info, format!("New game: {}", name.trim()));
            return Ok(());
        }
        if games::list(&dir).iter().any(|g| !g.locked) {
            self.show_game_picker(true);
        }
        Ok(())
    }

    /// Forget everything that belongs to the open game: position, scores,
    /// undo history, the opponent model, inference, results and the
    /// simulation resume cache.
    fn clear_game_state(&mut self) {
        self.cancel_evaluation();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.state.reset();
        self.our_score = 0;
        self.opp_score = 0;
        self.final_turn = FinalTurn::default();
        self.final_turn_known = false;
        self.opp_loss_total = 0.0;
        self.opp_moves_measured = 0;
        self.inferred_opponent_rack = None;
        self.inference_receiver = None;
        self.opp_move_stamp += 1;
        self.last_applied_board = None;
        self.pending_opponent_tiles = None;
        self.pre_opponent_board = None;
        self.pending_opponent_score = 0;
        self.awaiting_opp_reply = false;
        if let Ok(mut cache) = self.sim_cache.lock() {
            *cache = None;
        }
        self.cursor = (7, 7);
        self.typing_direction = Direction::Horizontal;
        self.focus = Focus::Board;
        self.filter = None;
        self.filter_editing = false;
        self.selected_suggestion = 0;
        self.list_offset.set(0);
        self.last_row_click = None;
        self.clear_suggestions();
    }

    /// Open the saved game `slug` in this window: refused while another
    /// window has it. The game open until now is saved and released first.
    pub fn open_game(&mut self, slug: &str) -> Result<(), String> {
        let Some(dir) = self.games_dir.clone() else {
            return Err("Saved games are off".to_string());
        };
        if self.game_slug().as_deref() == Some(slug) {
            return Ok(());
        }
        let game =
            games::load(&dir, slug).map_err(|e| format!("Could not read {}: {}", slug, e))?;
        match games::lock(&dir, slug) {
            Ok(()) => {}
            Err(games::LockError::Held(_)) => {
                return Err(format!("{} is open in another window", game.name));
            }
            Err(games::LockError::Io(e)) => {
                return Err(format!("Could not lock {}: {}", game.name, e));
            }
        }
        self.shutdown();
        self.clear_game_state();
        if let Err(e) = self.load_position(&game.cgp, false) {
            log(&format!("Saved game {} unreadable: {}", game.name, e));
            self.set_message(
                MessageKind::Error,
                format!("{} could not be read: {}", game.name, e),
            );
        } else {
            self.set_message(
                MessageKind::Info,
                format!("Opened {} (^O lists your games)", game.name),
            );
        }
        self.opp_loss_total = game.opp_losses.total;
        self.opp_moves_measured = game.opp_losses.moves;
        self.game_name = Some(game.name);
        self.game_dirty = false;
        self.last_autosave = None;
        Ok(())
    }

    /// Save and release the open game and start an empty one, with `name`
    /// (locked now) or unnamed (named on its first save).
    fn start_new_game(&mut self, name: Option<String>) -> Result<(), String> {
        if let (Some(dir), Some(name)) = (self.games_dir.as_ref(), name.as_ref()) {
            match games::lock(dir, &games::slug(name)) {
                Ok(()) => {}
                Err(games::LockError::Held(_)) => {
                    return Err(format!("{} is open in another window", name))
                }
                Err(games::LockError::Io(e)) => {
                    return Err(format!("Could not lock {}: {}", name, e))
                }
            }
        }
        self.shutdown();
        self.clear_game_state();
        self.game_name = name;
        self.game_dirty = self.game_name.is_some();
        self.last_autosave = None;
        Ok(())
    }

    /// Esc menu or picker `n`: a new untitled game.
    pub fn new_game(&mut self) {
        // Starting unnamed cannot fail.
        let _ = self.start_new_game(None);
        self.set_message(
            MessageKind::Info,
            "New game \u{2014} it is saved as you play (Esc menu: Save Game As)",
        );
    }

    /// Reload the picker's game list.
    pub(super) fn refresh_picker(&mut self) {
        self.picker_games = self
            .games_dir
            .as_deref()
            .map(games::list)
            .unwrap_or_default();
        // The game open here holds its own lock; it is not "in another
        // window".
        if let Some(slug) = self.game_slug() {
            for g in &mut self.picker_games {
                if g.slug == slug {
                    g.locked = false;
                }
            }
        }
    }

    /// Open the game picker (`startup`: shown at launch, Esc starts a new
    /// game). The open game is saved first so the list is current.
    pub fn show_game_picker(&mut self, startup: bool) {
        if self.games_dir.is_none() {
            self.set_message(MessageKind::Error, "Saved games are off");
            return;
        }
        if self.game_dirty {
            self.autosave();
        }
        self.refresh_picker();
        self.last_picker_click = None;
        self.dialog = DialogState::GamePicker {
            selected: 0,
            startup,
        };
    }

    /// The picker after a rename or delete, with the selection on `slug`
    /// when it is still listed.
    fn return_to_picker(&mut self, slug: Option<&str>, startup: bool) {
        self.refresh_picker();
        let selected = slug
            .and_then(|s| self.picker_games.iter().position(|g| g.slug == s))
            .unwrap_or(0);
        self.dialog = DialogState::GamePicker { selected, startup };
    }

    /// True if this game is the one open in this window.
    pub fn is_current_game(&self, entry: &GameEntry) -> bool {
        self.game_slug().as_deref() == Some(entry.slug.as_str())
    }

    /// Open the "Save game as" prompt.
    pub fn show_save_as(&mut self, purpose: SaveAsPurpose) {
        if self.games_dir.is_none() {
            self.set_message(MessageKind::Error, "Saved games are off");
            return;
        }
        let name = match &purpose {
            SaveAsPurpose::Rename { slug, .. } => self
                .picker_games
                .iter()
                .find(|g| &g.slug == slug)
                .map(|g| g.name.clone())
                .unwrap_or_default(),
            _ => {
                if !self.position_is_empty() {
                    self.ensure_named();
                }
                self.game_name.clone().unwrap_or_default()
            }
        };
        self.dialog = DialogState::SaveAs {
            pristine: games::is_automatic_name(&name),
            buffer: name,
            purpose,
        };
    }

    /// True if quitting should first offer to name the game: saving is on,
    /// and the game has something in it but only an automatic name.
    pub fn needs_name_before_quit(&self) -> bool {
        self.games_dir.is_some()
            && match &self.game_name {
                Some(name) => games::is_automatic_name(name),
                None => !self.position_is_empty(),
            }
    }

    /// Ask to quit (Ctrl+C or the menu): the naming prompt for an
    /// untitled game, otherwise the usual confirmation.
    pub(super) fn request_quit(&mut self) {
        if self.needs_name_before_quit() {
            self.show_save_as(SaveAsPurpose::Quit);
        } else {
            self.dialog = DialogState::ConfirmQuit(false);
        }
    }

    /// Typing in the "Save game as" prompt.
    pub(super) fn handle_save_as_key(&mut self, key: KeyEvent) {
        let DialogState::SaveAs {
            ref mut buffer,
            ref mut pristine,
            ref purpose,
        } = self.dialog
        else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => {
                buffer.clear();
                *pristine = false;
            }
            KeyCode::Char(c) if !ctrl && !c.is_control() => {
                // An automatic name is replaced by the first key typed.
                if *pristine {
                    buffer.clear();
                    *pristine = false;
                }
                if buffer.chars().count() < MAX_NAME_LEN {
                    buffer.push(c);
                }
            }
            KeyCode::Backspace => {
                *pristine = false;
                buffer.pop();
            }
            KeyCode::Enter => {
                let (text, purpose) = (buffer.clone(), purpose.clone());
                self.commit_save_as(&text, purpose, false);
            }
            KeyCode::Esc => match purpose.clone() {
                SaveAsPurpose::Rename { slug, startup } => {
                    self.return_to_picker(Some(&slug), startup)
                }
                SaveAsPurpose::Quit => {
                    self.close_dialog();
                    self.set_message(MessageKind::Info, "Quit cancelled");
                }
                SaveAsPurpose::Save => self.close_dialog(),
            },
            _ => {}
        }
    }

    /// Enter in the "Save game as" prompt (or Yes on the overwrite
    /// question, with `overwrite`): save or rename under `text`, asking
    /// first before replacing another saved game.
    pub(super) fn commit_save_as(&mut self, text: &str, purpose: SaveAsPurpose, overwrite: bool) {
        let name = text.trim().to_string();
        let Some(dir) = self.games_dir.clone() else {
            self.close_dialog();
            return;
        };
        if name.is_empty() {
            if purpose == SaveAsPurpose::Quit {
                self.should_quit = true;
                self.close_dialog();
            } else {
                self.set_message(MessageKind::Error, "Type a name for the game");
            }
            return;
        }
        let from = match &purpose {
            SaveAsPurpose::Rename { slug, .. } => Some(slug.clone()),
            _ => self.game_slug(),
        };
        let to = games::slug(&name);
        if from.as_deref() != Some(to.as_str()) {
            if games::lock_holder(&dir, &to).is_some() {
                self.set_message(
                    MessageKind::Error,
                    format!("{} is open in another window", name),
                );
                return;
            }
            if games::exists(&dir, &to) && !overwrite {
                self.dialog = DialogState::ConfirmOverwrite {
                    name,
                    purpose,
                    yes: false,
                };
                return;
            }
        }
        let renaming_current = from.is_some() && from == self.game_slug();
        let result = if renaming_current || from.is_none() {
            self.rename_current(&dir, &name)
        } else {
            Self::rename_other(&dir, from.as_deref().unwrap_or_default(), &name)
        };
        if let Err(e) = result {
            self.set_message(MessageKind::Error, e);
            return;
        }
        match purpose {
            SaveAsPurpose::Save => {
                self.close_dialog();
                self.set_message(MessageKind::Success, format!("Saved as {}", name));
            }
            SaveAsPurpose::Quit => {
                self.should_quit = true;
                self.close_dialog();
            }
            SaveAsPurpose::Rename { startup, .. } => {
                self.set_message(MessageKind::Success, format!("Renamed to {}", name));
                self.return_to_picker(Some(&to), startup);
            }
        }
    }

    /// Give the open game `name`: its file and lock move to the new slug
    /// (replacing a saved game there, which the caller confirmed).
    fn rename_current(&mut self, dir: &std::path::Path, name: &str) -> Result<(), String> {
        let old = self.game_slug();
        let new = games::slug(name);
        if old.as_deref() != Some(new.as_str()) {
            match games::lock(dir, &new) {
                Ok(()) => {}
                Err(games::LockError::Held(_)) => {
                    return Err(format!("{} is open in another window", name))
                }
                Err(games::LockError::Io(e)) => return Err(format!("Could not save: {}", e)),
            }
        }
        self.game_name = Some(name.to_string());
        if !self.autosave() {
            // The save failed (logged): keep the old name and file.
            if old.as_deref() != Some(new.as_str()) {
                games::unlock(dir, &new);
            }
            self.game_name = old.and_then(|s| games::load(dir, &s).ok().map(|g| g.name));
            return Err("Could not save the game (see the log)".to_string());
        }
        if let Some(old) = old.filter(|o| *o != new) {
            let _ = std::fs::remove_file(games::game_path(dir, &old));
            games::unlock(dir, &old);
        }
        Ok(())
    }

    /// Rename a saved game that is not open here (and not open anywhere).
    fn rename_other(dir: &std::path::Path, from: &str, name: &str) -> Result<(), String> {
        let mut game = games::load(dir, from).map_err(|e| format!("Could not read it: {}", e))?;
        match games::lock(dir, from) {
            Ok(()) => {}
            Err(games::LockError::Held(_)) => {
                return Err(format!("{} is open in another window", game.name))
            }
            Err(games::LockError::Io(e)) => return Err(format!("Could not rename: {}", e)),
        }
        game.name = name.to_string();
        let result = games::save(dir, &game).map_err(|e| format!("Could not rename: {}", e));
        if result.is_ok() && games::slug(name) != from {
            let _ = std::fs::remove_file(games::game_path(dir, from));
        }
        games::unlock(dir, from);
        result
    }

    /// Delete a saved game from the picker (never the open one, nor one
    /// open in another window).
    pub(super) fn delete_game(&mut self, slug: &str) {
        let Some(dir) = self.games_dir.clone() else {
            return;
        };
        if games::lock_holder(&dir, slug).is_some() {
            self.set_message(MessageKind::Error, "That game is open in a window");
            return;
        }
        match games::delete(&dir, slug) {
            Ok(()) => self.set_message(MessageKind::Info, "Game deleted"),
            Err(e) => self.set_message(MessageKind::Error, format!("Could not delete: {}", e)),
        }
    }

    /// Keys in the game picker.
    pub(super) fn handle_picker_key(&mut self, key: KeyEvent, selected: usize, startup: bool) {
        let n = self.picker_games.len();
        let last = n.saturating_sub(1);
        let select = |app: &mut App, i: usize| {
            app.dialog = DialogState::GamePicker {
                selected: i.min(last),
                startup,
            }
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => select(self, selected.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => select(self, selected + 1),
            KeyCode::PageUp => select(self, selected.saturating_sub(PICKER_PAGE)),
            KeyCode::PageDown => select(self, selected + PICKER_PAGE),
            KeyCode::Home => select(self, 0),
            KeyCode::End => select(self, last),
            KeyCode::Enter => self.open_from_picker(selected),
            KeyCode::Char('n') => {
                self.close_dialog();
                self.new_game();
            }
            KeyCode::Char('r') => {
                if let Some(g) = self.picker_games.get(selected).cloned() {
                    if g.locked {
                        self.set_message(
                            MessageKind::Error,
                            format!("{} is open in another window", g.name),
                        );
                    } else {
                        self.show_save_as(SaveAsPurpose::Rename {
                            slug: g.slug,
                            startup,
                        });
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(g) = self.picker_games.get(selected).cloned() {
                    if self.is_current_game(&g) {
                        self.set_message(
                            MessageKind::Error,
                            "That game is open here \u{2014} open another one first",
                        );
                    } else if g.locked {
                        self.set_message(
                            MessageKind::Error,
                            format!("{} is open in another window", g.name),
                        );
                    } else {
                        self.dialog = DialogState::ConfirmDelete {
                            slug: g.slug,
                            name: g.name,
                            startup,
                            yes: false,
                        };
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => self.close_dialog(),
            _ => {}
        }
    }

    /// Open the picker's game at `index` (closing the picker), or explain
    /// why not.
    pub(super) fn open_from_picker(&mut self, index: usize) {
        let Some(g) = self.picker_games.get(index).cloned() else {
            return;
        };
        if g.locked {
            self.set_message(
                MessageKind::Error,
                format!("{} is open in another window", g.name),
            );
            return;
        }
        match self.open_game(&g.slug) {
            Ok(()) => self.close_dialog(),
            Err(e) => {
                self.set_message(MessageKind::Error, e);
                self.refresh_picker();
            }
        }
    }

    /// Yes/No on the delete question; either way back to the picker.
    pub(super) fn answer_delete(&mut self, yes: bool) {
        let DialogState::ConfirmDelete {
            ref slug, startup, ..
        } = self.dialog
        else {
            return;
        };
        let (slug, startup) = (slug.clone(), startup);
        if yes {
            self.delete_game(&slug);
        }
        self.return_to_picker(if yes { None } else { Some(&slug) }, startup);
    }

    /// Yes/No on the overwrite question: Yes saves over the other game,
    /// No goes back to the prompt.
    pub(super) fn answer_overwrite(&mut self, yes: bool) {
        let DialogState::ConfirmOverwrite {
            ref name,
            ref purpose,
            ..
        } = self.dialog
        else {
            return;
        };
        let (name, purpose) = (name.clone(), purpose.clone());
        if yes {
            self.commit_save_as(&name, purpose, true);
        } else {
            self.dialog = DialogState::SaveAs {
                buffer: name,
                purpose,
                pristine: false,
            };
        }
    }

    /// A click on a picker row: select it; a second click on the same row
    /// within the double-click window opens it.
    pub(super) fn click_picker_row(&mut self, index: usize) {
        let DialogState::GamePicker { selected, startup } = self.dialog else {
            return;
        };
        let now = Instant::now();
        let double = selected == index
            && self
                .last_picker_click
                .is_some_and(|(row, at)| row == index && now - at <= DOUBLE_CLICK);
        if double {
            self.last_picker_click = None;
            self.open_from_picker(index);
        } else {
            self.last_picker_click = Some((index, now));
            self.dialog = DialogState::GamePicker {
                selected: index.min(self.picker_games.len().saturating_sub(1)),
                startup,
            };
        }
    }
}
