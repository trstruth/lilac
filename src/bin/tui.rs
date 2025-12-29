use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{backend::CrosstermBackend, Terminal};

use lilac::tui::{self, AppState, KeyInput};

fn main() -> anyhow::Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    std::io::stdout()
        .execute(EnterAlternateScreen)
        .context("enter alternate screen")?;

    let result = run_app();

    std::io::stdout()
        .execute(LeaveAlternateScreen)
        .context("leave alternate screen")?;
    disable_raw_mode().context("disable raw mode")?;

    result
}

fn run_app() -> anyhow::Result<()> {
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).context("create terminal")?;

    let mut state = AppState::default();
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(33);

    loop {
        terminal.draw(|frame| tui::view(frame, &state))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::from_millis(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    break;
                }

                if key.code == KeyCode::Esc {
                    break;
                }

                if let Some(input) = map_key(key.code) {
                    let _ = state.handle_input(input);
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            state.tick();
            last_tick = Instant::now();
        }
    }

    Ok(())
}

fn map_key(code: KeyCode) -> Option<KeyInput> {
    match code {
        KeyCode::Char('q') => None,
        KeyCode::Char(ch) => Some(KeyInput::Char(ch)),
        KeyCode::Enter => Some(KeyInput::Enter),
        KeyCode::Backspace => Some(KeyInput::Backspace),
        KeyCode::Tab => Some(KeyInput::Tab),
        KeyCode::Up => Some(KeyInput::Up),
        KeyCode::Down => Some(KeyInput::Down),
        KeyCode::Esc => Some(KeyInput::Esc),
        _ => None,
    }
}
