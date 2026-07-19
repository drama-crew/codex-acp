# drama-crew/codex-acp — fork tracking

Fork of **zed-industries/codex-acp**, maintained for the **Drama platform**.

This is the **released artifact** of the codex-summary-titles mechanism: codex-acp is the ACP adapter the drama sandbox installs/runs, so the binary built from this fork is what ships into the sandbox image.

## Why this fork exists
codex-acp builds the ACP `toolCall.title` mechanically from codex's parsed command (`src/thread.rs` `parse_command_tool_call`). We patch it to instead use a **model-authored `summary`** (added in our codex-core fork) as the tool-call title / `_meta`, so the drama studio "behind-the-scenes" log shows user-language descriptions cleanly (no command pollution).

## Pinned upstream
- Working branch `drama/summary-titles` is based on upstream `main` (commit `bb59050`).
- `main` is kept clean = upstream, for easy syncing (`gh repo sync drama-crew/codex-acp`).

## Planned patch (on `drama/summary-titles`)
- `Cargo.toml`: add `[patch."https://github.com/openai/codex"]` overrides pointing the `codex-*` crates at **`drama-crew/codex` branch `drama/summary-titles`** (our codex-core fork with the `summary` schema+plumbing patch).
- `src/thread.rs`: read the model `summary` from the tool call and use it as the ACP `toolCall` title (or `_meta`), falling back to the existing parsed-command title when absent.

## Release
- Build the `codex-acp` (a.k.a. `drama-acp` / `drama-acp-host`) binary per target arch on tag; the drama sandbox image installs it in place of `npm i -g @zed-industries/codex-acp`.
- Versioning: tag as `drama-vX.Y.Z+codex-rust-v0.137.0` (our version + the pinned upstream codex version) so the codex-core pin is traceable from the release.

## Tracking workflow
1. `gh repo sync drama-crew/codex-acp` (main ← upstream).
2. On version bump: update the codex tag our codex fork is based on, rebase both forks' `drama/summary-titles`, re-apply patches, rebuild + retest the sandbox.

## Relationship to the shipped interim mechanism
See `drama-crew/codex` DRAMA_FORK.md: the platform currently ships an interim `#__DRAMA_SUMMARY__` marker approach (merged on drama-platform main); this fork is the clean production mechanism that supersedes it once released.

## Consumers
- **Sandbox** installs this binary as the in-container ACP agent (see "Release" above); it does not call `_drama/session/fork` and is unaffected by that extension method.
- **Desktop app** embeds this binary directly (resolved in `frontend/packages/agent-host/src/platform.ts`), so there is no separate host↔binary version skew to manage — the desktop release always ships the codex-acp build it was built against. The desktop's session-fork feature (clone a session's history/memory up to an earlier user message into a new session) depends on the inbound ACP ext-method `_drama/session/fork` (`src/fork.rs` + `CodexAgent::fork_session` in `src/codex_agent.rs`), which wraps codex-core's native `ThreadManager::fork_thread` / `ForkSnapshot::TruncateBeforeNthUserMessage`. Bumping this fork therefore also gates that desktop feature; see `docs/superpowers/specs/2026-07-20-desktop-session-fork-design.md` in the OpenHands repo for the full design.
