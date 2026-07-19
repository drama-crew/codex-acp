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

## Custom ACP extension: `_drama/steer`
Agent-bound extension request (client → agent, `src/codex_agent.rs` `DramaSteerRequest`) that injects additional user input into an **in-flight** turn without interrupting it, bridging to codex-core's `CodexThread::steer_input`.

- Params: `{ sessionId: string, input: ContentBlock[] }` (camelCase wire).
- Success response: `{ turnId: string }` — the turn the input was steered into.
- Failure: JSON-RPC error `code=-32002`; `message` contains `no_active_turn` (nothing running to steer into, or the steered-into turn no longer matches what was expected) or `not_steerable` (the active turn is a Review or Compact turn, which can't be steered).
- This bypasses the prompt queue entirely: unlike `session/prompt`, no `PromptState`/submission is created and no `Op` is submitted to codex-core — the request resolves as soon as `steer_input` returns. It also does **not** echo the injected input back to the client as a session update; hosts render it themselves, same constraint as `_drama/ask`.
- See `src/thread.rs`: `STEER_EXT_METHOD`, `CodexThreadImpl::steer_input`, `ThreadMessage::Steer`, `Thread::steer`, `ThreadActor::handle_steer`, `steer_input_error_to_acp_error`.
