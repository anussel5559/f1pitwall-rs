use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Block, Borders, Paragraph,
        canvas::{Canvas, Line as CanvasLine},
    },
};

use super::style::{clock_style, format_clock_info, team_color};
use crate::app::AppState;
use f1core::db::DriverLocation;
use f1core::domain::track;

pub fn render_track_map(f: &mut Frame, area: Rect, state: &AppState, locations: &[DriverLocation]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(10),   // map
            Constraint::Length(1), // footer/help
        ])
        .split(area);

    render_header(f, chunks[0], state);
    render_map(f, chunks[1], state, locations);
    render_footer(f, chunks[2], state);
}

fn render_header(f: &mut Frame, area: Rect, state: &AppState) {
    let circuit = &state.session.circuit;
    let session_name = &state.session.session_name;
    let clock_info = format_clock_info(state);

    let title = format!(" {} {} ", circuit, session_name);
    let selected_count = state.selected_drivers.len();

    let spans = vec![
        Span::styled(title, Style::default().fg(Color::White)),
        Span::styled(
            format!(
                " [{} driver{}] ",
                selected_count,
                if selected_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(clock_info, clock_style(state)),
    ];

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_map(f: &mut Frame, area: Rect, state: &AppState, locations: &[DriverLocation]) {
    let outline = track::get_track_outline(&state.session.circuit);

    if outline.is_none() && locations.is_empty() {
        let msg = if state.selected_drivers.is_empty() {
            "Select drivers with Space in board view, then press m"
        } else {
            "No track data available"
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Track Map ");
        let p = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        f.render_widget(p, area);
        return;
    }

    let (norm_outline, norm_locations) = match &outline {
        Some(outline) => {
            let bb = outline.bounding_box();
            let (pts, _aspect) = outline.normalize();
            let locs: Vec<(f64, f64, i64)> = locations
                .iter()
                .map(|loc| {
                    let (nx, ny) = match &bb {
                        Some(bb) => bb.normalize_point(loc.x, loc.y),
                        None => (0.5, 0.5),
                    };
                    (nx, ny, loc.driver_number)
                })
                .collect();
            (pts, locs)
        }
        None => (Vec::new(), Vec::new()),
    };

    // Map coordinates: normalized 0..1, we use canvas bounds 0..100 for readability
    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Track Map "),
        )
        .marker(Marker::Braille)
        .x_bounds([-5.0, 105.0])
        .y_bounds([-5.0, 105.0])
        .paint(move |ctx| {
            // Draw track outline
            if norm_outline.len() >= 2 {
                for i in 0..norm_outline.len() {
                    let (x1, y1) = norm_outline[i];
                    let (x2, y2) = norm_outline[(i + 1) % norm_outline.len()];
                    ctx.draw(&CanvasLine {
                        x1: x1 * 100.0,
                        y1: (1.0 - y1) * 100.0, // flip Y for canvas (origin bottom-left)
                        x2: x2 * 100.0,
                        y2: (1.0 - y2) * 100.0,
                        color: Color::Indexed(240), // subtle gray
                    });
                }
            }

            // Draw driver positions
            for &(nx, ny, driver_number) in &norm_locations {
                let color = driver_color(state, driver_number);
                let cx = nx * 100.0;
                let cy = (1.0 - ny) * 100.0;

                // Small marker dot
                ctx.draw(&ratatui::widgets::canvas::Points {
                    coords: &[(cx, cy)],
                    color,
                });

                // Label with driver acronym
                let acronym = driver_acronym(state, driver_number);
                ctx.print(
                    cx + 2.0,
                    cy + 1.0,
                    Span::styled(acronym, Style::default().fg(color)),
                );
            }
        });

    f.render_widget(canvas, area);
}

fn render_footer(f: &mut Frame, area: Rect, _state: &AppState) {
    let spans = vec![
        Span::styled(" m", Style::default().fg(Color::Yellow)),
        Span::styled(" back  ", Style::default().fg(Color::DarkGray)),
        Span::styled("←/→", Style::default().fg(Color::Yellow)),
        Span::styled(" seek  ", Style::default().fg(Color::DarkGray)),
        Span::styled("p", Style::default().fg(Color::Yellow)),
        Span::styled(" pause  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Space", Style::default().fg(Color::Yellow)),
        Span::styled(
            " toggle driver (in board)  ",
            Style::default().fg(Color::DarkGray),
        ),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn driver_color(state: &AppState, driver_number: i64) -> Color {
    state
        .driver_info(driver_number)
        .map(|(_, _, tc)| team_color(&tc))
        .unwrap_or(Color::White)
}

fn driver_acronym(state: &AppState, driver_number: i64) -> String {
    state
        .driver_info(driver_number)
        .map(|(acr, _, _)| acr)
        .unwrap_or_else(|| format!("#{}", driver_number))
}
