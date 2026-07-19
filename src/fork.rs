//! `_drama/session/fork` inbound ACP extension method.
//!
//! Bridges a client-initiated "fork this session at an earlier user message"
//! request to codex-core's native `ThreadManager::fork_thread` primitive
//! (`ForkSnapshot::TruncateBeforeNthUserMessage`). The wire contract, error
//! codes, and truncation semantics are the drama desktop fork-session design
//! (`docs/superpowers/specs/2026-07-20-desktop-session-fork-design.md` in the
//! OpenHands repo).
//!
//! # Anchor-text matching, not UI ordinals
//!
//! The caller cannot reliably compute "this is the Nth user message" purely
//! from its own UI history (silent monitor wake-ups and compaction both skew
//! the count relative to the rollout's model-context user messages -- see the
//! design doc's "关键设计决定" section). Instead the caller sends the anchor
//! message's exact text plus a 1-based `occurrence` (which repeat of that
//! text, counting only real user-message rollout entries), and this module
//! finds the matching 0-based `ForkSnapshot::TruncateBeforeNthUserMessage`
//! index by re-deriving the rollout's user-message boundaries itself.
//!
//! # Why this isn't `codex_core::user_message_positions_in_rollout`
//!
//! codex-core has the canonical implementation of this scan
//! (`core/src/thread_rollout_truncation.rs::user_message_positions_in_rollout`),
//! but that function -- and the `event_mapping` module it depends on -- are
//! `pub(crate)`/private and unreachable from codex-acp. The "contextual
//! fragment" registry that decides which injected `role: user` messages don't
//! count (environment context, additional-context key/value fragments, skill
//! instructions, hook prompts, ...) is likewise private
//! (`core/src/context/contextual_user_message.rs`,
//! `core/src/context/world_state/`). This module reimplements the same
//! *outward* behavior against the public `codex_protocol` wire types, using
//! the same open/close tag markers and literal-prefix warnings those private
//! fragment types render (verified by reading the pinned codex source at
//! `drama-crew/codex@drama/summary-titles-codex-0.144`, commit `be371ba`).
//! It is a best-effort mirror, not shared code: if upstream adds a new
//! contextual-fragment type with a novel marker, this scan will not
//! automatically recognize it (residual risk accepted in the design doc).

use agent_client_protocol::schema::{McpServer, SessionId};
use agent_client_protocol::{JsonRpcRequest, JsonRpcResponse};
use codex_protocol::items::parse_hook_prompt_fragment;
use codex_protocol::models::{ContentItem, ResponseItem};
use codex_protocol::protocol::{
    APPS_INSTRUCTIONS_OPEN_TAG, COLLABORATION_MODE_OPEN_TAG, CONTEXT_WINDOW_GUIDANCE_OPEN_TAG,
    CONTEXT_WINDOW_OPEN_TAG, ENVIRONMENT_CONTEXT_OPEN_TAG, EventMsg, MULTI_AGENT_MODE_OPEN_TAG,
    PLUGINS_INSTRUCTIONS_OPEN_TAG, REALTIME_CONVERSATION_OPEN_TAG, RolloutItem,
    SKILLS_INSTRUCTIONS_OPEN_TAG,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// JSON-RPC error code for "the anchor text/occurrence did not match any
/// user message in the source rollout" (e.g. it was folded into a
/// compaction summary). Not retryable -- the caller should give up on this
/// exact fork point.
pub const ANCHOR_NOT_FOUND_CODE: i32 = -32011;

/// JSON-RPC error code for a transient failure to read the source session's
/// state/rollout (SQLITE_BUSY, a rollout file that's mid-write, etc.).
/// Retryable.
pub const SOURCE_BUSY_CODE: i32 = -32012;

/// Build the `-32011 anchor_not_found` error. `message` always contains the
/// literal substring `anchor_not_found` so the desktop host's regex mapping
/// (`friendlyCommandErrorDetail`) can key off it even if it only sees the
/// JSON-RPC error's `message` field.
pub fn anchor_not_found_error() -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(
        ANCHOR_NOT_FOUND_CODE,
        "anchor_not_found: no user message in the source session's rollout matched the fork \
         anchor (it may have been folded into a context-compaction summary)",
    )
}

/// Build the `-32012 source_busy` error. `detail` is appended to the
/// message for debugging; the literal substring `source_busy` is always
/// present for the same reason as [`anchor_not_found_error`].
pub fn source_busy_error(detail: impl std::fmt::Display) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(
        SOURCE_BUSY_CODE,
        format!(
            "source_busy: the source session's rollout/state is temporarily unavailable, \
             retry shortly ({detail})"
        ),
    )
}

/// The user message to fork at, identified by exact/containment text match
/// plus which occurrence of that text (1-based) to use when the text repeats
/// (e.g. a source session that says "continue" twice).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkAnchor {
    pub text: String,
    pub occurrence: usize,
}

/// `_drama/session/fork` request. Wire shape (camelCase) is normative --
/// see Task 3/9's shared interface contract.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_drama/session/fork", response = ForkSessionResponse)]
pub struct ForkSessionRequest {
    /// The working directory for the *new* forked session. The forking
    /// process has no already-loaded session to infer this from, so the
    /// caller (host) must supply it, just as it does for `new_session` /
    /// `load_session` / `resume_session`.
    pub cwd: PathBuf,
    pub source_session_id: SessionId,
    pub anchor: ForkAnchor,
    /// Diagnostic-only: the caller's own best guess at the 1-based ordinal
    /// of the anchor user message. Logged as a `tracing::warn!` when it
    /// disagrees with the text-matched result; never used to decide the
    /// truncation point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth_hint: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
}

/// `_drama/session/fork` response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionResponse {
    pub session_id: SessionId,
}

/// Contextual open-tag markers for `role: "user"` rollout messages that are
/// injected context rather than something the user actually typed, mirrored
/// from the private fragment registry in
/// `core/src/context/contextual_user_message.rs::CONTEXTUAL_USER_FRAGMENTS`
/// (environment context, apps/skills/plugins instructions, collaboration &
/// multi-agent mode banners, realtime-conversation banner, context-window
/// guidance, additional-context key/value fragments, user shell command
/// echoes, turn-aborted/subagent notifications, recommended-plugins list,
/// and the internal-model-context wrapper).
const CONTEXTUAL_USER_TAG_PREFIXES: &[&str] = &[
    ENVIRONMENT_CONTEXT_OPEN_TAG,
    SKILLS_INSTRUCTIONS_OPEN_TAG,
    APPS_INSTRUCTIONS_OPEN_TAG,
    PLUGINS_INSTRUCTIONS_OPEN_TAG,
    COLLABORATION_MODE_OPEN_TAG,
    MULTI_AGENT_MODE_OPEN_TAG,
    REALTIME_CONVERSATION_OPEN_TAG,
    CONTEXT_WINDOW_OPEN_TAG,
    CONTEXT_WINDOW_GUIDANCE_OPEN_TAG,
    "# AGENTS.md instructions", // UserInstructions fragment marker
    "<external_",               // AdditionalContextUserFragment: <external_KEY>...</external_KEY>
    "<user_shell_command>",
    "<turn_aborted>",
    "<subagent_notification>",
    "<recommended_plugins>",
    "<codex_internal_context", // InternalModelContextFragment (has attrs after the tag name)
    "<goal_context>",          // legacy InternalModelContextFragment wrapper
];

/// Literal-prefix legacy warning fragments (no wrapper tags), mirrored from
/// `core/src/context/legacy_*_warning.rs`.
const CONTEXTUAL_USER_LITERAL_PREFIXES: &[&str] = &[
    "Warning: The maximum number of unified exec processes you can keep open is",
    "Warning: Your account was flagged for potentially high-risk cyber activity",
];

fn is_legacy_apply_patch_warning(trimmed: &str) -> bool {
    trimmed.starts_with("Warning: apply_patch was requested via ")
        && trimmed.ends_with("Use the apply_patch tool instead of exec_command.")
}

/// True when a single `ContentItem::InputText` fragment of a `role: "user"`
/// rollout message is injected context rather than user-authored text.
fn is_contextual_user_text(text: &str) -> bool {
    if parse_hook_prompt_fragment(text).is_some() {
        return true;
    }
    let trimmed = text.trim_start();
    if CONTEXTUAL_USER_TAG_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return true;
    }
    if CONTEXTUAL_USER_LITERAL_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return true;
    }
    is_legacy_apply_patch_warning(trimmed)
}

/// Concatenate a message's text content items (skipping non-text parts like
/// images) the same way a fork anchor's `text` is expected to be rendered
/// (one user-visible string per rollout user message).
fn message_text(content: &[ContentItem]) -> String {
    content
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// True if any content item is a contextual fragment (mirrors codex-core's
/// `is_contextual_user_message_content`, which short-circuits the *whole*
/// message as non-counting if *any* fragment is contextual).
fn is_contextual_user_message(content: &[ContentItem]) -> bool {
    content.iter().any(|item| match item {
        ContentItem::InputText { text } => is_contextual_user_text(text),
        _ => false,
    })
}

/// If `item` is a genuine (non-contextual) `role: "user"` rollout message,
/// return its rendered text. A multimodal message (text + image(s)) still
/// counts as exactly one user message, per the design's "多模态一条计一"
/// rule; `message_text` simply omits the non-text parts from the anchor
/// string.
fn classify_user_message(item: &RolloutItem) -> Option<String> {
    let RolloutItem::ResponseItem(ResponseItem::Message { role, content, .. }) = item else {
        return None;
    };
    if role != "user" {
        return None;
    }
    if is_contextual_user_message(content) {
        return None;
    }
    Some(message_text(content))
}

/// Return `(rollout_index, text)` for every real user-message boundary in
/// `items`, in order, applying any `ThreadRolledBack` markers the same way
/// codex-core's `user_message_positions_in_rollout` does (a rollback drops
/// the trailing N user-message boundaries so indexing reflects the
/// post-rollback effective history).
///
/// The position of an entry *within the returned `Vec`* is the 0-based
/// index `ForkSnapshot::TruncateBeforeNthUserMessage` expects -- i.e. the
/// `k`-th entry here is exactly `n_from_start = k`.
fn user_message_positions_in_rollout(items: &[RolloutItem]) -> Vec<(usize, String)> {
    let mut positions: Vec<(usize, String)> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        if let RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) = item {
            let num_turns = usize::try_from(rollback.num_turns).unwrap_or(usize::MAX);
            let new_len = positions.len().saturating_sub(num_turns);
            positions.truncate(new_len);
            continue;
        }
        if let Some(text) = classify_user_message(item) {
            positions.push((idx, text));
        }
    }
    positions
}

/// The anchor text/occurrence did not match any real user message in the
/// source rollout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorNotFound;

/// Find the 0-based `ForkSnapshot::TruncateBeforeNthUserMessage` index for
/// `anchor_text`/`occurrence` within `items`.
///
/// Matching strategy (see the design doc's "关键设计决定"): try exact
/// equality first; if no rollout user message is an exact match, fall back
/// to containment (the rollout text contains `anchor_text` as a substring,
/// to tolerate a host that prefixed/rewrote the text it actually sent).
/// Within whichever match set is non-empty, `occurrence` (1-based) selects
/// which match to use. Returns [`AnchorNotFound`] if `occurrence` is `0`,
/// out of range, or no user message matches at all.
pub fn find_fork_anchor(
    items: &[RolloutItem],
    anchor_text: &str,
    occurrence: usize,
) -> Result<usize, AnchorNotFound> {
    if occurrence == 0 {
        return Err(AnchorNotFound);
    }

    let positions = user_message_positions_in_rollout(items);

    let exact_ks: Vec<usize> = positions
        .iter()
        .enumerate()
        .filter(|(_, (_, text))| text == anchor_text)
        .map(|(k, _)| k)
        .collect();

    let candidate_ks = if exact_ks.is_empty() {
        positions
            .iter()
            .enumerate()
            .filter(|(_, (_, text))| text.contains(anchor_text))
            .map(|(k, _)| k)
            .collect::<Vec<_>>()
    } else {
        exact_ks
    };

    candidate_ks
        .get(occurrence - 1)
        .copied()
        .ok_or(AnchorNotFound)
}

/// Log a diagnostic warning (never an error) when the caller's `nthHint`
/// (1-based UI ordinal) disagrees with the text-matched 0-based index `k`.
/// The text match is always authoritative; this exists purely so drift
/// between the UI's and rollout's user-message counts (silent monitor
/// wake-ups, compaction, ...) is observable in logs.
pub fn warn_on_nth_hint_mismatch(nth_hint: Option<usize>, source_session_id: &str, k: usize) {
    let Some(hint) = nth_hint else {
        return;
    };
    if hint.checked_sub(1) != Some(k) {
        tracing::warn!(
            source_session_id,
            nth_hint = hint,
            matched_k = k,
            "_drama/session/fork: nthHint diagnostic mismatch against anchor-text match \
             (using the text match as source of truth)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::ThreadRolledBackEvent;

    fn user_message(text: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        })
    }

    fn agent_message(text: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        })
    }

    fn contextual_user_message(tag_body: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: tag_body.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        })
    }

    fn three_turn_rollout() -> Vec<RolloutItem> {
        vec![
            user_message("hello there"),
            agent_message("hi, how can I help?"),
            user_message("please write a poem"),
            agent_message("here's a poem..."),
            user_message("thanks, one more please"),
            agent_message("sure, here's another..."),
        ]
    }

    // --- Scenario 1: fork at the 2nd of 3 turns -> only the 1st turn remains ---

    #[test]
    fn fork_anchor_second_user_message_yields_k_one() {
        let items = three_turn_rollout();
        let k = find_fork_anchor(&items, "please write a poem", 1).expect("anchor should match");
        // 0-based: "hello there" is k=0, "please write a poem" is k=1.
        assert_eq!(k, 1);
    }

    // --- Scenario 2: repeated text, occurrence=2 selects the second one ---

    #[test]
    fn fork_anchor_repeated_text_occurrence_selects_second_match() {
        let items = vec![
            user_message("continue"),
            agent_message("ok"),
            user_message("continue"),
            agent_message("ok again"),
            user_message("continue"),
        ];
        let k_first = find_fork_anchor(&items, "continue", 1).expect("first occurrence matches");
        let k_second = find_fork_anchor(&items, "continue", 2).expect("second occurrence matches");
        let k_third = find_fork_anchor(&items, "continue", 3).expect("third occurrence matches");
        assert_eq!(k_first, 0);
        assert_eq!(k_second, 1);
        assert_eq!(k_third, 2);
    }

    // --- Scenario 3: contextual injections don't perturb user-message counting ---

    #[test]
    fn fork_anchor_ignores_contextual_fragments() {
        let items = vec![
            contextual_user_message("<environment_context>\ncwd: /repo\n</environment_context>"),
            user_message("hello there"),
            agent_message("hi"),
            contextual_user_message("<external_docs>some injected reference doc</external_docs>"),
            user_message("please write a poem"),
            agent_message("a poem"),
        ];
        // Despite two contextual injections ahead of / between the real user
        // messages, "please write a poem" must still resolve to k=1 (the
        // *second* real user message), not k=3 (its raw rollout index).
        let k = find_fork_anchor(&items, "please write a poem", 1).expect("should match");
        assert_eq!(k, 1);

        // And the first real user message is still k=0.
        let k0 = find_fork_anchor(&items, "hello there", 1).expect("should match");
        assert_eq!(k0, 0);
    }

    // --- Scenario 4: no match -> AnchorNotFound ---

    #[test]
    fn fork_anchor_not_found_when_text_absent() {
        let items = three_turn_rollout();
        let result = find_fork_anchor(&items, "this text was never said", 1);
        assert_eq!(result, Err(AnchorNotFound));
    }

    #[test]
    fn fork_anchor_not_found_when_occurrence_out_of_range() {
        let items = three_turn_rollout();
        // Only one match exists; asking for the 2nd occurrence must fail,
        // not silently fall back to the 1st.
        let result = find_fork_anchor(&items, "please write a poem", 2);
        assert_eq!(result, Err(AnchorNotFound));
    }

    #[test]
    fn fork_anchor_not_found_when_occurrence_is_zero() {
        let items = three_turn_rollout();
        let result = find_fork_anchor(&items, "please write a poem", 0);
        assert_eq!(result, Err(AnchorNotFound));
    }

    // --- Scenario 5: containment fallback (host-rewritten/prefixed text) ---

    #[test]
    fn fork_anchor_falls_back_to_containment_match() {
        let items = vec![
            user_message("[from web] please write a poem about the sea"),
            agent_message("a poem about the sea..."),
        ];
        // No rollout message is *exactly* "please write a poem about the
        // sea" (the host prefixed it), so this must fall back to containment.
        let k = find_fork_anchor(&items, "please write a poem about the sea", 1)
            .expect("containment match should succeed");
        assert_eq!(k, 0);
    }

    #[test]
    fn fork_anchor_prefers_exact_match_over_containment() {
        let items = vec![
            user_message("please write a poem about the sea, extended remix"),
            agent_message("ok"),
            user_message("please write a poem about the sea"),
        ];
        // An exact match exists (k=1); it must win over the containment
        // match at k=0, even though k=0 comes first and also contains the
        // anchor text as a substring.
        let k = find_fork_anchor(&items, "please write a poem about the sea", 1)
            .expect("exact match should be found");
        assert_eq!(k, 1);
    }

    // --- Additional coverage: ThreadRolledBack markers drop trailing boundaries ---

    #[test]
    fn thread_rolled_back_drops_trailing_user_message_boundaries() {
        let mut items = three_turn_rollout();
        // Roll back the last turn: the 3rd user message ("thanks, one more
        // please") should no longer be a valid fork boundary.
        items.push(RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
            ThreadRolledBackEvent { num_turns: 1 },
        )));

        let result = find_fork_anchor(&items, "thanks, one more please", 1);
        assert_eq!(result, Err(AnchorNotFound));

        // The earlier two boundaries remain valid.
        let k = find_fork_anchor(&items, "please write a poem", 1).expect("still present");
        assert_eq!(k, 1);
    }

    #[test]
    fn multimodal_message_counts_as_one_user_message() {
        let items = vec![
            RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![
                    ContentItem::InputText {
                        text: "look at this".to_string(),
                    },
                    ContentItem::InputImage {
                        image_url: "data:image/png;base64,AAAA".to_string(),
                        detail: None,
                    },
                ],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }),
            agent_message("nice image"),
            user_message("ok next"),
        ];
        let k = find_fork_anchor(&items, "ok next", 1).expect("should match");
        // The multimodal message is exactly one boundary (k=0), not two.
        assert_eq!(k, 1);
    }

    #[test]
    fn wire_request_uses_camel_case_field_names() {
        let json = serde_json::json!({
            "cwd": "/repo",
            "sourceSessionId": "abc-123",
            "anchor": {"text": "hello", "occurrence": 1},
            "nthHint": 2,
            "mcpServers": [],
        });
        let request: ForkSessionRequest =
            serde_json::from_value(json).expect("camelCase wire shape should deserialize");
        assert_eq!(request.cwd, PathBuf::from("/repo"));
        assert_eq!(request.source_session_id.0.as_ref(), "abc-123");
        assert_eq!(request.anchor.text, "hello");
        assert_eq!(request.anchor.occurrence, 1);
        assert_eq!(request.nth_hint, Some(2));
    }

    #[test]
    fn wire_response_serializes_session_id_camel_case() {
        let response = ForkSessionResponse {
            session_id: SessionId::new("new-session-id"),
        };
        let value = serde_json::to_value(&response).expect("should serialize");
        assert_eq!(value["sessionId"], "new-session-id");
    }

    #[test]
    fn nth_hint_mismatch_does_not_panic_and_matching_hint_is_silent() {
        // These are smoke tests to confirm the diagnostic helper never
        // affects control flow; behavior itself is only observable via
        // `tracing` output.
        warn_on_nth_hint_mismatch(None, "session-a", 0);
        warn_on_nth_hint_mismatch(Some(1), "session-a", 0);
        warn_on_nth_hint_mismatch(Some(2), "session-a", 0);
    }
}
