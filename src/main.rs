mod config;
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

    let org = cli.org.or_else(|| cfg.default_org.clone()).ok_or_else(|| {
        anyhow::anyhow!(
            "no organisation given: pass --org or set default_org in {}",
            config::Config::path().display()
        )
    })?;

    let token = github::auth::resolve_token(cli.token)?;
    let client = github::Client::new(token)?;

    install_panic_hook();
    tui::run(client, org, cli.all, cfg.default_collapsed).await
}
