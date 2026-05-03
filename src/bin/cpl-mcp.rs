use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use cognitive_project_layer::budget::ContextBudgetManager;

#[derive(Debug, Parser)]
#[command(name = "cpl-mcp")]
#[command(about = "MCP stdio server for Cognitive Project Layer")]
#[command(version)]
struct Cli {
    #[arg(long, short = 'r', default_value = ".")]
    root: PathBuf,
    #[arg(long, default_value_t = ContextBudgetManager::default().max_tokens)]
    max_tokens: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    cognitive_project_layer::mcp_server::run_stdio_with_budget(cli.root, cli.max_tokens)
}
