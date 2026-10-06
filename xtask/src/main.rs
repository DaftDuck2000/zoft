//! Zoft Build Automation

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask", version, about = "Zoft build automation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build release with optimizations
    BuildRelease {
        /// Target triple (e.g., x86_64-unknown-linux-gnu)
        #[arg(long)]
        target: Option<String>,
    },
    /// Run all tests
    Test,
    /// Run clippy on all crates
    Lint,
    /// Format all code
    Fmt {
        #[arg(long)]
        check: bool,
    },
    /// Generate documentation
    Doc {
        #[arg(long)]
        open: bool,
    },
    /// Create release package
    Package {
        #[arg(long)]
        version: String,
    },
    /// Update dependencies
    Update,
    /// Audit dependencies
    Audit,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Command::BuildRelease { target } => build_release(target)?,
        Command::Test => run_tests()?,
        Command::Lint => run_lint()?,
        Command::Fmt { check } => run_fmt(check)?,
        Command::Doc { open } => generate_docs(open)?,
        Command::Package { version } => create_package(version)?,
        Command::Update => update_deps()?,
        Command::Audit => audit_deps()?,
    }
    
    Ok(())
}

fn build_release(target: Option<String>) -> Result<()> {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["build", "--release", "--workspace"]);
    if let Some(t) = target {
        cmd.args(["--target", &t]);
    }
    run_cmd(cmd)
}

fn run_tests() -> Result<()> {
    run_cmd(std::process::Command::new("cargo").args(["test", "--workspace"]))
}

fn run_lint() -> Result<()> {
    run_cmd(std::process::Command::new("cargo").args(["clippy", "--workspace", "--", "-D", "warnings"]))
}

fn run_fmt(check: bool) -> Result<()> {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["fmt"]);
    if check {
        cmd.arg("--check");
    }
    run_cmd(cmd)
}

fn generate_docs(open: bool) -> Result<()> {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["doc", "--workspace", "--no-deps"]);
    run_cmd(&mut cmd)?;
    
    if open {
        #[cfg(target_os = "linux")]
        std::process::Command::new("xdg-open").arg("target/doc/zoft/index.html").spawn()?;
        #[cfg(target_os = "macos")]
        std::process::Command::new("open").arg("target/doc/zoft/index.html").spawn()?;
        #[cfg(target_os = "windows")]
        std::process::Command::new("cmd").args(["/c", "start", "target/doc/zoft/index.html"]).spawn()?;
    }
    Ok(())
}

fn create_package(version: String) -> Result<()> {
    println!("Creating package v{}", version);
    // TODO: Implement packaging for each platform
    Ok(())
}

fn update_deps() -> Result<()> {
    run_cmd(std::process::Command::new("cargo").args(["update", "--workspace"]))
}

fn audit_deps() -> Result<()> {
    run_cmd(std::process::Command::new("cargo").args(["audit"]))
}

fn run_cmd(cmd: &mut std::process::Command) -> Result<()> {
    println!("Running: {:?}", cmd);
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("Command failed with status: {}", status);
    }
    Ok(())
}