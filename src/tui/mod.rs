use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph},
};

pub static FIRE_PALETTE: [Color; 36] = [
    Color::from_u32(0x00000000),
    Color::from_u32(0x000D0000),
    Color::from_u32(0x001B0000),
    Color::from_u32(0x00280000),
    Color::from_u32(0x00350000),
    Color::from_u32(0x00430000),
    Color::from_u32(0x00500000),
    Color::from_u32(0x005D0000),
    Color::from_u32(0x006B0000),
    Color::from_u32(0x00780000),
    Color::from_u32(0x00870900),
    Color::from_u32(0x00961200),
    Color::from_u32(0x00A51B00),
    Color::from_u32(0x00B42400),
    Color::from_u32(0x00C32C00),
    Color::from_u32(0x00D23500),
    Color::from_u32(0x00E13E00),
    Color::from_u32(0x00F04700),
    Color::from_u32(0x00FF5000),
    Color::from_u32(0x00FF5D00),
    Color::from_u32(0x00FF6B00),
    Color::from_u32(0x00FF7800),
    Color::from_u32(0x00FF8500),
    Color::from_u32(0x00FF9300),
    Color::from_u32(0x00FFA000),
    Color::from_u32(0x00FFAD00),
    Color::from_u32(0x00FFBB00),
    Color::from_u32(0x00FFC800),
    Color::from_u32(0x00FFCF20),
    Color::from_u32(0x00FFD640),
    Color::from_u32(0x00FFDD60),
    Color::from_u32(0x00FFE480),
    Color::from_u32(0x00FFEA9F),
    Color::from_u32(0x00FFF1BF),
    Color::from_u32(0x00FFF8DF),
    Color::from_u32(0x00FFFFFF),
];

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

    pub fn draw_background(f: &mut Frame, tick: u64) {
        let area = f.area();
        let buf = f.buffer_mut();

        let source_index = FIRE_PALETTE.len().saturating_sub(6);
        // Seed the bottom row with a hot (but not max) color.
        for x in area.left()..area.right() {
            let rand = pseudo_rand(tick, x, area.bottom() - 1);
            let jitter = (rand & 1) as usize;
            let seed_index = source_index.saturating_sub(jitter);
            buf[(x, area.bottom() - 1)]
                .set_char('▒')
                .set_style(Style::default().fg(FIRE_PALETTE[seed_index]));
        }

        // Propagate upward by cooling slightly from the cell below.
        for y in (area.top()..area.bottom() - 1).rev() {
            for x in area.left()..area.right() {
                let rand = pseudo_rand(tick, x, y);
                let x_offset = (rand % 5) as i32 - 2;
                let sample_x = (x as i32 + x_offset)
                    .clamp(area.left() as i32, (area.right() - 1) as i32) as u16;
                let below = buf[(sample_x, y + 1)].style().fg;
                let below_index = palette_index(below.unwrap_or(Color::Black)).unwrap_or(0);
                let cool_step = match rand % 5 {
                    0 => 2,
                    1 => 1,
                    _ => 0,
                };
                let cooled = below_index.saturating_sub(cool_step);
                let target_index = if rand & 4 == 0 {
                    cooled
                } else {
                    (below_index * 2 + cooled) / 3
                };
                let current = buf[(x, y)].style().fg;
                let current_index = palette_index(current.unwrap_or(Color::Black)).unwrap_or(0);
                let next_index = if target_index < current_index {
                    current_index.saturating_sub(1)
                } else {
                    target_index
                };
                buf[(x, y)]
                    .set_char('▒')
                    .set_style(Style::default().fg(FIRE_PALETTE[next_index]));
            }
        }
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
    AppState::draw_background(frame, state.tick);
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

    let box_style = Style::default()
        .fg(Color::White)
        .bg(Color::from_u32(0x00333333));
    {
        let buf = frame.buffer_mut();
        for y in box_area.top()..box_area.bottom() {
            for x in box_area.left()..box_area.right() {
                buf[(x, y)].set_char(' ').set_style(box_style);
            }
        }
    }
    let block = centered_block(title).style(box_style);
    frame.render_widget(block.clone(), box_area);

    let masked = "*".repeat(state.password.len());

    let info = if let Some(message) = state.error_message.as_ref() {
        format!("Error: {message}")
    } else {
        "".to_string()
    };
    let paragraph = Paragraph::new(Text::from(vec![
        Line::styled(info, box_style),
        Line::styled(format!(" Username: {}", state.username), box_style),
        Line::styled("", box_style),
        Line::styled(format!(" Password: {}", masked), box_style),
    ]))
    .style(box_style);
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

fn palette_index(color: Color) -> Option<usize> {
    FIRE_PALETTE.iter().position(|entry| *entry == color)
}

fn pseudo_rand(tick: u64, x: u16, y: u16) -> u16 {
    let mut v = tick as u32 ^ ((x as u32) << 16) ^ (y as u32);
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^= v >> 16;
    (v & 0xFFFF) as u16
}
