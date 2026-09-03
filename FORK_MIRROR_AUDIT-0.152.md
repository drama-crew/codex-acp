# fork.rs 镜像审计：codex rust-v0.144.3 → rust-v0.152.1

`DRAMA_FORK.md` 的 Tracking workflow 第 3 条要求：凡 codex 变更影响 rollout 的用户消息
分类，必须把 codex-core 的私有 fragment registry 与截断规则和 `src/fork.rs` 的镜像逐条
比对。本次跨 8 个小版本，审计结论如下。

## Δ1 — 结构性变更（编译期可捕获，必须改）

1. `RolloutItem::ResponseItem` 的载荷从裸 `ResponseItem` 变成了一个记录结构，真正的
   `ResponseItem` 在其 `.item` 字段上。
   - 上游 `thread_rollout_truncation.rs` 的匹配从
     `RolloutItem::ResponseItem(item @ ResponseItem::Message { .. })`
     改成了 `RolloutItem::ResponseItem(item) if matches!(&item.item, ResponseItem::Message { .. })`。
   - `src/fork.rs::classify_user_message` 的 `let RolloutItem::ResponseItem(ResponseItem::Message
     { role, content, .. }) = item` **不再能编译**，必须同步改。
2. `RolloutItem` 与 `InitialHistory` 已从 `codex_protocol::protocol` **迁到新的 `codex-history`
   crate**。`src/fork.rs` 的 import 与 `Cargo.toml` 的依赖列表都要跟着改。
3. `truncate_rollout_before_nth_user_message_from_start` / `truncate_rollout_after_turn_id`
   的签名从 `&[RolloutItem]` 改为按值 `Vec<RolloutItem>`；新增
   `truncate_rollout_before_turn_id`（按 turn id fork）。

## Δ2 — 分类语义变更（静默风险，本次结论：无索引漂移）

`core/src/event_mapping.rs::parse_user_message` 在 0.152 里除了原有的**图片标签**跳过，
又增加了**音频标签**跳过：与 `ContentItem::InputAudio` 相邻的
`<local_audio>`/`<audio>` 开闭标签文本不计入渲染内容；同时新增
`ContentItem::InputAudio` 作为正式内容项。

对 `src/fork.rs` 的影响评估：

- **计数不受影响**。只含音频附件的用户消息，core 侧 `parse_user_message` 仍返回
  `Some`（content 里有 Audio）从而计为一条用户消息；fork.rs 侧 `is_contextual_user_message`
  同样判为非上下文并计入。两侧条数一致 → **`k` 不会漂移**，不存在静默截断错位。
- **锚点文本**：fork.rs 的 `message_text` 会把所有 `InputText` 拼进去，因此 rollout 侧字符串
  会多出 `<local_audio>…` 标签文本，而 host 拿到的锚点文本没有。这一点被
  `find_fork_anchor` 的**包含式回退匹配**吸收（rollout 文本 *contains* 锚点文本）——
  图片标签本来就是同样情形，属既有且已被容忍的行为。
- 结论：**不需要为音频改 `message_text`**。若将来要提高精确匹配命中率，可选地把
  图片/音频标签一起从 `message_text` 里剔除，但那是增强而非修复。

## Δ3 — fragment registry 比对（结论：集合未变）

`core/src/context/contextual_user_message.rs` 的匹配器集合在两个版本间**完全一致**（12 项）：

UserInstructions、EnvironmentsState、AdditionalContextUserFragment、Skill instructions、
UserShellCommand、TurnAborted、SubagentNotification、InternalModelContextFragment、
RecommendedPluginsInstructions、LegacyUnifiedExecProcessLimitWarning、
LegacyApplyPatchExecCommandWarning、LegacyModelMismatchWarning。

唯一差异是实现形态：0.144 用 `FragmentRegistrationProxy<SkillInstructions>` 静态注册，
0.152 改为直接调用 `codex_skills_extension::is_skill_prompt_fragment`——而后者只是
`SkillInstructions::matches_text` 的转发（`ext/skills/src/lib.rs:50-54`），**标记串没变**。
因此 `CONTEXTUAL_USER_TAG_PREFIXES` 无需增删。

## Δ4 — 新增但不在本路径上的分类器

0.152 新增 `is_user_authorization_message`（用 host annotation 而非文本标记判定）。
它只在 `core/src/context_manager/history.rs:255` 被使用，**不在
`user_message_positions_in_rollout` / `parse_turn_item` 这条 fork 索引计算路径上**，
因此不需要镜像。若将来它被引入截断路径，必须重新审计——它基于宿主注解而非文本，
纯文本镜像**无法**复现，届时需要改设计而不是加前缀。

## 结果

- [x] Δ1 的三项结构性改动落地，`cargo check --all-targets` 零错误
- [x] `cargo test` 全绿（82 项，其中 fork 相关 13 项、steer 6 项）
- [x] **镜像本身已被取消**：`classify_user_message` 改为直接调用
      `codex_core::parse_turn_item`（正是
      `user_message_positions_in_rollout` 用的那个谓词），漂移在构造上不可能发生。
      本文档描述的逐条比对因此不再是每次升级的必做项。
- [ ] **真二进制 `_drama/session/fork` e2e** —— 仍需在发布前跑一次。
      自动化差分测试覆盖的是**分类**，不覆盖其外围的 RPC/线程接线。

## 补记：差分测试当场抓出的两个真实缺陷

写 `mirror_agrees_with_codex_core_classifier` 时，它立刻判定旧镜像与 codex-core
不一致，暴露出两个**静默**缺陷（都属于"在错误的用户消息处 fork"）：

1. 镜像只匹配**前缀**，而 codex-core 的 `matches_marked_text` 要求
   **开标记与闭标记都在**。一条仅仅以 `<environment_context>` 开头的用户消息，
   codex-core 计数、镜像不计 → `k` 偏小 → fork 早了一条。
2. 镜像**大小写敏感**，codex-core 用 `eq_ignore_ascii_case`。

这两个都是人工逐条比对标记串**看不出来**的——比的是标记本身，而不是匹配规则。
