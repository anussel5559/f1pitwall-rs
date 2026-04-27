use std::time::Duration;

use ratatui::{
    Frame,
    layout::Constraint,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Row, Table},
};

use super::style::{SessionTypeExt, clock_style, format_clock_info, team_color};
use crate::app::{AppState, BoardRows};
use f1core::db::BoardRow;
use f1core::domain::rules;
use f1core::domain::sector::{SectorStatus, classify_sector};

const PURPLE: Color = Color::Rgb(160, 32, 240);
const GREEN: Color = Color::Rgb(0, 220, 0);
const YELLOW: Color = Color::Rgb(200, 200, 0);
const DRS_COLOR: Color = Color::Rgb(255, 80, 80);
const FLASH_DURATION: Duration = Duration::from_secs(rules::SECTOR_FLASH_SECS);

fn compound_color(compound: &str) -> Color {
    match compound.to_uppercase().as_str() {
        "SOFT" => Color::Red,
        "MEDIUM" => Color::Yellow,
        "HARD" => Color::White,
        "INTERMEDIATE" => Color::Green,
        "WET" => Color::Blue,
        _ => Color::Gray,
    }
}

fn format_time(secs: Option<f64>) -> String {
    match secs {
        Some(t) if t > 0.0 => {
            let mins = (t / 60.0) as u64;
            let remainder = t - (mins as f64 * 60.0);
            if mins > 0 {
                format!("{}:{:06.3}", mins, remainder)
            } else {
                format!("{:.3}", remainder)
            }
        }
        _ => "---".to_string(),
    }
}

fn sector_style(
    value: Option<f64>,
    session_best: Option<f64>,
    personal_best: Option<f64>,
    flash: bool,
) -> Style {
    let status = classify_sector(value, session_best, personal_best);
    let mut style = match status {
        SectorStatus::SessionBest => Style::default().fg(PURPLE).add_modifier(Modifier::BOLD),
        SectorStatus::PersonalBest => Style::default().fg(GREEN),
        SectorStatus::Normal => Style::default().fg(YELLOW),
        SectorStatus::None => return Style::default().fg(Color::DarkGray),
    };
    if flash {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

/// Parse an interval string like "1.234" or "+1.234" into seconds.
fn parse_interval(s: &str) -> Option<f64> {
    let trimmed = s.trim().trim_start_matches('+');
    trimmed.parse::<f64>().ok()
}

/// Style for the interval column — red+bold if within DRS range.
fn interval_style(interval_str: &str) -> Style {
    if let Some(secs) = parse_interval(interval_str)
        && rules::is_drs_range(secs)
    {
        return Style::default().fg(DRS_COLOR).add_modifier(Modifier::BOLD);
    }
    Style::default()
}

/// Format position differential vs grid: "▲3" (gained), "▼2" (lost), "─" (same)
fn format_pos_diff(current: i64, grid: Option<i64>) -> (String, Style) {
    match grid {
        Some(g) if current > 0 && g > 0 => {
            let diff = g - current; // positive = gained positions
            if diff > 0 {
                (
                    format!("{:>2}", format!("▲{}", diff)),
                    Style::default().fg(Color::Green),
                )
            } else if diff < 0 {
                (
                    format!("{:>2}", format!("▼{}", -diff)),
                    Style::default().fg(Color::Red),
                )
            } else {
                (" ─".to_string(), Style::default().fg(Color::DarkGray))
            }
        }
        _ => ("  ".to_string(), Style::default()),
    }
}

fn driver_status(b: &BoardRow) -> String {
    if b.stopped {
        return "STOP".to_string();
    }
    if b.in_pit {
        return "PIT".to_string();
    }
    // Lap 1 is flagged as pit_out_lap by the API (cars leave pit lane to form the grid)
    // but it's not an actual pit stop — ignore it.
    if b.is_pit_out_lap && b.lap_number.is_some_and(|n| n > 1) {
        if b.pit_exit_confirmed {
            return "OUT".to_string();
        } else {
            return "PIT".to_string();
        }
    }
    String::new()
}

fn driver_status_style(b: &BoardRow) -> Style {
    if b.stopped {
        return Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    }
    if b.in_pit {
        return Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
    }
    if b.is_pit_out_lap && b.lap_number.is_some_and(|n| n > 1) {
        if b.pit_exit_confirmed {
            return Style::default().fg(Color::Yellow);
        } else {
            return Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
        }
    }
    Style::default()
}

/// Format tyre age with visual wear indicator
fn format_tyre(compound: &str, age: Option<i64>) -> (String, Style) {
    if compound.is_empty() {
        return ("---".to_string(), Style::default().fg(Color::DarkGray));
    }

    let initial = compound
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let age_val = age.unwrap_or(0);
    let color = compound_color(compound);

    let style = if rules::is_tyre_degraded(age_val) {
        Style::default().fg(color).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(color)
    };

    (format!("{}{:>2}", initial, age_val), style)
}

pub fn render_board(f: &mut Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    // 2 for borders + 1 for header row
    state.visible_rows = area.height.saturating_sub(3) as usize;

    use crate::session_types::SessionType;
    match state.session.session_type_enum {
        SessionType::Qualifying | SessionType::SprintQualifying | SessionType::Practice => {
            render_qualifying_board(f, area, state)
        }
        _ => render_race_board(f, area, state),
    }
}

fn render_race_board(f: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let header_cells = [
        "P", "+/-", "Driver", "Team", "Gap", "Int", "Last", "S1", "S2", "S3", "Tyre", "Pit", "",
    ]
    .iter()
    .map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1);

    let s = &state.session;
    let race_rows = match &s.rows {
        BoardRows::Race(rows) => rows,
        _ => return,
    };
    let rows: Vec<Row> = race_rows
        .iter()
        .enumerate()
        .skip(state.scroll_offset)
        .map(|(idx, r)| {
            let b = &r.board;
            let is_selected = idx == state.selected_index;
            let tc = team_color(&b.team_colour);
            let pb = s
                .driver_best_sectors
                .get(&b.driver_number)
                .cloned()
                .unwrap_or((None, None, None));

            let (pos_diff_str, pos_diff_style) = format_pos_diff(b.position, b.grid_position);
            // Show previous stint's tyres while in pit, new tyres once out
            let actual_pit_out = b.is_pit_out_lap && b.lap_number.is_some_and(|n| n > 1);
            let show_prev_tyre = !b.prev_compound.is_empty()
                && (b.in_pit || (actual_pit_out && !b.pit_exit_confirmed));
            let (tyre_str, tyre_style) = if show_prev_tyre {
                format_tyre(&b.prev_compound, b.prev_tyre_age)
            } else {
                format_tyre(&b.compound, b.tyre_age)
            };
            let int_str = format_gap(&b.interval);
            let int_style = interval_style(&b.interval);

            // Check flash state for each sector
            let flash_s1 = state
                .sector_flash
                .get(&(b.driver_number, 0))
                .is_some_and(|t| t.elapsed() < FLASH_DURATION);
            let flash_s2 = state
                .sector_flash
                .get(&(b.driver_number, 1))
                .is_some_and(|t| t.elapsed() < FLASH_DURATION);
            let flash_s3 = state
                .sector_flash
                .get(&(b.driver_number, 2))
                .is_some_and(|t| t.elapsed() < FLASH_DURATION);

            let cells = vec![
                Cell::from(format!("{:>2}", b.position)),
                Cell::from(Span::styled(pos_diff_str, pos_diff_style)),
                Cell::from(Line::from(vec![
                    if state.selected_drivers.contains(&b.driver_number) {
                        Span::styled("* ", Style::default().fg(Color::Cyan))
                    } else {
                        Span::raw("  ")
                    },
                    Span::styled(
                        &b.acronym,
                        Style::default().fg(tc).add_modifier(Modifier::BOLD),
                    ),
                ])),
                Cell::from(Span::styled(truncate(&b.team, 12), Style::default().fg(tc))),
                Cell::from(if b.position == 1 {
                    "LEADER".to_string()
                } else {
                    format_gap(&b.gap)
                }),
                Cell::from(Span::styled(int_str, int_style)),
                Cell::from(format_time(r.display_last_lap)),
                Cell::from(Span::styled(
                    format_sector(r.display_s1),
                    sector_style(r.display_s1, s.best_s1, pb.0, flash_s1),
                )),
                Cell::from(Span::styled(
                    format_sector(r.display_s2),
                    sector_style(r.display_s2, s.best_s2, pb.1, flash_s2),
                )),
                Cell::from(Span::styled(
                    format_sector(r.display_s3),
                    sector_style(r.display_s3, s.best_s3, pb.2, flash_s3),
                )),
                Cell::from(Span::styled(tyre_str, tyre_style)),
                Cell::from(format!("{}", b.pit_count)),
                Cell::from(Span::styled(driver_status(b), driver_status_style(b))),
            ];
            let row = Row::new(cells);
            if b.in_pit {
                row.style(Style::default().fg(Color::DarkGray))
            } else if is_selected {
                row.style(
                    Style::default()
                        .bg(Color::Rgb(60, 60, 80))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                row
            }
        })
        .collect();

    let title_left = format!(
        " {} - {} - {} - {} ",
        s.country,
        s.circuit,
        s.session_name,
        state.lap_display()
    );

    let title_right = format_clock_info(state);
    let border_color = s.session_type_enum.color();

    let widths = [
        Constraint::Length(3),  // P
        Constraint::Length(3),  // +/-
        Constraint::Length(5),  // Driver
        Constraint::Length(13), // Team
        Constraint::Length(8),  // Gap
        Constraint::Length(8),  // Int
        Constraint::Length(9),  // Last
        Constraint::Length(7),  // S1
        Constraint::Length(7),  // S2
        Constraint::Length(7),  // S3
        Constraint::Length(4),  // Tyre
        Constraint::Length(3),  // Pit
        Constraint::Length(3),  // Status
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title_left)
        .title_bottom(
            Line::from(Span::styled(title_right, clock_style(state)))
                .alignment(ratatui::layout::Alignment::Right),
        )
        .border_style(Style::default().fg(border_color));

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_widget(table, area);
}

fn best_lap_style(value: Option<f64>, session_best: Option<f64>) -> Style {
    let Some(v) = value else {
        return Style::default().fg(Color::DarkGray);
    };
    if session_best.is_some_and(|sb| (v - sb).abs() < 0.001) {
        Style::default().fg(PURPLE).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn render_qualifying_board(f: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let header_cells = [
        "P", "Driver", "Team", "Best", "Gap", "Last", "S1", "S2", "S3", "Tyre", "Laps", "",
    ]
    .iter()
    .map(|h| {
        Cell::from(*h).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    });
    let header = Row::new(header_cells).height(1);

    let s = &state.session;
    let quali_rows = match &s.rows {
        BoardRows::Qualifying(rows) => rows,
        _ => return,
    };
    let rows: Vec<Row> = quali_rows
        .iter()
        .enumerate()
        .skip(state.scroll_offset)
        .map(|(idx, r)| {
            let b = &r.board;
            let is_selected = idx == state.selected_index;
            let tc = team_color(&b.team_colour);
            let pb = s
                .driver_best_sectors
                .get(&b.driver_number)
                .cloned()
                .unwrap_or((None, None, None));

            let (tyre_str, tyre_style) = format_tyre(&b.compound, b.tyre_age);

            let flash_s1 = state
                .sector_flash
                .get(&(b.driver_number, 0))
                .is_some_and(|t| t.elapsed() < FLASH_DURATION);
            let flash_s2 = state
                .sector_flash
                .get(&(b.driver_number, 1))
                .is_some_and(|t| t.elapsed() < FLASH_DURATION);
            let flash_s3 = state
                .sector_flash
                .get(&(b.driver_number, 2))
                .is_some_and(|t| t.elapsed() < FLASH_DURATION);

            let cells = vec![
                Cell::from(format!("{:>2}", b.position)),
                Cell::from(Line::from(vec![
                    if state.selected_drivers.contains(&b.driver_number) {
                        Span::styled("* ", Style::default().fg(Color::Cyan))
                    } else {
                        Span::raw("  ")
                    },
                    Span::styled(
                        &b.acronym,
                        Style::default().fg(tc).add_modifier(Modifier::BOLD),
                    ),
                ])),
                Cell::from(Span::styled(truncate(&b.team, 12), Style::default().fg(tc))),
                Cell::from(Span::styled(
                    format_time(b.best_lap),
                    best_lap_style(b.best_lap, s.best_lap_time),
                )),
                Cell::from(if b.position == 1 {
                    "LEADER".to_string()
                } else {
                    format_gap(&b.gap)
                }),
                Cell::from(format_time(r.display_last_lap)),
                Cell::from(Span::styled(
                    format_sector(r.display_s1),
                    sector_style(r.display_s1, s.best_s1, pb.0, flash_s1),
                )),
                Cell::from(Span::styled(
                    format_sector(r.display_s2),
                    sector_style(r.display_s2, s.best_s2, pb.1, flash_s2),
                )),
                Cell::from(Span::styled(
                    format_sector(r.display_s3),
                    sector_style(r.display_s3, s.best_s3, pb.2, flash_s3),
                )),
                Cell::from(Span::styled(tyre_str, tyre_style)),
                Cell::from(format!("{:>2}", b.lap_count)),
                Cell::from(if !b.knocked_out.is_empty() {
                    Span::styled(&b.knocked_out, Style::default().fg(Color::DarkGray))
                } else if b.in_pit {
                    Span::styled(
                        "PIT",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )
                } else if b.is_pit_out_lap {
                    Span::styled("OUT", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                }),
            ];
            let row = Row::new(cells);
            if !b.knocked_out.is_empty() || b.in_pit {
                row.style(Style::default().fg(Color::DarkGray))
            } else if is_selected {
                row.style(
                    Style::default()
                        .bg(Color::Rgb(60, 60, 80))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                row
            }
        })
        .collect();

    let lap_display = state.lap_display();
    let title_left = if lap_display.is_empty() {
        format!(" {} - {} - {} ", s.country, s.circuit, s.session_name)
    } else {
        format!(
            " {} - {} - {} - {} ",
            s.country, s.circuit, s.session_name, lap_display
        )
    };

    let title_right = format_clock_info(state);
    let border_color = s.session_type_enum.color();

    let widths = [
        Constraint::Length(3),  // P
        Constraint::Length(5),  // Driver
        Constraint::Length(13), // Team
        Constraint::Length(9),  // Best
        Constraint::Length(8),  // Gap
        Constraint::Length(9),  // Last
        Constraint::Length(7),  // S1
        Constraint::Length(7),  // S2
        Constraint::Length(7),  // S3
        Constraint::Length(4),  // Tyre
        Constraint::Length(4),  // Laps
        Constraint::Length(3),  // Status (knockout marker)
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title_left)
        .title_bottom(
            Line::from(Span::styled(title_right, clock_style(state)))
                .alignment(ratatui::layout::Alignment::Right),
        )
        .border_style(Style::default().fg(border_color));

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_widget(table, area);
}

fn format_gap(s: &str) -> String {
    if s.is_empty() {
        "---".to_string()
    } else {
        s.to_string()
    }
}

fn format_sector(v: Option<f64>) -> String {
    match v {
        Some(t) if t > 0.0 => format!("{:.3}", t),
        _ => "---".to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}.", &s[..max - 1])
    } else {
        s.to_string()
    }
}
