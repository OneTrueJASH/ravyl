//! Binary entry point for the Crossplay solver TUI.
//!
//! Sets up the terminal, loads the dictionary, and runs the main event loop.

use std::io;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;

use cpsolver2::app::{self, App};
use cpsolver2::ui::events::AppAction;
use cpsolver2::{datafiles, dictionary, logging, settings, ui};

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

Usage: cpsolver2 [--data-dir <DIR>]

Options:
  --data-dir <DIR>  Directory holding data/words.fst and external/refengine/data/
  -h, --help        Print this help
  -V, --version     Print the version

Data is looked for in $CPSOLVER_DATA_DIR, then --data-dir, then the user
data directory (cpsolver2/), then <exe>/../share/cpsolver2, then the
working directory.
";

/// What the command line asks for.
enum Cli {
    Run,
    Exit(ExitCode),
}

/// Parse the (few) command-line options.
fn parse_args(args: impl Iterator<Item = String>) -> Cli {
    let mut args = args.peekable();
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
            other => {
                eprintln!("cpsolver2: unknown argument '{other}'\n\n{USAGE}");
                return Cli::Exit(ExitCode::from(2));
            }
        }
    }
    Cli::Run
}

fn main() -> ExitCode {
    if let Cli::Exit(code) = parse_args(std::env::args().skip(1)) {
        return code;
    }
    install_panic_hook();

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

    let res = run_tui(settings, settings_warning, dict);
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
) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // A frame before the rest of startup (RefEngine, the saved session), so
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
    app.restore_session();
    if let Some(warning) = datafiles::lfs_warning() {
        app.set_message(app::MessageKind::Error, warning);
    }
    if let Some(warning) = settings_warning {
        app.set_message(app::MessageKind::Error, warning);
    }
    let res = run_app(&mut terminal, &mut app);
    app.autosave();
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

fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    // Draw only when something changed: an input event, a result arriving,
    // or (while an evaluation runs) the spinner.
    let mut dirty = true;
    loop {
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

        let timeout = if app.is_mcts_running() {
            Duration::from_millis(100)
        } else if app.has_background_work() {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        };

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
