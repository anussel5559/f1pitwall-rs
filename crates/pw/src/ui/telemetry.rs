use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph},
};

use super::style::{clock_style, format_clock_info, team_color};
use crate::app::AppState;
use f1core::telemetry::SharedTelemetry;

const SPEED_COLOR: Color = Color::Cyan;
const THROTTLE_COLOR: Color = Color::Green;
const GEAR_COLOR: Color = Color::Yellow;
const BRAKE_COLOR: Color = Color::Red;
const LAP_MARKER_COLOR: Color = Color::DarkGray;
const GEAR_GRID_COLOR: Color = Color::Indexed(236);

pub fn render_telemetry(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    telem: Option<&SharedTelemetry>,
) {
    let (_driver_number, acronym, _team, team_colour, lap_display) =
        if let crate::app::ViewMode::Telemetry { driver_number } = state.view_mode {
            let info = state.driver_info(driver_number);
            let (acr, _team, tc) =
                info.unwrap_or(("???".to_string(), String::new(), "FFFFFF".to_string()));
            (driver_number, acr, _team, tc, state.lap_display())
        } else {
            return;
        };

    let shared = match telem {
        Some(t) => t,
        None => return,
    };

    let ts = shared.lock().unwrap();

    if ts.data.is_empty() {
        render_loading(f, area, &acronym, &team_colour, &lap_display, state);
        return;
    }

    // Clone chart data while holding the lock, then drop it
    let speed_points = ts.speed_points.clone();
    let throttle_points = ts.throttle_points.clone();
    let brake_points = ts.brake_points.clone();
    let gear_points = ts.gear_points.clone();
    let lap_boundary_xs = ts.lap_boundary_xs.clone();
    let x_bounds = ts.x_bounds;
    let scroll_offset = ts.scroll_offset;
    drop(ts);

    // Visible lap boundaries within the window
    let visible_laps: Vec<(f64, i64)> = lap_boundary_xs
        .iter()
        .filter(|(x, _)| *x > x_bounds.0 && *x <= x_bounds.1)
        .copied()
        .collect();

    // Three charts: Speed (40%), Throttle+Brake (30%), Gear (30%)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ])
        .split(area);

    render_speed_chart(
        f,
        chunks[0],
        &speed_points,
        &visible_laps,
        x_bounds,
        &acronym,
        &team_colour,
        &lap_display,
        state,
        scroll_offset,
    );
    render_throttle_brake_chart(
        f,
        chunks[1],
        &throttle_points,
        &brake_points,
        &visible_laps,
        x_bounds,
        scroll_offset,
    );
    render_gear_chart(
        f,
        chunks[2],
        &gear_points,
        &visible_laps,
        x_bounds,
        scroll_offset,
    );
}

fn render_loading(
    f: &mut Frame,
    area: Rect,
    acronym: &str,
    team_colour: &str,
    lap_display: &str,
    _state: &AppState,
) {
    let tc = team_color(team_colour);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(tc))
        .title(format!(" {}  {}  Telemetry ", acronym, lap_display))
        .title_bottom(help_line(0.0));

    let loading = Paragraph::new("Loading telemetry data...")
        .style(Style::default().fg(Color::DarkGray))
        .block(block);

    f.render_widget(loading, area);
}

#[allow(clippy::too_many_arguments)]
fn render_speed_chart(
    f: &mut Frame,
    area: Rect,
    speed_points: &[(f64, f64)],
    visible_laps: &[(f64, i64)],
    x_bounds: (f64, f64),
    acronym: &str,
    team_colour: &str,
    lap_display: &str,
    state: &AppState,
    scroll_offset: f64,
) {
    let tc = team_color(team_colour);

    let lap_lines = make_lap_lines(visible_laps, 360.0);
    let mut datasets = vec![
        Dataset::default()
            .name("Speed")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(SPEED_COLOR))
            .data(speed_points),
    ];
    for line_data in &lap_lines {
        datasets.push(
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(LAP_MARKER_COLOR))
                .data(line_data),
        );
    }

    let scroll_indicator = if scroll_offset > 0.5 {
        format!("  [-{:.0}s]", scroll_offset)
    } else {
        String::new()
    };

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(tc))
                .title(Line::from(vec![
                    Span::raw(format!(" {}  {}  Speed (km/h)", acronym, lap_display)),
                    Span::styled(scroll_indicator, Style::default().fg(Color::Yellow)),
                    Span::raw(" "),
                ]))
                .title_bottom(
                    Line::from(Span::styled(format_clock_info(state), clock_style(state)))
                        .alignment(ratatui::layout::Alignment::Right),
                ),
        )
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([x_bounds.0, x_bounds.1])
                .labels(make_x_labels(x_bounds, scroll_offset)),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, 360.0])
                .labels(vec![Span::raw("0"), Span::raw("180"), Span::raw("360")]),
        );

    f.render_widget(chart, area);
    render_lap_labels(f, area, visible_laps, x_bounds, 3);
}

fn render_throttle_brake_chart(
    f: &mut Frame,
    area: Rect,
    throttle_points: &[(f64, f64)],
    brake_points: &[(f64, f64)],
    visible_laps: &[(f64, i64)],
    x_bounds: (f64, f64),
    scroll_offset: f64,
) {
    let lap_lines = make_lap_lines(visible_laps, 105.0);
    let mut datasets = vec![
        Dataset::default()
            .name("Throttle")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(THROTTLE_COLOR))
            .data(throttle_points),
        Dataset::default()
            .name("Brake")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(BRAKE_COLOR))
            .data(brake_points),
    ];
    for line_data in &lap_lines {
        datasets.push(
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(LAP_MARKER_COLOR))
                .data(line_data),
        );
    }

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Line::from(vec![
                    Span::raw(" "),
                    Span::styled("Throttle", Style::default().fg(THROTTLE_COLOR)),
                    Span::raw(" / "),
                    Span::styled("Brake", Style::default().fg(BRAKE_COLOR)),
                    Span::raw(" "),
                ])),
        )
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([x_bounds.0, x_bounds.1])
                .labels(make_x_labels(x_bounds, scroll_offset)),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, 105.0])
                .labels(vec![Span::raw("0"), Span::raw("50"), Span::raw("100")]),
        );

    f.render_widget(chart, area);
}

fn render_gear_chart(
    f: &mut Frame,
    area: Rect,
    gear_points: &[(f64, f64)],
    visible_laps: &[(f64, i64)],
    x_bounds: (f64, f64),
    scroll_offset: f64,
) {
    let lap_lines = make_lap_lines(visible_laps, 105.0);

    // Horizontal grid lines at each gear level (1-8)
    let gear_grid: Vec<Vec<(f64, f64)>> = (1..=8)
        .map(|g| {
            let y = g as f64 * 12.5;
            vec![(x_bounds.0, y), (x_bounds.1, y)]
        })
        .collect();

    // Grid lines first so gear data renders on top
    let mut datasets: Vec<Dataset> = Vec::new();
    for line_data in &gear_grid {
        datasets.push(
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(GEAR_GRID_COLOR))
                .data(line_data),
        );
    }
    for line_data in &lap_lines {
        datasets.push(
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(LAP_MARKER_COLOR))
                .data(line_data),
        );
    }
    datasets.push(
        Dataset::default()
            .name("Gear")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(GEAR_COLOR))
            .data(gear_points),
    );

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(" Gear ", Style::default().fg(GEAR_COLOR)))
                .title_bottom(help_line(scroll_offset)),
        )
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([x_bounds.0, x_bounds.1])
                .labels(make_x_labels(x_bounds, scroll_offset)),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, 105.0])
                .labels(vec![Span::raw("0"), Span::raw("4"), Span::raw("8")]),
        );

    f.render_widget(chart, area);
}

/// Build vertical line datasets for lap boundaries.
fn make_lap_lines(visible_laps: &[(f64, i64)], y_max: f64) -> Vec<Vec<(f64, f64)>> {
    visible_laps
        .iter()
        .map(|&(x, _)| vec![(x, 0.0), (x, y_max)])
        .collect()
}

/// Render lap number labels at the top of a chart area, positioned at each lap boundary.
fn render_lap_labels(
    f: &mut Frame,
    chart_area: Rect,
    visible_laps: &[(f64, i64)],
    x_bounds: (f64, f64),
    y_label_width: u16,
) {
    let x_range = x_bounds.1 - x_bounds.0;
    if x_range <= 0.0 || chart_area.width < y_label_width + 3 {
        return;
    }
    // Graph area within borders and after y-axis labels
    let graph_x = chart_area.x + 1 + y_label_width;
    let graph_w = chart_area.width - 2 - y_label_width;
    let label_y = chart_area.y + 1;

    for &(x, lap_num) in visible_laps {
        let frac = (x - x_bounds.0) / x_range;
        let screen_x = graph_x + (frac * graph_w as f64) as u16;
        let label = format!("L{}", lap_num);
        let label_len = label.len() as u16;
        // Don't render if it would overflow the chart
        if screen_x + label_len > chart_area.right() - 1 {
            continue;
        }
        let label_area = Rect::new(screen_x, label_y, label_len, 1);
        let widget = Paragraph::new(label).style(Style::default().fg(Color::White));
        f.render_widget(widget, label_area);
    }
}

fn help_line(scroll_offset: f64) -> Line<'static> {
    let mut spans = vec![
        Span::styled("t", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(":close "),
        Span::styled("j/k", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(":driver "),
        Span::styled("h/l", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(":scroll "),
        Span::styled(
            "\u{2190}\u{2192}",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(":seek "),
        Span::styled("p", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(":pause "),
    ];
    if scroll_offset > 0.5 {
        spans.push(Span::styled(
            "0",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(":live "));
    }
    Line::from(spans).alignment(ratatui::layout::Alignment::Right)
}

fn make_x_labels(x_bounds: (f64, f64), scroll_offset: f64) -> Vec<Span<'static>> {
    let window = x_bounds.1 - x_bounds.0;
    if window <= 0.0 {
        return vec![Span::raw("0s")];
    }
    // Labels relative to the live edge (now). Right edge = -scroll_offset.
    let right = -scroll_offset;
    let left = right - window;
    let mid = right - window / 2.0;
    vec![
        Span::raw(format!("{:.0}s", left)),
        Span::raw(format!("{:.0}s", mid)),
        Span::raw(if scroll_offset < 0.5 {
            "0s".to_string()
        } else {
            format!("{:.0}s", right)
        }),
    ]
}
