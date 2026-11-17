//! Binary entry point for the Crossplay solver TUI.
//!
//! Sets up the terminal, loads the dictionary, and runs the main event loop.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::{event, execute};
use ratatui::prelude::*;

use cpsolver2::app::{self, App};
use cpsolver2::ui::events::AppAction;
use cpsolver2::{datafiles, dictionary, games, logging, settings, ui};

/// Put the terminal back the way we found it. Safe to call more than once.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        DisableMouseCapture,
        Show
    );
}

/// Restores the terminal when dropped, so every exit path from the TUI
/// (including an error from `Terminal::new` or the event loop) leaves the
/// user's terminal usable.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        // From here on the guard owns the cleanup, even if the next call
        // fails half-way.
        let guard = TerminalGuard;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Panics on the main thread restore the terminal before the message is
/// printed, or it would be unreadable on the alternate screen. Panics on
/// worker threads go to the log instead: the TUI is still running, the
/// worker's caller reports the failure, and printing to stderr would
/// scribble over the screen.
fn install_panic_hook() {
    let main_thread = std::thread::current().id();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == main_thread {
            restore_terminal();
            default_hook(info);
        } else {
            logging::log(&format!(
                "Worker thread '{}' panicked: {}",
                std::thread::current().name().unwrap_or("unnamed"),
                info
            ));
        }
    }));
}

const USAGE: &str = "\
cpsolver2 - NYT Crossplay solver (terminal UI)

Usage: cpsolver2 [--game <NAME>] [--data-dir <DIR>]
       cpsolver2 --list

Options:
  --game <NAME>     Open the saved game NAME, or start a new game called NAME
  --list            List the saved games and exit
  --data-dir <DIR>  Directory holding data/words.fst and external/refengine/data/
  -h, --help        Print this help
  -V, --version     Print the version

Every game is saved as you play, one file per game in the user data
directory (cpsolver2/games/). Without --game the app starts with a list
of saved games to pick from, when there are any.

Data is looked for in --data-dir, then $CPSOLVER_DATA_DIR, then the user
data directory (cpsolver2/), then <exe>/../share/cpsolver2, then the
working directory.
";

/// What the command line asks for.
enum Cli {
    /// Run the TUI, opening (or creating) the named game if one is given.
    Run(Option<String>),
    List,
    Exit(ExitCode),
}

/// Parse the (few) command-line options.
fn parse_args(args: impl Iterator<Item = String>) -> Cli {
    let mut args = args.peekable();
    let mut game = None;
    let mut list = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Cli::Exit(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("cpsolver2 {}", env!("CARGO_PKG_VERSION"));
                return Cli::Exit(ExitCode::SUCCESS);
            }
            "--data-dir" => match args.next() {
                Some(dir) => datafiles::set_cli_data_dir(dir.into()),
                None => {
                    eprintln!("cpsolver2: --data-dir needs a directory\n\n{USAGE}");
                    return Cli::Exit(ExitCode::from(2));
                }
            },
            other if other.starts_with("--data-dir=") => {
                datafiles::set_cli_data_dir(other["--data-dir=".len()..].into());
            }
            "--game" => match args.next().filter(|n| !n.trim().is_empty()) {
                Some(name) => game = Some(name),
                None => {
                    eprintln!("cpsolver2: --game needs a name\n\n{USAGE}");
                    return Cli::Exit(ExitCode::from(2));
                }
            },
            other if other.starts_with("--game=") => {
                game = Some(other["--game=".len()..].to_string());
            }
            "--list" => list = true,
            other => {
                eprintln!("cpsolver2: unknown argument '{other}'\n\n{USAGE}");
                return Cli::Exit(ExitCode::from(2));
            }
        }
    }
    if list {
        Cli::List
    } else {
        Cli::Run(game)
    }
}

/// `--list`: the saved games, most recently played first. Read-only: it
/// takes no locks and leaves the old session file for the app to import.
fn list_games() -> ExitCode {
    let Some(dir) = games::default_dir() else {
        eprintln!("cpsolver2: no user data directory");
        return ExitCode::FAILURE;
    };
    let entries = games::list(&dir);
    if entries.is_empty() {
        println!("No saved games in {}", dir.display());
        if dir.with_file_name("session.cgp").is_file() {
            println!(
                "(the last session will be imported as \"{}\" at the next start)",
                games::RESTORED_NAME
            );
        }
        return ExitCode::SUCCESS;
    }
    let width = entries
        .iter()
        .map(|g| g.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(4);
    println!(
        "{:<width$}  {:>9}  {:>4}  {:>9}",
        "Game", "Score", "Bag", "Played"
    );
    let now = games::now_secs();
    for g in &entries {
        println!(
            "{:<width$}  {:>9}  {:>4}  {:>9}{}",
            g.name,
            g.score_text(),
            g.bag.map_or("-".to_string(), |b| b.to_string()),
            games::relative_time(g.updated, now),
            if g.locked {
                "  (open in another window)"
            } else {
                ""
            }
        );
    }
    ExitCode::SUCCESS
}

/// What the event loop and the signal watchdog share.
#[derive(Default)]
struct ExitState {
    /// Set by SIGHUP (the terminal closed), SIGTERM, or SIGINT sent from
    /// outside (in the TUI Ctrl+C is a key).
    signalled: AtomicBool,
    /// Set once the game has been saved on the way out.
    saved: AtomicBool,
    /// The open game as it would be saved now (`App::save_snapshot`),
    /// refreshed on every pass of the event loop.
    snapshot: Mutex<Option<(PathBuf, games::SavedGame)>>,
}

/// How long the watchdog gives the event loop to notice a signal and save
/// the game itself (it polls at least once a second).
#[cfg(unix)]
const SIGNAL_GRACE: Duration = Duration::from_millis(2500);

/// Handle SIGHUP, SIGTERM and SIGINT: instead of dying, set
/// `signalled`, which the event loop checks on every pass; it then saves
/// the game and restores the terminal. When the terminal has gone away,
/// crossterm's input reader can spin forever on the dead tty and the loop
/// never gets there, so a watchdog thread waits [`SIGNAL_GRACE`] and then
/// saves the last snapshot itself, releases the lock and exits.
fn watch_signals(exit: &Arc<ExitState>) {
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let mut signals = match signal_hook::iterator::Signals::new([SIGHUP, SIGTERM, SIGINT]) {
            Ok(signals) => signals,
            Err(e) => {
                logging::log(&format!("Could not handle signals: {}", e));
                return;
            }
        };
        let exit = Arc::clone(exit);
        std::thread::spawn(move || {
            let Some(signal) = signals.forever().next() else {
                return;
            };
            logging::log(&format!("Signal {} received: saving and exiting", signal));
            exit.signalled.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + SIGNAL_GRACE;
            while Instant::now() < deadline {
                if exit.saved.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let snapshot = exit.snapshot.lock().ok().and_then(|s| s.clone());
            if let Some((dir, mut game)) = snapshot {
                game.updated = games::now_secs();
                if let Err(e) = games::save(&dir, &game) {
                    logging::log(&format!("Saving {} on exit failed: {}", game.name, e));
                }
                games::unlock(&dir, &games::slug(&game.name));
            }
            logging::log("The event loop did not stop; the watchdog saved the game");
            restore_terminal();
            std::process::exit(0);
        });
    }
    #[cfg(not(unix))]
    let _ = exit;
}

fn main() -> ExitCode {
    let game = match parse_args(std::env::args().skip(1)) {
        Cli::Exit(code) => return code,
        Cli::List => return list_games(),
        Cli::Run(game) => game,
    };
    // A game open in another window is reported before the terminal is
    // taken over.
    if let (Some(dir), Some(name)) = (games::default_dir(), game.as_deref()) {
        if games::lock_holder(&dir, &games::slug(name)).is_some() {
            eprintln!("cpsolver2: {} is open in another window", name.trim());
            return ExitCode::FAILURE;
        }
    }
    let exit = Arc::new(ExitState::default());
    watch_signals(&exit);
    install_panic_hook();
    cpsolver2::logging::install_engine_bridge();

    // Data is loaded before the terminal is taken over, so a missing file
    // is reported plainly on stderr with a non-zero exit code.
    let dict = match dictionary::load_default_dictionary() {
        Ok(dict) => dict,
        Err(e) => {
            eprintln!("cpsolver2: failed to load the dictionary: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let (settings, settings_warning) = settings::Settings::load_or_recover();
    ui::theme::init(settings.display.theme);

    let res = run_tui(settings, settings_warning, dict, game, &exit);
    if let Err(err) = res {
        eprintln!("cpsolver2: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_tui(
    settings: settings::Settings,
    settings_warning: Option<String>,
    dict: dictionary::Dictionary,
    game: Option<String>,
    exit: &ExitState,
) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // A frame before the rest of startup (the saved games), so
    // the terminal never sits blank.
    terminal.draw(|f| {
        let area = f.area();
        let y = area.y + area.height / 2;
        f.render_widget(
            ratatui::widgets::Paragraph::new("Loading\u{2026}").centered(),
            Rect::new(area.x, y, area.width, 1),
        );
    })?;

    let mut app = App::with_settings(settings);
    app.set_dictionary(dict);
    app.games_dir = games::default_dir();
    if let Err(e) = app.start_session(game.as_deref()) {
        app.set_message(app::MessageKind::Error, e);
    }
    if let Some(warning) = datafiles::lfs_warning() {
        app.set_message(app::MessageKind::Error, warning);
    }
    if let Some(warning) = settings_warning {
        app.set_message(app::MessageKind::Error, warning);
    }
    // Whatever ended the loop (quit, a signal, or an error such as the
    // terminal going away), the game is saved and its lock released.
    let res = run_app(&mut terminal, &mut app, exit);
    app.shutdown();
    exit.saved.store(true, Ordering::SeqCst);
    res
}

/// Copy `text` to the system clipboard with an OSC 52 escape (works over
/// SSH in most terminals).
fn write_clipboard(text: &str) {
    use std::io::Write;
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", app::base64(text.as_bytes()));
    let _ = out.flush();
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, exit: &ExitState) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let res = event_loop(terminal, app, exit);
    // After a hangup the terminal is gone and drawing or reading fails;
    // that is the expected way out, not an error to report.
    if exit.signalled.load(Ordering::SeqCst) {
        return Ok(());
    }
    res
}

fn event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App, exit: &ExitState) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    // Draw only when something changed: an input event, a result arriving,
    // or (while an evaluation runs) the spinner.
    let mut dirty = true;
    loop {
        if exit.signalled.load(Ordering::SeqCst) {
            return Ok(());
        }
        if dirty {
            if let Ok(size) = terminal.size() {
                app.terminal_size = Some((size.width, size.height));
                app.ensure_focus_visible();
            }
            terminal.draw(|f| ui::frame::draw_ui(f, app))?;
            dirty = false;
        }
        if let Some(text) = app.pending_clipboard.take() {
            write_clipboard(&text);
        }

        if app.has_background_work() && app.poll_mcts() {
            dirty = true;
        }

        // Saved at most about once a second while the position changes;
        // the watchdog keeps the latest state in case the loop gets stuck.
        app.autosave_if_due(Instant::now());
        if let Ok(mut snapshot) = exit.snapshot.lock() {
            *snapshot = app.save_snapshot();
        }

        let timeout = if app.is_mcts_running() {
            Duration::from_millis(100)
        } else if app.has_background_work() {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        };
        // Wake up for a pending autosave.
        let timeout = app.autosave_due_in(Instant::now()).map_or(timeout, |due| {
            timeout.min(due.max(Duration::from_millis(10)))
        });

        if event::poll(timeout)? {
            // Handle every pending event before drawing again, so a burst
            // of keys or wheel ticks costs one frame.
            loop {
                let action = match event::read()? {
                    Event::Key(key) => app.handle_key(key),
                    Event::Mouse(mouse) => app.handle_mouse(mouse),
                    Event::Paste(text) => app.handle_paste(&text),
                    Event::Resize(..) => AppAction::Redraw,
                    _ => AppAction::Nothing,
                };
                match action {
                    AppAction::Quit => return Ok(()),
                    AppAction::Redraw => dirty = true,
                    AppAction::Nothing => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        } else if app.is_mcts_running() {
            // The spinner and the live progress line.
            dirty = true;
        }
    }
}
