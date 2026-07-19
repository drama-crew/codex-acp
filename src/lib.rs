//! Codex ACP - An Agent Client Protocol implementation for Codex.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use agent_client_protocol::ByteStreams;
use codex_core::config::{Config, ConfigOverrides};
use codex_features::Feature;
use codex_utils_cli::CliConfigOverrides;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing_subscriber::EnvFilter;

mod codex_agent;
mod fork;
mod thread;

/// Env var escape hatch: when set to a truthy value, the `request_user_input`
/// tool is left disabled in Default collaboration mode (matching upstream
/// codex's default behavior).
const DISABLE_ASK_ENV_VAR: &str = "DRAMA_DISABLE_ASK";

fn disable_ask_env_var_set() -> bool {
    std::env::var(DISABLE_ASK_ENV_VAR).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Enable the `request_user_input` tool in `ModeKind::Default`, unless the
/// `DRAMA_DISABLE_ASK` escape hatch is set. Upstream codex gates this tool
/// behind `Feature::DefaultModeRequestUserInput` (default disabled); we want
/// it available by default so agents can ask the user clarifying questions
/// even in Default mode.
fn apply_default_mode_ask_feature(config: &mut Config) {
    if disable_ask_env_var_set() {
        return;
    }
    if let Err(err) = config
        .features
        .set_enabled(Feature::DefaultModeRequestUserInput, true)
    {
        tracing::warn!("failed to enable request_user_input in default mode: {err}");
    }
}

/// Run the Codex ACP agent.
///
/// This sets up an ACP agent that communicates over stdio, bridging
/// the ACP protocol with the existing codex-rs infrastructure.
///
/// # Errors
///
/// If unable to parse the config or start the program.
pub async fn run_main(
    codex_linux_sandbox_exe: Option<PathBuf>,
    cli_config_overrides: CliConfigOverrides,
) -> std::io::Result<()> {
    // Install a simple subscriber so `tracing` output is visible.
    // Users can control the log level with `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Parse CLI overrides and load configuration
    let cli_kv_overrides = cli_config_overrides.parse_overrides().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("error parsing -c overrides: {e}"),
        )
    })?;

    let config_overrides = ConfigOverrides {
        codex_linux_sandbox_exe: codex_linux_sandbox_exe.clone(),
        ..ConfigOverrides::default()
    };

    let mut config =
        Config::load_with_cli_overrides_and_harness_overrides(cli_kv_overrides, config_overrides)
            .await
            .map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("error loading config: {e}"),
                )
            })?;
    apply_default_mode_ask_feature(&mut config);
    // Apply residency requirement so the HTTP client sends the
    // x-openai-internal-codex-residency header on all requests.
    codex_login::default_client::set_default_client_residency_requirement(
        config.enforce_residency.value(),
    );

    let agent = Arc::new(codex_agent::CodexAgent::new(config, codex_linux_sandbox_exe).await?);

    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();

    agent
        .serve(ByteStreams::new(stdout, stdin))
        .await
        .map_err(|e| std::io::Error::other(format!("ACP error: {e}")))?;

    Ok(())
}

// Re-export the MCP server types for compatibility
pub use codex_mcp_server::{
    CodexToolCallParam, CodexToolCallReplyParam, ExecApprovalElicitRequestParams,
    ExecApprovalResponse, PatchApprovalElicitRequestParams, PatchApprovalResponse,
};

#[cfg(test)]
mod default_mode_ask_feature_tests {
    use super::*;

    // SAFETY: this test mutates a process-wide env var (`DRAMA_DISABLE_ASK`).
    // It is the only test in the crate that touches it, and both assertions
    // run sequentially within a single `#[test]` function, so there is no
    // cross-test interleaving to guard against.

    #[tokio::test]
    async fn default_mode_ask_feature_respects_escape_hatch() -> anyhow::Result<()> {
        // Default: env var unset -> feature enabled.
        unsafe {
            std::env::remove_var(DISABLE_ASK_ENV_VAR);
        }
        let mut config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        apply_default_mode_ask_feature(&mut config);
        assert!(
            config
                .features
                .get()
                .enabled(Feature::DefaultModeRequestUserInput),
            "expected request_user_input to be enabled in default mode"
        );

        // Escape hatch: DRAMA_DISABLE_ASK=1 -> feature stays disabled.
        unsafe {
            std::env::set_var(DISABLE_ASK_ENV_VAR, "1");
        }
        let mut config = Config::load_with_cli_overrides_and_harness_overrides(
            vec![],
            ConfigOverrides::default(),
        )
        .await?;
        apply_default_mode_ask_feature(&mut config);
        assert!(
            !config
                .features
                .get()
                .enabled(Feature::DefaultModeRequestUserInput),
            "expected DRAMA_DISABLE_ASK=1 to keep request_user_input disabled"
        );

        unsafe {
            std::env::remove_var(DISABLE_ASK_ENV_VAR);
        }
        Ok(())
    }
}
