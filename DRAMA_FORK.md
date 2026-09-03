# drama-crew/codex-acp - fork tracking

Fork of **zed-industries/codex-acp**, maintained for the **Drama platform**.

## Canonical branches

- `main` is the canonical Drama fork and the source of the ACP binary shipped
  in the sandbox and desktop application.
- `upstream/main` is the tracking reference for Zed's upstream `main` via the
  `upstream` remote. It is used for review and synchronization, not release.
- Historical versioned branches remain available only to reproduce old
  releases; consumers must target `main`.

## Why this fork exists

codex-acp translates Codex events into ACP. Drama maintains the following
product extensions here:

- Model-authored Codex command summaries become ACP `toolCall.title` and
  metadata, keeping the behind-the-scenes log readable.
- `_drama/steer` injects input into an in-flight steerable turn.
- `_drama/session/fork` creates a session from an earlier user-message anchor.

## Codex dependency

All patched `codex-*` dependencies resolve from **`drama-crew/codex` branch
`main`**, which is the canonical Drama Codex fork based on `rust-v0.144.3` and
contains the maintained summary, image, and memory patches. Cargo.lock records
the exact resolved commit for a reproducible build.

The in-flight upgrade to codex `rust-v0.152.1` lives on branch
`drama/test-0.152` in both repositories, consumed by the test environment and
the `Causyn-beta` desktop build. It is deliberately not merged into `main`
until the test environment has been validated. See
`FORK_MIRROR_AUDIT-0.152.md` for that upgrade's classification audit.

## Tracking workflow

1. Fetch `upstream/main` and review upstream changes against Drama `main`.
2. Integrate the selected upstream release into Drama `main`, preserve the ACP
   extensions, run `cargo update`, and test the resulting binary.
3. `src/fork.rs` no longer mirrors codex-core's user-message classification —
   it calls `codex_core::parse_turn_item`, the same predicate
   `user_message_positions_in_rollout` uses, so the two cannot drift. The
   manual registry diff this step used to mandate is therefore obsolete; the
   `mirror_agrees_with_codex_core_classifier` test enforces it instead. Still
   run a real-binary `_drama/session/fork` e2e before release: the test covers
   classification, not the RPC/thread wiring around it.

## Release

Build and tag the `codex-acp` binary for each target architecture. The sandbox
installs that binary and the desktop application embeds its matching build.
