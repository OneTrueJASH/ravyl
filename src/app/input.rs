//! Keyboard, mouse and paste input: global keys, per-pane keys, dialogs,
//! the list filter and focus cycling.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::board::Direction;
use crate::solver::{Move, MoveKind};
use crate::ui::events::AppAction;
use crate::ui::hit::{resolve_click, HitTarget};
use crate::ui::menu_popup::EscapeMenuOption;

use super::session::SaveAsPurpose;
use super::state::{App, BoardTab, DialogState, Focus, MessageKind, ScoreTarget};

/// Two clicks on the same suggestion row within this window apply it.
pub(super) const DOUBLE_CLICK: Duration = Duration::from_millis(400);

impl App {
    // ── Event handlers ──────────────────────────────────────────────

    /// Handle a keyboard event and return the action for the main loop.
    pub fn handle_key(&mut self, key: KeyEvent) -> AppAction {
        let before = self.selected_suggestion;
        let action = self.dispatch_key(key);
        self.note_selection_moved(before);
        action
    }

    /// A selection changed by the user (not by arriving results) stops a
    /// live simulation from keeping the cursor on the leader: from then on
    /// it stays on the move the user picked.
    fn note_selection_moved(&mut self, before: usize) {
        if self.selected_suggestion != before {
            self.follow_leader = false;
        }
    }

    fn dispatch_key(&mut self, key: KeyEvent) -> AppAction {
        // Terminals that report releases send every key twice.
        if key.kind == KeyEventKind::Release {
            return AppAction::Nothing;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Ctrl+C asks first; a second Ctrl+C (or y) confirms. An untitled
        // game asks for a name instead, and a second Ctrl+C there quits
        // keeping the automatic name.
        if key.code == KeyCode::Char('c') && ctrl {
            if matches!(
                self.dialog,
                DialogState::ConfirmQuit { .. }
                    | DialogState::SaveAs {
                        purpose: SaveAsPurpose::Quit,
                        ..
                    }
            ) {
                return AppAction::Quit;
            }
            self.request_quit();
            return AppAction::Redraw;
        }

        // Esc: stop a running evaluation first (M resumes a simulation),
        // then close a dialog, then leave the filter, then return to the
        // board, then open the menu.
        if key.code == KeyCode::Esc && self.is_mcts_running() {
            self.cancel_evaluation();
            self.set_message(
                MessageKind::Info,
                "Simulation stopped \u{2014} M continues it",
            );
            return AppAction::Redraw;
        }

        // Dialog input.
        if self.has_dialog() {
            self.handle_dialog_key(key);
            if self.should_quit {
                return AppAction::Quit;
            }
            return AppAction::Redraw;
        }

        // Typing a suggestions filter.
        if self.filter_editing {
            self.handle_filter_key(key);
            return AppAction::Redraw;
        }

        // Keys that work in every pane.
        match key.code {
            KeyCode::F(1) => self.toggle_help(),
            KeyCode::Char('\\') => self.toggle_sidebar(),
            KeyCode::Char('e') if ctrl => self.toggle_final_turn(),
            KeyCode::Char('s') if ctrl => self.find_moves_and_focus(),
            KeyCode::Char('r') if ctrl => self.show_reset_confirm(),
            KeyCode::Char('o') if ctrl => self.show_game_picker(false),
            KeyCode::Char('z') if ctrl => self.undo(),
            KeyCode::Char('y') if ctrl => self.redo(),
            KeyCode::Char('=') => self.open_score_input(ScoreTarget::Opponent),
            KeyCode::Char('+') => self.open_score_input(ScoreTarget::Player),
            KeyCode::Char(':') => {
                self.dialog = DialogState::QuickEntry {
                    buffer: String::new(),
                }
            }
            KeyCode::Char('`') => self.toggle_board_tab(),
            KeyCode::Tab => self.next_focus(),
            KeyCode::BackTab => self.prev_focus(),
            KeyCode::Esc if self.focus != Focus::Board => {
                if self.filter.take().is_none() || self.focus != Focus::Suggestions {
                    self.focus = Focus::Board;
                }
            }
            KeyCode::Esc => self.show_escape_menu(),
            _ => match self.focus {
                Focus::Board => self.handle_board_key(key),
                Focus::Rack => self.handle_rack_key(key),
                Focus::Suggestions => self.handle_suggestions_key(key),
            },
        }
        AppAction::Redraw
    }

    /// Handle a mouse event and return the action for the main loop.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> AppAction {
        let before = self.selected_suggestion;
        let action = self.dispatch_mouse(mouse);
        self.note_selection_moved(before);
        action
    }

    fn dispatch_mouse(&mut self, mouse: MouseEvent) -> AppAction {
        if self.has_dialog() {
            return self.handle_dialog_mouse(mouse);
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(target) = resolve_click(mouse.column, mouse.row) {
                    self.handle_click(target)
                } else {
                    AppAction::Nothing
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                // The wheel scrolls the pane under the pointer, and stops
                // at the ends instead of wrapping.
                let down = mouse.kind == MouseEventKind::ScrollDown;
                match resolve_click(mouse.column, mouse.row) {
                    Some(
                        HitTarget::SuggestionRow(_)
                        | HitTarget::SortTab(_)
                        | HitTarget::FocusPane(Focus::Suggestions),
                    ) => self.step_selection(if down { 3 } else { -3 }),
                    Some(HitTarget::BoardCell(..) | HitTarget::FocusPane(Focus::Board))
                        if self.board_tab == BoardTab::Logs =>
                    {
                        if down {
                            self.scroll_log_down(3)
                        } else {
                            self.scroll_log_up(3)
                        }
                    }
                    _ => return AppAction::Nothing,
                }
                AppAction::Redraw
            }
            // Pointer movement changes nothing on screen.
            _ => AppAction::Nothing,
        }
    }

    /// Mouse input while a dialog is open: only the dialog reacts. A click
    /// on a button acts on it, a click elsewhere inside the dialog does
    /// nothing, and a click outside it cancels the dialog.
    pub(super) fn handle_dialog_mouse(&mut self, mouse: MouseEvent) -> AppAction {
        // The wheel moves the picker's selection.
        if let DialogState::GamePicker { selected, startup } = self.dialog {
            let step = match mouse.kind {
                MouseEventKind::ScrollUp => Some(selected.saturating_sub(1)),
                MouseEventKind::ScrollDown => Some(selected + 1),
                _ => None,
            };
            if let Some(i) = step {
                self.dialog = DialogState::GamePicker {
                    selected: i.min(self.picker_games.len().saturating_sub(1)),
                    startup,
                };
                return AppAction::Redraw;
            }
        }
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return AppAction::Nothing;
        }
        match resolve_click(mouse.column, mouse.row) {
            Some(HitTarget::DialogYes) => match self.dialog {
                DialogState::ConfirmQuit { .. } => {
                    self.close_dialog();
                    return AppAction::Quit;
                }
                DialogState::ConfirmReset { .. } => {
                    self.close_dialog();
                    self.reset();
                }
                DialogState::ConfirmOverwrite { .. } => {
                    self.answer_overwrite(true);
                    if self.should_quit {
                        return AppAction::Quit;
                    }
                }
                DialogState::ConfirmDelete { .. } => self.answer_delete(true),
                _ => {}
            },
            Some(HitTarget::DialogNo) => match self.dialog {
                DialogState::ConfirmOverwrite { .. } => self.answer_overwrite(false),
                DialogState::ConfirmDelete { .. } => self.answer_delete(false),
                _ => self.close_dialog(),
            },
            Some(HitTarget::PickerRow(index)) => self.click_picker_row(index),
            Some(HitTarget::MenuItem(option)) => {
                if matches!(self.dialog, DialogState::EscapeMenu(_)) {
                    self.choose_menu_option(option);
                }
            }
            Some(HitTarget::DialogArea) => {}
            _ => self.close_dialog(),
        }
        AppAction::Redraw
    }

    /// Act on an escape-menu choice (Enter or a click).
    pub(super) fn choose_menu_option(&mut self, option: EscapeMenuOption) {
        match option {
            EscapeMenuOption::OpenGame => self.show_game_picker(false),
            EscapeMenuOption::SaveAs => self.show_save_as(SaveAsPurpose::Save),
            EscapeMenuOption::NewGame => {
                self.close_dialog();
                self.new_game();
            }
            EscapeMenuOption::Quit => self.request_quit(),
            EscapeMenuOption::Reset => self.dialog = DialogState::ConfirmReset { yes: false },
            EscapeMenuOption::Cancel => self.close_dialog(),
        }
    }

    /// Handle a resolved click target.
    pub(super) fn handle_click(&mut self, target: HitTarget) -> AppAction {
        match target {
            HitTarget::BoardCell(row, col) => {
                self.focus = Focus::Board;
                self.board_tab = BoardTab::Board;
                self.cursor = (row, col);
            }
            HitTarget::RackSlot(_) => {
                self.focus = Focus::Rack;
            }
            HitTarget::SuggestionRow(idx) => {
                let now = Instant::now();
                let double = self.focus == Focus::Suggestions
                    && self.selected_suggestion == idx
                    && self
                        .last_row_click
                        .is_some_and(|(row, at)| row == idx && now - at <= DOUBLE_CLICK);
                if double {
                    // A real double-click: apply the move.
                    self.last_row_click = None;
                    self.apply_suggestion_and_focus();
                } else {
                    self.focus = Focus::Suggestions;
                    self.selected_suggestion = idx.min(self.suggestions.len().saturating_sub(1));
                    self.last_row_click = Some((idx, now));
                }
            }
            HitTarget::TabHeader(tab) => {
                self.board_tab = tab;
                self.focus = Focus::Board;
                if tab == BoardTab::Logs {
                    self.log_scroll = 0;
                }
            }
            HitTarget::SortTab(mode) => {
                self.focus = Focus::Suggestions;
                self.set_sort_mode(mode);
                self.save_settings();
            }
            HitTarget::FocusPane(focus) => {
                self.focus = focus;
            }
            HitTarget::DialogYes
            | HitTarget::DialogNo
            | HitTarget::DialogArea
            | HitTarget::MenuItem(_)
            | HitTarget::PickerRow(_) => {}
        }
        AppAction::Redraw
    }

    // ── Focus-specific key handlers ─────────────────────────────────

    pub(super) fn handle_board_key(&mut self, key: KeyEvent) {
        if self.board_tab == BoardTab::Logs {
            match key.code {
                KeyCode::Up => self.scroll_log_up(1),
                KeyCode::Down => self.scroll_log_down(1),
                KeyCode::PageUp => self.scroll_log_up(20),
                KeyCode::PageDown => self.scroll_log_down(20),
                KeyCode::Home => self.scroll_log_up(10000),
                KeyCode::End => self.scroll_log_to_bottom(),
                _ => {}
            }
            return;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Up => self.move_cursor(-1, 0),
            KeyCode::Down => self.move_cursor(1, 0),
            KeyCode::Left => self.move_cursor(0, -1),
            KeyCode::Right => self.move_cursor(0, 1),
            // Shift+letter (an uppercase letter) places a blank.
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                let blank = shift || c.is_ascii_uppercase();
                self.place_on_board(c.to_ascii_uppercase(), blank);
            }
            KeyCode::Char('?') => self.place_on_board('?', true),
            KeyCode::Backspace => self.backspace_on_board(),
            KeyCode::Delete => self.remove_from_board(),
            KeyCode::Char(' ') => self.toggle_direction(),
            // Enter solves; moves are applied only from the list.
            KeyCode::Enter => self.find_moves_and_focus(),
            _ => {}
        }
    }

    pub(super) fn handle_rack_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.find_moves_and_focus(),
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                self.add_to_rack(c.to_ascii_uppercase());
            }
            KeyCode::Char('?') => {
                self.add_to_rack('?');
            }
            KeyCode::Backspace => self.pop_from_rack(),
            KeyCode::Delete => self.clear_rack(),
            _ => {}
        }
    }

    pub(super) fn handle_suggestions_key(&mut self, key: KeyEvent) {
        let page = self.list_height.get().saturating_sub(1).max(1) as i32;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_suggestion_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_suggestion_selection(1),
            KeyCode::PageUp => self.step_selection(-page),
            KeyCode::PageDown => self.step_selection(page),
            KeyCode::Home => self.step_selection(i32::MIN / 2),
            KeyCode::End => self.step_selection(i32::MAX / 2),
            KeyCode::Char(d @ '1'..='9') => {
                let row = self.list_offset.get() + (d as usize - '1' as usize);
                if let Some(&idx) = self.visible_suggestions().get(row) {
                    self.selected_suggestion = idx;
                }
            }
            KeyCode::Char('/') => {
                self.filter_editing = true;
                self.filter.get_or_insert_with(String::new);
            }
            KeyCode::Char('n') => self.move_suggestion_selection(1),
            KeyCode::Char('N') => self.move_suggestion_selection(-1),
            KeyCode::Enter => self.apply_suggestion_and_focus(),
            KeyCode::Char('s') | KeyCode::Char('S') => self.cycle_sort_mode(),
            KeyCode::Char('m') | KeyCode::Char('M') => self.run_mcts_evaluation(),
            KeyCode::Char('y') => self.copy_selected_move(),
            KeyCode::Char('Y') => self.copy_position(),
            _ => {}
        }
    }

    /// Keys while typing a suggestions filter: letters and digits narrow
    /// the list as they are typed, Enter keeps the filter, Esc drops it.
    pub(super) fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) if c.is_ascii_alphanumeric() => {
                self.filter.get_or_insert_with(String::new).push(c);
                self.select_first_visible();
            }
            KeyCode::Backspace => {
                if let Some(f) = self.filter.as_mut() {
                    f.pop();
                }
                self.select_first_visible();
            }
            KeyCode::Enter => {
                self.filter_editing = false;
                if self.filter.as_deref() == Some("") {
                    self.filter = None;
                }
            }
            KeyCode::Esc => {
                self.filter_editing = false;
                self.filter = None;
            }
            _ => {}
        }
    }

    pub(super) fn handle_dialog_key(&mut self, key: KeyEvent) {
        match self.dialog {
            DialogState::None => {}

            DialogState::Help => {
                let max = self.help_max_scroll.get();
                match key.code {
                    KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('q') => self.close_dialog(),
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.help_scroll = self.help_scroll.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.help_scroll = (self.help_scroll + 1).min(max)
                    }
                    KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                    KeyCode::PageDown => self.help_scroll = (self.help_scroll + 10).min(max),
                    KeyCode::Home => self.help_scroll = 0,
                    _ => {}
                }
            }

            DialogState::EscapeMenu(selected) => match key.code {
                KeyCode::Up => {
                    self.dialog = DialogState::EscapeMenu(selected.prev());
                }
                KeyCode::Down => {
                    self.dialog = DialogState::EscapeMenu(selected.next());
                }
                KeyCode::Esc => {
                    self.close_dialog();
                }
                KeyCode::Enter => self.choose_menu_option(selected),
                _ => {}
            },

            DialogState::ConfirmQuit { yes: yes_selected } => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    self.dialog = DialogState::ConfirmQuit { yes: !yes_selected };
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.should_quit = true;
                    self.close_dialog();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.close_dialog();
                }
                KeyCode::Enter => {
                    if yes_selected {
                        self.should_quit = true;
                    }
                    self.close_dialog();
                }
                _ => {}
            },

            DialogState::ConfirmReset { yes: yes_selected } => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    self.dialog = DialogState::ConfirmReset { yes: !yes_selected };
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.close_dialog();
                    self.reset();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.close_dialog();
                }
                KeyCode::Enter => {
                    if yes_selected {
                        self.close_dialog();
                        self.reset();
                    } else {
                        self.close_dialog();
                    }
                }
                _ => {}
            },

            DialogState::GamePicker { selected, startup } => {
                self.handle_picker_key(key, selected, startup)
            }

            DialogState::SaveAs { .. } => self.handle_save_as_key(key),

            DialogState::ConfirmOverwrite { yes, .. } => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    if let DialogState::ConfirmOverwrite { ref mut yes, .. } = self.dialog {
                        *yes = !*yes;
                    }
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => self.answer_overwrite(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.answer_overwrite(false)
                }
                KeyCode::Enter => self.answer_overwrite(yes),
                _ => {}
            },

            DialogState::ConfirmDelete { yes, .. } => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    if let DialogState::ConfirmDelete { ref mut yes, .. } = self.dialog {
                        *yes = !*yes;
                    }
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => self.answer_delete(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.answer_delete(false),
                KeyCode::Enter => self.answer_delete(yes),
                _ => {}
            },

            DialogState::QuickEntry { ref mut buffer } => match key.code {
                KeyCode::Char(c) if c.is_ascii_alphanumeric() || c == ' ' || c == '?' => {
                    if buffer.len() < 24 {
                        buffer.push(c);
                    }
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Enter => {
                    let text = buffer.clone();
                    self.commit_quick_entry(&text);
                }
                KeyCode::Esc => self.close_dialog(),
                _ => {}
            },

            DialogState::ScoreInput {
                ref target,
                ref mut buffer,
            } => match key.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    if buffer.len() < 4 {
                        buffer.push(c);
                    }
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Enter => {
                    let value: u32 = buffer.parse().unwrap_or(0);
                    let target = *target;
                    self.commit_score_input(target, value);
                }
                KeyCode::Esc => {
                    self.close_dialog();
                }
                _ => {}
            },
        }
    }

    /// Indices of the suggestions the list shows (all of them, or those
    /// matching the filter by word or by position).
    pub fn visible_suggestions(&self) -> Vec<usize> {
        let Some(filter) = self.filter.as_deref().filter(|f| !f.is_empty()) else {
            return (0..self.suggestions.len()).collect();
        };
        let q = filter.as_bytes();
        self.suggestions
            .iter()
            .enumerate()
            .filter(|(_, m)| contains_ignore_case(m.word.as_bytes(), q) || coords_start_with(m, q))
            .map(|(i, _)| i)
            .collect()
    }

    /// Select the first suggestion the filter lets through.
    pub(super) fn select_first_visible(&mut self) {
        if let Some(&first) = self.visible_suggestions().first() {
            self.selected_suggestion = first;
        }
        self.list_offset.set(0);
    }

    /// Move the selection by `delta` rows of the visible list, wrapping
    /// around (arrow keys, n/N).
    pub fn move_suggestion_selection(&mut self, delta: i32) {
        let visible = self.visible_suggestions();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|&i| i == self.selected_suggestion)
            .unwrap_or(0) as i32;
        let new = (pos + delta).rem_euclid(visible.len() as i32) as usize;
        self.selected_suggestion = visible[new];
    }

    /// Move the selection by `delta` rows of the visible list, stopping at
    /// the ends (paging, Home/End, the mouse wheel).
    pub fn step_selection(&mut self, delta: i32) {
        let visible = self.visible_suggestions();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|&i| i == self.selected_suggestion)
            .unwrap_or(0) as i64;
        let new = (pos + delta as i64).clamp(0, visible.len() as i64 - 1) as usize;
        self.selected_suggestion = visible[new];
    }

    /// Cycle input focus to the next pane.
    pub fn next_focus(&mut self) {
        self.focus = self.focus.next_visible(|f| self.pane_visible(f));
    }

    /// Cycle input focus to the previous pane (Shift+Tab).
    pub fn prev_focus(&mut self) {
        self.focus = self.focus.prev_visible(|f| self.pane_visible(f));
    }

    /// Whether the current layout shows the pane for `focus` (always true
    /// before the first frame, when the terminal size is unknown).
    pub fn pane_visible(&self, focus: Focus) -> bool {
        let Some((w, h)) = self.terminal_size else {
            return true;
        };
        let layout = crate::ui::layout::AppLayout::compute(
            ratatui::layout::Rect::new(0, 0, w, h),
            self.sidebar_visible,
        );
        match focus {
            Focus::Board => true,
            Focus::Rack => layout.shows_rack(),
            Focus::Suggestions => layout.suggestions_area.is_some(),
        }
    }

    /// Move focus to the board if the focused pane is no longer shown.
    pub fn ensure_focus_visible(&mut self) {
        if !self.pane_visible(self.focus) {
            self.focus = Focus::Board;
        }
    }

    /// Pasted text (bracketed paste): into the coordinate prompt when it is
    /// open, otherwise read as a CGP position.
    pub fn handle_paste(&mut self, text: &str) -> AppAction {
        if let DialogState::QuickEntry { buffer } = &mut self.dialog {
            buffer.push_str(text.trim());
            return AppAction::Redraw;
        }
        if let DialogState::SaveAs {
            buffer, pristine, ..
        } = &mut self.dialog
        {
            if *pristine {
                buffer.clear();
                *pristine = false;
            }
            buffer.extend(text.trim().chars().filter(|c| !c.is_control()).take(40));
            return AppAction::Redraw;
        }
        if matches!(
            self.dialog,
            DialogState::GamePicker { .. }
                | DialogState::ConfirmOverwrite { .. }
                | DialogState::ConfirmDelete { .. }
        ) {
            return AppAction::Nothing;
        }
        match self.load_position(text, /* undoable= */ true) {
            Ok(()) => self.set_message(MessageKind::Success, "Loaded the pasted position"),
            Err(e) => self.set_message(
                MessageKind::Error,
                format!("Pasted text is not a CGP position: {}", e),
            ),
        }
        AppAction::Redraw
    }
}

/// True if `needle` occurs in `hay`, ignoring ASCII case.
fn contains_ignore_case(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty()
        || hay
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle))
}

/// True if the move's coordinates ([`Move::coords`]) start with `prefix`,
/// ignoring ASCII case. The filter runs over every suggestion on every
/// frame, so this writes the coordinates into a stack buffer instead of a
/// new string.
fn coords_start_with(mv: &Move, prefix: &[u8]) -> bool {
    use std::io::Write;
    let mut buf = [0u8; 8];
    let len = {
        let mut w = &mut buf[..];
        let col = (b'A' + mv.col as u8) as char;
        let _ = if mv.kind != MoveKind::Play {
            write!(w, "-")
        } else if mv.direction == Direction::Horizontal {
            write!(w, "{}{}", mv.row + 1, col)
        } else {
            write!(w, "{}{}", col, mv.row + 1)
        };
        8 - w.len()
    };
    buf[..len]
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}
