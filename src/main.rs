mod config;
mod cwd_repo;
mod github;
mod tui;

use anyhow::Result;
use clap::Parser;

/// Interactive TUI for browsing and managing GitHub issues across an organisation.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// GitHub organisation to browse. Falls back to `default_org` in the config file.
    #[arg(short, long)]
    org: Option<String>,

    /// GitHub token. Falls back to GITHUB_TOKEN, GH_TOKEN, then `gh auth token`.
    #[arg(long)]
    token: Option<String>,

    /// Include closed issues in the initial fetch.
    #[arg(long)]
    all: bool,

    /// Auto-refresh interval in seconds (0 disables). Overrides
    /// `refresh_interval` in the config file.
    #[arg(long, value_name = "SECS")]
    refresh: Option<u64>,
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort terminal restore so the panic message is readable.
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stderr(), crossterm::terminal::LeaveAlternateScreen);
        original(info);
    }));
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::Config::load()?;

    // Precedence: --org flag → cwd git remote (owner + repo filter) →
    // config default_org. The repo filter only applies when the org being
    // browsed is the detected remote's owner.
    let detected = cwd_repo::detect();
    let (org, initial_repo) = match (cli.org, detected) {
        (Some(org), Some((owner, repo))) => {
            let filter = owner.eq_ignore_ascii_case(&org).then_some(repo);
            (org, filter)
        }
        (Some(org), None) => (org, None),
        (None, Some((owner, repo))) => (owner, Some(repo)),
        (None, None) => (
            cfg.default_org.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "no organisation given: pass --org, set default_org in {}, \
                     or run from inside a GitHub repository clone",
                    config::Config::path().display()
                )
            })?,
            None,
        ),
    };

    let theme = cfg.resolve_theme()?;
    let token = github::auth::resolve_token(cli.token)?;
    let client = github::Client::new(token)?;

    install_panic_hook();
    tui::run(
        client,
        org,
        initial_repo,
        cli.all,
        cfg.default_collapsed,
        cli.refresh.unwrap_or(cfg.refresh_interval),
        theme,
    )
    .await
}
