use anyhow::Result;
use clap::{Parser, Subcommand};
use codex_arg0::arg0_dispatch_or_else;
use codex_utils_cli::CliConfigOverrides;

/// codex-acp: an ACP-compatible coding agent powered by Codex.
///
/// Invoked with no subcommand, this starts the ACP server over stdio (the historical/default
/// behavior). `memory-run` / `memory-clear` are standalone, one-shot utilities for the
/// drama-desktop-memory background worker -- they do not start an ACP server.
#[derive(Parser)]
#[command(name = "codex-acp")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    config_overrides: CliConfigOverrides,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the memories pipeline (Phase 1 extraction + Phase 2 consolidation) once to
    /// completion and print a one-line JSON summary to stdout.
    MemoryRun,
    /// Clear all persisted memory state: the on-disk memory roots under `codex_home` and the
    /// memories table in the SQLite state DB under `sqlite_home`.
    MemoryClear,
}

fn main() -> Result<()> {
    // `arg0_dispatch_or_else` must run first and wrap everything else: it re-execs this binary
    // for linux-sandbox / apply_patch helper invocations based on argv0 before any subcommand
    // parsing happens, so subcommand dispatch has to live entirely inside its closure.
    arg0_dispatch_or_else(|args| async move {
        let cli = Cli::parse();
        match cli.command {
            None => {
                codex_acp::run_main(args.codex_linux_sandbox_exe, cli.config_overrides).await?;
            }
            Some(Commands::MemoryRun) => {
                codex_acp::memory_worker::run_memory_run(
                    args.codex_linux_sandbox_exe,
                    cli.config_overrides,
                )
                .await?;
            }
            Some(Commands::MemoryClear) => {
                codex_acp::memory_worker::run_memory_clear(cli.config_overrides).await?;
            }
        }
        Ok(())
    })
}
