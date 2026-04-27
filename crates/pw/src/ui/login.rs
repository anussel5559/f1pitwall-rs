use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub struct LoginState {
    pub username: String,
    pub password: String,
    pub cursor: usize,
    pub focused_field: LoginField,
    pub error: Option<String>,
    pub submitting: bool,
}

#[derive(PartialEq)]
pub enum LoginField {
    Username,
    Password,
}

impl LoginState {
    pub fn new() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            cursor: 0,
            focused_field: LoginField::Username,
            error: None,
            submitting: false,
        }
    }

    pub fn active_buf(&self) -> &str {
        match self.focused_field {
            LoginField::Username => &self.username,
            LoginField::Password => &self.password,
        }
    }

    fn active_buf_mut(&mut self) -> &mut String {
        match self.focused_field {
            LoginField::Username => &mut self.username,
            LoginField::Password => &mut self.password,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let cursor = self.cursor;
        let buf = self.active_buf_mut();
        if cursor >= buf.len() {
            buf.push(c);
        } else {
            buf.insert(cursor, c);
        }
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let cursor = self.cursor;
            self.active_buf_mut().remove(cursor);
        }
    }

    pub fn delete(&mut self) {
        let cursor = self.cursor;
        let buf = self.active_buf_mut();
        if cursor < buf.len() {
            buf.remove(cursor);
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        let len = self.active_buf().len();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.active_buf().len();
    }

    pub fn toggle_field(&mut self) {
        match self.focused_field {
            LoginField::Username => {
                self.focused_field = LoginField::Password;
                self.cursor = self.password.len();
            }
            LoginField::Password => {
                self.focused_field = LoginField::Username;
                self.cursor = self.username.len();
            }
        }
    }
}

pub fn render_login(f: &mut Frame, state: &LoginState) {
    let area = f.area();

    // Center the login form
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(14),
            Constraint::Min(0),
        ])
        .split(area);
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(50.min(area.width)),
            Constraint::Min(0),
        ])
        .split(vert[1]);
    let form_area = horiz[1];

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(3), // username
            Constraint::Length(3), // password
            Constraint::Length(1), // error
            Constraint::Length(3), // help
        ])
        .split(form_area);

    // Title
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " OpenF1 ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Login",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    f.render_widget(title, chunks[0]);

    // Username field
    let user_focused = state.focused_field == LoginField::Username;
    let user_border_color = if user_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let user_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(user_border_color))
        .title(Span::styled(
            " Username ",
            Style::default().fg(user_border_color),
        ));
    let user_text = Paragraph::new(state.username.as_str()).block(user_block);
    f.render_widget(user_text, chunks[1]);
    if user_focused && !state.submitting {
        f.set_cursor_position((chunks[1].x + 1 + state.cursor as u16, chunks[1].y + 1));
    }

    // Password field
    let pass_focused = state.focused_field == LoginField::Password;
    let pass_border_color = if pass_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let pass_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(pass_border_color))
        .title(Span::styled(
            " Password ",
            Style::default().fg(pass_border_color),
        ));
    let masked: String = "\u{2022}".repeat(state.password.len()); // bullet chars
    let pass_text = Paragraph::new(masked.as_str()).block(pass_block);
    f.render_widget(pass_text, chunks[2]);
    if pass_focused && !state.submitting {
        f.set_cursor_position((chunks[2].x + 1 + state.cursor as u16, chunks[2].y + 1));
    }

    // Error message
    if let Some(ref err) = state.error {
        let error = Paragraph::new(Span::styled(
            format!(" {err}"),
            Style::default().fg(Color::Red),
        ));
        f.render_widget(error, chunks[3]);
    } else if state.submitting {
        let msg = Paragraph::new(Span::styled(
            " Authenticating...",
            Style::default().fg(Color::Yellow),
        ));
        f.render_widget(msg, chunks[3]);
    }

    // Help bar
    let help = Paragraph::new(Line::from(vec![
        Span::styled(" Tab", Style::default().fg(Color::Cyan)),
        Span::styled(" switch  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::styled(" submit  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Cyan)),
        Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
    ]))
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(help, chunks[4]);
}
