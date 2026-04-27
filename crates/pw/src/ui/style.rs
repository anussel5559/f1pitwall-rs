use f1core::session_types::SessionType;
use ratatui::style::{Color, Modifier, Style};

use crate::app::AppState;

/// Extension trait adding ratatui-specific display methods to SessionType.
pub trait SessionTypeExt {
    fn color(&self) -> Color;
}

impl SessionTypeExt for SessionType {
    fn color(&self) -> Color {
        match self {
            SessionType::Race => Color::Red,
            SessionType::Qualifying => Color::Rgb(255, 165, 0),
            SessionType::Sprint => Color::Magenta,
            SessionType::SprintQualifying => Color::Rgb(255, 100, 200),
            SessionType::Practice => Color::Green,
        }
    }
}

/// Parse a hex color string (e.g. "FF8000" or "#FF8000") into a ratatui Color.
pub fn team_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() >= 6
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        )
    {
        return Color::Rgb(r, g, b);
    }
    Color::White
}

/// Style for clock display: red if live, yellow if replay.
pub fn clock_style(state: &AppState) -> Style {
    Style::default()
        .fg(if state.clock.is_live {
            Color::Red
        } else {
            Color::Yellow
        })
        .add_modifier(Modifier::BOLD)
}

/// Formatted clock info string, e.g. " LIVE 0:42:15 (14:42:15) 2x "
pub fn format_clock_info(state: &AppState) -> String {
    let label = state.clock.label();
    let elapsed = state.clock.elapsed_display();
    let local = state
        .clock
        .local_time_display()
        .map(|t| format!(" ({})", t))
        .unwrap_or_default();
    let speed = if !state.clock.is_live && (state.clock.speed - 1.0).abs() > 0.01 {
        format!(" {:.0}x", state.clock.speed)
    } else {
        String::new()
    };
    format!(" {} {}{}{} ", label, elapsed, local, speed)
}
