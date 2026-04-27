mod app;
mod pages;
mod session_types;
mod ui;
mod update;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use f1core::{api, auth, db};
use ratatui::prelude::*;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(name = "pw", about = "Live F1 timing board in the terminal")]
struct Args {
    /// Session key to display (omit for session picker)
    #[arg(short, long)]
    session: Option<i64>,

    /// Database file path
    #[arg(long, default_value_os_t = db::default_db_path())]
    db: PathBuf,

    /// Delete the database and start fresh
    #[arg(long)]
    fresh: bool,

    /// Playback speed for replays (e.g. 2.0 for 2x speed)
    #[arg(long, default_value = "1.0")]
    speed: f64,

    /// Check for and apply the latest update, then exit
    #[arg(long)]
    update: bool,

    /// OpenF1 username (overrides keychain; requires --password)
    #[arg(long, env = "PW_USERNAME")]
    username: Option<String>,

    /// OpenF1 password (overrides keychain; requires --username)
    #[arg(long, env = "PW_PASSWORD")]
    password: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.update {
        return update::perform_update();
    }

    if args.fresh {
        let _ = std::fs::remove_file(&args.db);
    }

    // Resolve credentials: CLI flags → keychain → none
    let credentials = match (args.username, args.password) {
        (Some(u), Some(p)) => Some(auth::Credentials::new(u, p)),
        (None, None) => auth::keychain::load(),
        _ => anyhow::bail!("Both --username and --password must be provided together"),
    };

    let database = db::Db::open(&args.db)?;
    let db = Arc::new(Mutex::new(database));
    let client = Arc::new(api::OpenF1Client::new(credentials).await?);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = if let Some(sk) = args.session {
        pages::session::run(&mut terminal, sk, args.speed, &client, &db).await
    } else {
        run_picker_loop(&mut terminal, args.speed, client, &db).await
    };

    // Cleanup terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_picker_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    speed: f64,
    mut client: Arc<api::OpenF1Client>,
    db: &Arc<Mutex<db::Db>>,
) -> Result<()> {
    loop {
        let authenticated = client.is_authenticated().await;
        match pages::picker::run(terminal, &client, db, authenticated).await? {
            pages::picker::PickerAction::Quit => return Ok(()),
            pages::picker::PickerAction::Select { session_key } => {
                pages::session::run(terminal, session_key, speed, &client, db).await?;
            }
            pages::picker::PickerAction::Login(creds) => {
                client = Arc::new(api::OpenF1Client::new(Some(creds)).await?);
            }
            pages::picker::PickerAction::Logout => {
                client = Arc::new(api::OpenF1Client::new(None).await?);
            }
        }
    }
}
