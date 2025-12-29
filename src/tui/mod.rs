use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Position},
    widgets::{Block, Borders, Paragraph},
};

#[derive(Debug, Default)]
pub struct AppState {
    pub username: String,
    pub password: String,
    pub error_message: Option<String>,
    pub focused: FocusTarget,
    pub tick: u64,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    Username,
    Password,
}

impl Default for FocusTarget {
    fn default() -> Self {
        Self::Username
    }
}

impl AppState {
    pub fn handle_input(&mut self, key: KeyInput) -> Option<AppAction> {
        match key {
            KeyInput::Char(ch) => match self.focused {
                FocusTarget::Username => self.username.push(ch),
                FocusTarget::Password => self.password.push(ch),
            },
            KeyInput::Backspace => match self.focused {
                FocusTarget::Username => {
                    self.username.pop();
                }
                FocusTarget::Password => {
                    self.password.pop();
                }
            },
            KeyInput::Tab => {
                self.focused = match self.focused {
                    FocusTarget::Username => FocusTarget::Password,
                    FocusTarget::Password => FocusTarget::Username,
                };
            }
            KeyInput::Up => {
                self.focused = FocusTarget::Username;
            }
            KeyInput::Down => {
                self.focused = FocusTarget::Password;
            }
            KeyInput::Enter => {
                let username_empty = self.username.is_empty();
                let password_empty = self.password.is_empty();
                if username_empty && password_empty {
                    self.focused = FocusTarget::Username;
                } else if !username_empty && password_empty {
                    self.focused = FocusTarget::Password;
                } else if !username_empty && !password_empty {
                    return Some(AppAction::Submit {
                        username: self.username.clone(),
                        password: self.password.clone(),
                    });
                }
            }
            KeyInput::Esc => {
                // TODO: clear or cancel.
            }
        }
        None
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.saturating_add(1);
    }
}

#[derive(Debug, Clone)]
pub enum KeyInput {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Up,
    Down,
    Esc,
}

#[derive(Debug, Clone)]
pub enum AppAction {
    Submit { username: String, password: String },
}

pub fn render_to_buffer(state: &AppState, width_cells: u16, height_cells: u16) -> Buffer {
    let backend = TestBackend::new(width_cells, height_cells);
    let mut terminal = Terminal::new(backend).expect("failed to create ratatui terminal");
    let _ = terminal.draw(|frame| view(frame, state));
    terminal.backend().buffer().clone()
}

pub fn view(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let title = "Lilac";

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(36),
            Constraint::Min(0),
        ])
        .split(vert[1]);
    let box_area = horiz[1];

    let block = centered_block(title);
    frame.render_widget(block.clone(), box_area);

    let masked = "*".repeat(state.password.len());

    let info = if let Some(message) = state.error_message.as_ref() {
        format!("Error: {message}")
    } else {
        "".to_string()
    };
    let content = format!(
        "{info}\n Username: {}\n\n Password: {}",
        state.username, masked
    );

    let paragraph = Paragraph::new(content);
    let inner = block.inner(box_area);
    frame.render_widget(paragraph, inner);

    if let Some((x, y)) = cursor_position(inner, state) {
        frame.set_cursor_position(Position { x, y });
    }
}

fn centered_block(title: &str) -> Block<'_> {
    Block::default().title(title).borders(Borders::ALL)
}

fn cursor_position(inner: ratatui::layout::Rect, state: &AppState) -> Option<(u16, u16)> {
    let base_x = inner.x + 1;
    let user_label = "Username: ";
    let pass_label = "Password: ";
    let base_y = inner.y + 1;

    match state.focused {
        FocusTarget::Username => Some((
            base_x + user_label.len() as u16 + state.username.len() as u16,
            base_y,
        )),
        FocusTarget::Password => Some((
            base_x + pass_label.len() as u16 + state.password.len() as u16,
            base_y + 2,
        )),
    }
}
