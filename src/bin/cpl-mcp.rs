use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "cpl-mcp")]
#[command(about = "MCP stdio server for Cognitive Project Layer")]
struct Cli {
    #[arg(long, short = 'r', default_value = ".")]
    root: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    cognitive_project_layer::mcp_server::run_stdio(cli.root)
}
