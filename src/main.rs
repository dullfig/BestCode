use anyhow::Result;
use clap::Parser;
use tracing::info;

#[derive(Parser)]
#[command(name = "bestcode", about = "AI coding agent. No compaction, ever.")]
struct Cli {
    /// Working directory (defaults to current)
    #[arg(short, long)]
    dir: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bestcode=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let work_dir = cli.dir.unwrap_or_else(|| ".".into());

    info!("BestCode starting in {work_dir}");

    Ok(())
}
