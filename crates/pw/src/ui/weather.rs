use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::AppState;

pub fn render_weather(f: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let content = match &state.session.weather {
        Some(w) => {
            let rain_indicator = if w.rainfall {
                Span::styled(
                    " RAIN ",
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("Dry", Style::default().fg(Color::Green))
            };

            Line::from(vec![
                Span::styled(" Air ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.0}°C", w.air_temp.unwrap_or(0.0)),
                    Style::default().fg(Color::White),
                ),
                Span::raw("  "),
                Span::styled("Track ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.0}°C", w.track_temp.unwrap_or(0.0)),
                    Style::default().fg(Color::White),
                ),
                Span::raw("  "),
                Span::styled("Humidity ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.0}%", w.humidity.unwrap_or(0.0)),
                    Style::default().fg(Color::White),
                ),
                Span::raw("  "),
                Span::styled("Wind ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.0}km/h", w.wind_speed.unwrap_or(0.0)),
                    Style::default().fg(Color::White),
                ),
                Span::raw("  "),
                rain_indicator,
            ])
        }
        None => Line::from(Span::styled(
            " No weather data",
            Style::default().fg(Color::DarkGray),
        )),
    };

    let paragraph = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Weather ")
            .border_style(Style::default().fg(Color::Cyan)),
    );

    f.render_widget(paragraph, area);
}
