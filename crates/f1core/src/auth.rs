use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::Client;
use tokio::sync::Mutex;

const TOKEN_URL: &str = "https://api.openf1.org/token";

/// How long before expiry to proactively refresh (10 minutes).
const REFRESH_BUFFER: Duration = Duration::from_secs(600);

/// Cooldown before retrying auth after entering degraded state.
const DEGRADED_COOLDOWN: Duration = Duration::from_secs(60);

/// Maximum retry attempts for token requests.
const MAX_RETRIES: u32 = 3;

pub struct Credentials {
    pub(crate) username: String,
    pub(crate) password: String,
}

impl Credentials {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    /// API returns this as a string, not a number.
    expires_in: String,
    #[allow(dead_code)]
    token_type: String,
}

enum TokenState {
    Valid {
        access_token: String,
        expires_at: Instant,
    },
    Degraded {
        last_attempt: Instant,
    },
}

pub struct AuthManager {
    credentials: Credentials,
    token_state: Mutex<TokenState>,
    /// Separate HTTP client for auth requests — not rate-limited, longer timeout.
    http_client: Client,
}

impl AuthManager {
    /// Create a new AuthManager and perform initial authentication.
    /// Returns Err if the first authentication attempt fails (bad credentials, network error).
    pub async fn new(credentials: Credentials) -> Result<Self> {
        let http_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build auth HTTP client")?;

        let token_state = Self::authenticate(&http_client, &credentials).await?;

        Ok(Self {
            credentials,
            token_state: Mutex::new(token_state),
            http_client,
        })
    }

    /// Get a valid access token, refreshing proactively if near expiry.
    /// Returns None if auth has degraded (caller should proceed without auth).
    pub async fn get_valid_token(&self) -> Option<String> {
        let mut state = self.token_state.lock().await;

        match &*state {
            TokenState::Valid {
                access_token,
                expires_at,
            } => {
                if Instant::now() + REFRESH_BUFFER < *expires_at {
                    // Token is still fresh — return it.
                    return Some(access_token.clone());
                }
                // Token nearing expiry — try to refresh.
                match self.try_refresh().await {
                    Ok(new_state) => {
                        let token = match &new_state {
                            TokenState::Valid { access_token, .. } => Some(access_token.clone()),
                            TokenState::Degraded { .. } => None,
                        };
                        *state = new_state;
                        token
                    }
                    Err(_) => {
                        // Refresh failed — degrade but return current (still valid) token.
                        Some(access_token.clone())
                    }
                }
            }
            TokenState::Degraded { last_attempt } => {
                if last_attempt.elapsed() < DEGRADED_COOLDOWN {
                    return None;
                }
                // Cooldown elapsed — retry auth.
                match self.try_refresh().await {
                    Ok(new_state) => {
                        let token = match &new_state {
                            TokenState::Valid { access_token, .. } => Some(access_token.clone()),
                            TokenState::Degraded { .. } => None,
                        };
                        *state = new_state;
                        token
                    }
                    Err(_) => {
                        *state = TokenState::Degraded {
                            last_attempt: Instant::now(),
                        };
                        None
                    }
                }
            }
        }
    }

    /// Force re-authentication (called on 401 response).
    pub async fn force_refresh(&self) {
        let mut state = self.token_state.lock().await;
        match self.try_refresh().await {
            Ok(new_state) => *state = new_state,
            Err(_) => {
                *state = TokenState::Degraded {
                    last_attempt: Instant::now(),
                };
            }
        }
    }

    /// Whether the manager currently holds a valid (non-degraded) token.
    /// Cheap sync-ish check for polling config selection.
    pub async fn is_valid(&self) -> bool {
        let state = self.token_state.lock().await;
        matches!(&*state, TokenState::Valid { .. })
    }

    /// Try to get a fresh token with retries and backoff.
    async fn try_refresh(&self) -> Result<TokenState> {
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            match Self::authenticate(&self.http_client, &self.credentials).await {
                Ok(state) => return Ok(state),
                Err(e) => {
                    last_err = Some(e);
                    let wait = Duration::from_secs(2u64.pow(attempt + 1));
                    tokio::time::sleep(wait).await;
                }
            }
        }
        Err(last_err.unwrap())
    }

    /// POST to the token endpoint and return a Valid token state.
    async fn authenticate(client: &Client, credentials: &Credentials) -> Result<TokenState> {
        let resp = client
            .post(TOKEN_URL)
            .form(&[
                ("username", credentials.username.as_str()),
                ("password", credentials.password.as_str()),
            ])
            .send()
            .await
            .context("failed to reach auth endpoint")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("authentication failed ({status}): {body}");
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .context("failed to parse token response")?;

        let expires_in_secs: u64 = token_resp
            .expires_in
            .parse()
            .context("expires_in is not a valid number")?;

        Ok(TokenState::Valid {
            access_token: token_resp.access_token,
            expires_at: Instant::now() + Duration::from_secs(expires_in_secs),
        })
    }
}

/// Keychain helpers for persisting credentials in the OS secret store.
/// Stores both username and password in a single keychain entry to minimize
/// macOS keychain access prompts.
pub mod keychain {
    const SERVICE: &str = "f1-pitwall";
    const ACCOUNT: &str = "credentials";
    const SEPARATOR: char = '\n';

    pub fn store(username: &str, password: &str) -> Result<(), keyring::Error> {
        let combined = format!("{}{}{}", username, SEPARATOR, password);
        keyring::Entry::new(SERVICE, ACCOUNT)?.set_password(&combined)?;
        Ok(())
    }

    pub fn load() -> Option<super::Credentials> {
        // Try combined format (single keychain access)
        let combined = keyring::Entry::new(SERVICE, ACCOUNT)
            .ok()?
            .get_password()
            .ok()?;
        let (username, password) = combined.split_once(SEPARATOR)?;
        Some(super::Credentials::new(
            username.to_string(),
            password.to_string(),
        ))
    }

    pub fn clear() {
        for key in &[ACCOUNT, "username", "password"] {
            if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
                let _ = entry.delete_credential();
            }
        }
    }
}
