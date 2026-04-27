use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;

use crate::ui::login::{LoginState, render_login};
use f1core::auth::{self, Credentials};

pub enum LoginAction {
    /// User cancelled — return to picker.
    Cancel,
    /// Authentication succeeded — return credentials to reinitialize the client.
    Authenticated(Credentials),
}

pub async fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<LoginAction> {
    let mut state = LoginState::new();

    // If there's a pending submit, run it asynchronously
    let mut pending_submit: Option<tokio::task::JoinHandle<Result<(), String>>> = None;

    loop {
        // Check if a pending auth attempt completed
        if let Some(ref handle) = pending_submit
            && handle.is_finished()
        {
            let handle = pending_submit.take().unwrap();
            match handle.await {
                Ok(Ok(())) => {
                    // Auth succeeded — store in keychain and return
                    if let Err(e) = auth::keychain::store(&state.username, &state.password) {
                        eprintln!("warning: failed to save credentials to keychain: {e}");
                    }
                    return Ok(LoginAction::Authenticated(Credentials::new(
                        state.username,
                        state.password,
                    )));
                }
                Ok(Err(msg)) => {
                    state.error = Some(msg);
                    state.submitting = false;
                }
                Err(e) => {
                    state.error = Some(format!("Internal error: {e}"));
                    state.submitting = false;
                }
            }
        }

        terminal.draw(|f| render_login(f, &state))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if state.submitting {
                // Ignore input while submitting (except Esc)
                if key.code == KeyCode::Esc {
                    return Ok(LoginAction::Cancel);
                }
                continue;
            }

            match key.code {
                KeyCode::Esc => return Ok(LoginAction::Cancel),
                KeyCode::Tab | KeyCode::BackTab => state.toggle_field(),
                KeyCode::Enter => {
                    if state.username.is_empty() || state.password.is_empty() {
                        state.error = Some("Both fields are required".to_string());
                        continue;
                    }
                    state.error = None;
                    state.submitting = true;
                    let username = state.username.clone();
                    let password = state.password.clone();
                    pending_submit = Some(tokio::spawn(async move {
                        auth::AuthManager::new(Credentials::new(username, password))
                            .await
                            .map(|_| ()) // discard the manager, we just validated
                            .map_err(|e| format!("{e}"))
                    }));
                }
                KeyCode::Char(c) => state.insert_char(c),
                KeyCode::Backspace => state.backspace(),
                KeyCode::Delete => state.delete(),
                KeyCode::Left => state.move_left(),
                KeyCode::Right => state.move_right(),
                KeyCode::Home => state.home(),
                KeyCode::End => state.end(),
                _ => {}
            }
        }
    }
}
