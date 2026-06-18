//! xtask: Development tasks for claude-vault
//!
//! Rust-based alternative to shell scripts for common development tasks.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::env;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Run all tests
    Test {
        /// Run tests in release mode
        #[arg(long)]
        release: bool,
    },
    /// Format code
    Fmt {
        /// Check formatting without making changes
        #[arg(long)]
        check: bool,
    },
    /// Build all binaries
    Build {
        /// Build in release mode
        #[arg(long)]
        release: bool,
    },
    /// Run all validation tasks
    Check {
        /// Include clippy (disabled by default per project preference)
        #[arg(long)]
        clippy: bool,
    },
    /// Test MCP server integration
    TestMcp,
    /// Test trainer integration
    TestTrainer,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let workspace_dir = workspace_root()?;

    match args.task {
        Task::Test { release } => run_test(release),
        Task::Fmt { check } => run_fmt(check),
        Task::Build { release } => run_build(release),
        Task::Check { clippy } => run_check(clippy),
        Task::TestMcp => test_mcp(&workspace_dir),
        Task::TestTrainer => test_trainer(&workspace_dir),
    }
}

fn workspace_root() -> Result<PathBuf> {
    let current = env::current_dir().context("Failed to get current directory")?;
    if current.ends_with("xtask") {
        Ok(current.parent().unwrap().to_path_buf())
    } else {
        Ok(current)
    }
}

fn cargo() -> Command {
    Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()))
}

fn run_test(release: bool) -> Result<()> {
    let mut cmd = cargo();
    cmd.args(["test"]);
    if release {
        cmd.args(["--release"]);
    }

    let status = cmd.status().context("Failed to run cargo test")?;
    if !status.success() {
        bail!("Tests failed");
    }

    println!("All tests passed");
    Ok(())
}

fn run_fmt(check: bool) -> Result<()> {
    let mut cmd = cargo();
    cmd.args(["fmt"]);
    if check {
        cmd.args(["--", "--check"]);
    }

    let status = cmd.status().context("Failed to run cargo fmt")?;
    if !status.success() {
        bail!("Formatting check failed");
    }

    if check {
        println!("Formatting check passed");
    } else {
        println!("Code formatted");
    }
    Ok(())
}

fn run_build(release: bool) -> Result<()> {
    let mut cmd = cargo();
    cmd.args(["build"]);
    if release {
        cmd.args(["--release"]);
    }

    let status = cmd.status().context("Failed to run cargo build")?;
    if !status.success() {
        bail!("Build failed");
    }

    println!("Build succeeded");
    Ok(())
}

fn run_check(clippy: bool) -> Result<()> {
    println!("Running fmt check...");
    run_fmt(true)?;

    println!("Running build...");
    run_build(false)?;

    println!("Running tests...");
    run_test(false)?;

    if clippy {
        println!("Running clippy...");
        let status = cargo()
            .args(["clippy", "--all-targets"])
            .status()
            .context("Failed to run clippy")?;
        if !status.success() {
            bail!("Clippy check failed");
        }
    }

    println!("\nAll validation tasks passed");
    Ok(())
}

/// Test MCP server by sending a JSON-RPC request
fn test_mcp(workspace_dir: &PathBuf) -> Result<()> {
    let mcp_path = workspace_dir.join("target/debug/claude-vault-mcp");

    if !mcp_path.exists() {
        bail!("MCP server not built. Run `cargo build` first.");
    }

    println!("Testing MCP server...");

    // Test initialize request
    let test_json = r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#;
    let mut cmd = Command::new(&mcp_path);
    let response = pipe_input(&mut cmd, test_json)?;

    if response.contains("\"result\"") || response.contains("\"tools\"") {
        println!("MCP server initialize: OK");
    } else {
        bail!("Unexpected MCP response: {}", response);
    }

    // Test tools/list
    let tools_json = r#"{"jsonrpc":"2.0","method":"tools/list","id":2}"#;
    let mut cmd = Command::new(&mcp_path);
    let response = pipe_input(&mut cmd, tools_json)?;

    if response.contains("search_vault") && response.contains("get_session") {
        println!("MCP tools/list: OK");
    } else {
        bail!("Tools list missing expected tools: {}", response);
    }

    println!("MCP server tests passed");
    Ok(())
}

/// Test trainer CLI
fn test_trainer(workspace_dir: &PathBuf) -> Result<()> {
    let trainer_path = workspace_dir.join("target/debug/claude-trainer");

    if !trainer_path.exists() {
        bail!("Trainer not built. Run `cargo build` first.");
    }

    println!("Testing trainer...");

    // Test --help
    let output = Command::new(&trainer_path)
        .arg("--help")
        .output()
        .context("Failed to run trainer")?;

    if !output.status.success() {
        bail!("Trainer --help failed");
    }

    let help = String::from_utf8_lossy(&output.stdout);
    if help.contains("export") && help.contains("tokens") && help.contains("clean") {
        println!("Trainer help: OK");
    } else {
        bail!("Trainer help missing expected commands");
    }

    // Test tokens --help
    let output = Command::new(&trainer_path)
        .args(["tokens", "--help"])
        .output()
        .context("Failed to run trainer tokens")?;

    if !output.status.success() {
        bail!("Trainer tokens --help failed");
    }

    println!("Trainer tests passed");
    Ok(())
}

/// Write input to a command's stdin
fn pipe_input(cmd: &mut Command, input: &str) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn process")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .context("Failed to write to stdin")?;
    }

    let output = child.wait_with_output().context("Failed to read output")?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
