use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use super::style::SessionTypeExt;
use f1core::db::SessionEntry;

pub struct PickerState {
    pub paused: Vec<SessionEntry>,
    pub sessions: Vec<SessionEntry>,
    pub selected: usize,
    pub total_items: usize,
    pub loading: bool,
    pub error: Option<String>,
    /// Which year is currently being browsed
    pub browse_year: i32,
    /// New version tag if an update is available (e.g. "v0.2.0")
    pub update_available: Option<String>,
    /// Whether the client is currently authenticated with OpenF1.
    pub authenticated: bool,
}

impl PickerState {
    pub fn new() -> Self {
        let year = chrono::Utc::now()
            .format("%Y")
            .to_string()
            .parse()
            .unwrap_or(2026);
        Self {
            paused: Vec::new(),
            sessions: Vec::new(),
            selected: 0,
            total_items: 0,
            loading: true,
            error: None,
            browse_year: year,
            update_available: None,
            authenticated: false,
        }
    }

    pub fn update_total(&mut self) {
        self.total_items = self.paused.len() + self.sessions.len();
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.total_items > 0 && self.selected < self.total_items - 1 {
            self.selected += 1;
        }
    }

    /// Returns the selected session entry, if any.
    pub fn selected_entry(&self) -> Option<&SessionEntry> {
        if self.selected < self.paused.len() {
            self.paused.get(self.selected)
        } else {
            self.sessions.get(self.selected - self.paused.len())
        }
    }
}

fn session_type_color(session_type: &str) -> Color {
    crate::session_types::SessionType::from_api_str(session_type)
        .map_or(Color::White, |t| t.color())
}

fn format_short_date(date_start: &str) -> String {
    if let Ok(dt) = date_start.parse::<chrono::DateTime<chrono::Utc>>() {
        dt.format("%b %d %H:%M").to_string()
    } else {
        date_start.to_string()
    }
}

fn format_group_header(entry: &SessionEntry) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {} ", entry.country),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("· {}", entry.circuit),
            Style::default().fg(Color::Gray),
        ),
    ])
}

fn format_session_line(entry: &SessionEntry) -> Line<'static> {
    let date = format_short_date(&entry.date_start);
    let tc = session_type_color(&entry.session_type);
    let mut spans = vec![
        Span::styled(
            format!("    {:<14}", date),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(entry.session_name.clone(), Style::default().fg(tc)),
    ];
    if entry.date_end.is_none() {
        spans.push(Span::styled(
            " LIVE",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// Build list items from sessions, grouped by meeting_key.
/// `selected_idx` is the global selectable index; `offset` is added to local index
/// to compute the global index for highlight comparison.
fn build_grouped_items(
    sessions: &[SessionEntry],
    selected_idx: usize,
    offset: usize,
) -> Vec<ListItem<'static>> {
    let mut items: Vec<ListItem<'static>> = Vec::new();
    let mut last_meeting: Option<i64> = None;

    for (i, entry) in sessions.iter().enumerate() {
        let global_idx = i + offset;

        // Insert group header when meeting changes
        if last_meeting != Some(entry.meeting_key) {
            last_meeting = Some(entry.meeting_key);
            items.push(ListItem::new(format_group_header(entry)));
        }

        let line = format_session_line(entry);
        let style = if global_idx == selected_idx {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        items.push(ListItem::new(line).style(style));
    }

    items
}

pub fn render_picker(f: &mut Frame, state: &PickerState) {
    let area = f.area();

    let update_height = if state.update_available.is_some() {
        4
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),             // header
            Constraint::Min(8),                // content
            Constraint::Length(3),             // help bar
            Constraint::Length(update_height), // update notice
        ])
        .split(area);

    // Header
    let title = vec![
        Line::from(vec![
            Span::styled(
                " F1",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " TUI",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![Span::styled(
            " Select a session to watch",
            Style::default().fg(Color::DarkGray),
        )]),
    ];
    let header = Paragraph::new(title);
    f.render_widget(header, chunks[0]);

    // Content — always render both sections
    render_session_lists(f, chunks[1], state);

    // Help bar
    let mut help_spans = vec![
        Span::styled(" ↑↓/jk", Style::default().fg(Color::Cyan)),
        Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
        Span::styled("←→/hl", Style::default().fg(Color::Cyan)),
        Span::styled(" year  ", Style::default().fg(Color::DarkGray)),
    ];

    if state.authenticated {
        help_spans.push(Span::styled("d", Style::default().fg(Color::Cyan)));
        help_spans.push(Span::styled(
            " logout  ",
            Style::default().fg(Color::DarkGray),
        ));
        help_spans.push(Span::styled(
            "[Authenticated]",
            Style::default().fg(Color::Green),
        ));
    } else {
        help_spans.push(Span::styled("a", Style::default().fg(Color::Cyan)));
        help_spans.push(Span::styled(
            " login  ",
            Style::default().fg(Color::DarkGray),
        ));
    }

    help_spans.push(Span::styled("  q", Style::default().fg(Color::Cyan)));
    help_spans.push(Span::styled(" quit", Style::default().fg(Color::DarkGray)));

    let help = Paragraph::new(Line::from(help_spans)).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(help, chunks[2]);

    // Update notice
    if let Some(ref version) = state.update_available {
        let notice_area = Rect {
            y: chunks[3].y + 1,
            height: chunks[3].height.saturating_sub(1),
            ..chunks[3]
        };
        let notice = Paragraph::new(Line::from(vec![
            Span::styled(" Update available: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                version.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Run 'pw --update' to upgrade",
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );
        f.render_widget(notice, notice_area);
    }
}

fn render_session_lists(f: &mut Frame, area: Rect, state: &PickerState) {
    let has_paused = !state.paused.is_empty();

    // Count visual rows for paused section (sessions + group headers + border)
    let paused_groups = if has_paused {
        let mut count = 0i64;
        let mut last = None;
        for e in &state.paused {
            if last != Some(e.meeting_key) {
                count += 1;
                last = Some(e.meeting_key);
            }
        }
        count as u16
    } else {
        0
    };

    let constraints = if has_paused {
        vec![
            Constraint::Length((state.paused.len() as u16 + paused_groups + 2).min(12)),
            Constraint::Min(6),
        ]
    } else {
        vec![Constraint::Min(6)]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut chunk_idx = 0;

    // Paused sessions section
    if has_paused {
        let items = build_grouped_items(&state.paused, state.selected, 0);

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    " Paused Sessions ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(Color::Yellow)),
        );
        f.render_widget(list, chunks[chunk_idx]);
        chunk_idx += 1;
    }

    // Browse sessions section
    let year_block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" ◀ {} Sessions ▶ ", state.browse_year),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::DarkGray));

    if state.loading {
        let loading = Paragraph::new("  Loading sessions...")
            .style(Style::default().fg(Color::DarkGray))
            .block(year_block);
        f.render_widget(loading, chunks[chunk_idx]);
    } else if let Some(ref err) = state.error {
        let error = Paragraph::new(format!("  Error: {}", err))
            .style(Style::default().fg(Color::Red))
            .block(year_block);
        f.render_widget(error, chunks[chunk_idx]);
    } else {
        let items = build_grouped_items(&state.sessions, state.selected, state.paused.len());
        let list = List::new(items).block(year_block);
        f.render_widget(list, chunks[chunk_idx]);
    }
}
