use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let cwd = env::current_dir().context("cannot determine current directory")?;
    let left = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.clone());
    let right = args.get(1).map(PathBuf::from).unwrap_or(cwd);
    cian_tui::run(left, right)
}
