use anyhow::Result;

const REPO_OWNER: &str = "anussel5559";
const REPO_NAME: &str = "f1-pitwall";
const BIN_NAME: &str = "pw";

/// Check GitHub releases for a newer version.
/// Returns the version tag (e.g. "v0.2.0") if a newer release exists, or None.
pub async fn check_for_update() -> Option<String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        REPO_OWNER, REPO_NAME
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(format!("{}/{}", BIN_NAME, env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;

    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    let tag = json["tag_name"].as_str()?;
    let latest = tag.trim_start_matches('v');
    let current = env!("CARGO_PKG_VERSION");

    if semver_gt(latest, current) {
        Some(tag.to_string())
    } else {
        None
    }
}

fn semver_gt(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> (u32, u32, u32) {
        let p: Vec<u32> = v.split('.').filter_map(|x| x.parse().ok()).collect();
        (
            p.first().copied().unwrap_or(0),
            p.get(1).copied().unwrap_or(0),
            p.get(2).copied().unwrap_or(0),
        )
    };
    parse(a) > parse(b)
}

/// Download and replace the current binary with the latest GitHub release.
/// This is a blocking operation and must run outside the async runtime.
pub fn perform_update() -> Result<()> {
    let status = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .bin_path_in_archive("{{ bin }}")
        .show_download_progress(true)
        .current_version(env!("CARGO_PKG_VERSION"))
        .build()?
        .update()?;

    if status.updated() {
        println!("Updated to {}!", status.version());
    } else {
        println!("Already up to date (v{}).", env!("CARGO_PKG_VERSION"));
    }
    Ok(())
}
