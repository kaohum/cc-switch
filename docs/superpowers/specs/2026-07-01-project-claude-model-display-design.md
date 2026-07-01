# 项目级 Claude 路由：改回直接写具体模型名

- **日期：** 2026-07-01
- **状态：** 已批准（待实现）
- **关联 commit：** `85fba5bc`（引入别名改写）、`d36692a1`（项目路由隔离）

## 背景

`85fba5bc` 为修复「项目级 provider 绑定后模型被错误路由」（SLG `glm-4.5-air → glm-5.2`、Information `minimax-m2.5 → MiniMax-M3`）加了两道互补修复：

1. **别名改写（主）：** 项目 writer 在 proxy 模式下复用全局 takeover 的 `rewrite_claude_model_env_to_aliases`，把 provider 的具体模型名改写成 `claude-*` 别名（真名存 `*_NAME`），让本地代理 `model_mapper` 走关键字分支稳定映射。
2. **`is_configured_target` 守卫（备）：** `model_mapper` 入参已是某档已配置目标模型时原样透传，不走 default 兜底——保护现存已写入具体名的项目文件。

## 问题

别名改写引入了**显示问题**：Claude Code 的 `/model` 菜单每个档位显示成原始别名串（`claude-haiku-4-5` 等），而非真实模型名（`glm-4.5-air`）。

**根因（经 claude-code-guide 核实官方文档）：**

- `ANTHROPIC_DEFAULT_*_MODEL_NAME` 这套显示名 env 在「`ANTHROPIC_BASE_URL` 指向 LLM gateway/代理」时按文档**本应生效**，但实测未盖住别名显示（疑似 CC 版本或 settings 层级问题，无法在代码侧定论）。
- 更本质：CC 只对**它认识的模型 id** 显示内置友好名；对**不认识的 id 一律显示原始字符串**，除非被 `display_name`/`_NAME` 覆盖。cc-switch 写的 `claude-haiku-4-5` / `claude-sonnet-4-6` / `claude-opus-4-8` 是 cc-switch **自定义的短别名**，不在 CC 的识别表内 → 当成未知 id → 显示原始别名串。
- 排除「全局靠 `/v1/models` 返回标签」假设：根路径 `/v1/models`（`handle_models`）返回的是 Codex 模型目录，与 Claude 无关；带 `labelOverride` 的 `handle_claude_desktop_models` 只挂在 `/claude-desktop/v1/models`，是 Claude Desktop 专属。项目路径 `/claude/project/<id>/` 连 `/v1/models` 都没挂。

## 方案

**关键认知：** `is_configured_target` 守卫单独就足以保证路由正确性（逐档验证见下）。别名改写既是显示问题的元凶，又是多余的一道。故**撤掉别名改写，让项目路径直接写 provider 的具体模型名**，守卫升为主力。

路由正确性验证（CC 每个档位发出的都是 provider 自己配的具体名，必然命中守卫原样透传）：

| 档位 | CC 发出 | 守卫命中 | 结果 |
|---|---|---|---|
| haiku | `glm-4.5-air` | haiku_model | 透传 ✓ |
| sonnet | `glm-5.1` | sonnet_model | 透传 ✓ |
| opus | `glm-5.2[1M]` | opus_model（`[1M]` 大小写不敏感） | 透传，下游剥 `[1M]` → `glm-5.2` ✓ |
| default | `glm-5.2` | default_model | 透传 ✓ |
| 真正未识别 | 任意 | 否 | 落 default（= `glm-5.2`），符合默认档语义 ✓ |

## 改动清单

### 1. 核心行为变更 — `src-tauri/src/services/project.rs`

`write_claude_to_project` 的 proxy 分支（约 `373–422` 行）删掉「与全局 takeover 同构」注释 + `rewrite_claude_model_env_to_aliases(&mut sanitized)` 调用（`375–383` 行）。BASE_URL/TOKEN 指向项目端点的覆盖逻辑不动。

改后写出的 `env`：

```jsonc
"ANTHROPIC_BASE_URL": "http://127.0.0.1:port/claude/project/<id>/",
"ANTHROPIC_AUTH_TOKEN": "ccs-project-<id>",
"ANTHROPIC_MODEL": "glm-5.2",
"ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-4.5-air",
"ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.1",
"ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.2[1M]"
```

不再有 `claude-*` 别名，也不再写 `*_NAME`。行为与「直连模式（proxy 关）」对齐，仅 BASE_URL/TOKEN 指向本地代理。

### 2. 保留 — `src-tauri/src/proxy/model_mapper.rs`

`is_configured_target` 及其调用**不动**，由备份升为主力路由机制。

### 3. 清理死代码 — `src-tauri/src/services/proxy.rs`

撤掉项目调用后，`rewrite_claude_model_env_to_aliases`（`264–276` 行）无任何调用方（全局 takeover 走 `apply_claude_takeover_fields_for_provider`，不依赖它）。删除：

- `pub(crate) fn rewrite_claude_model_env_to_aliases`（`264–276`）
- 测试 `rewrite_claude_model_env_to_aliases_matches_global_takeover_semantics`（`2872`）
- 测试 `rewrite_claude_model_env_to_aliases_noop_without_model_env`（`2906`）

`build_claude_takeover_model_fields` / `push_claude_takeover_role_fields` **保留**——全局 takeover 仍在用。

### 4. 测试反转 — `src-tauri/src/services/project.rs`

`write_claude_to_project_proxy_mode_rewrites_models_to_claude_aliases`（`1094`）改名为 `write_claude_to_project_proxy_mode_keeps_concrete_model_names`，断言改为：

- `ANTHROPIC_DEFAULT_HAIKU_MODEL == "glm-4.5-air"`（具体名原样保留）
- `ANTHROPIC_DEFAULT_SONNET_MODEL == "glm-5.1"`
- `ANTHROPIC_DEFAULT_OPUS_MODEL == "glm-5.2[1M]"`
- 不存在 `ANTHROPIC_DEFAULT_*_MODEL_NAME` 字段（未写别名机制）
- 不存在 `claude-*` 别名值
- `ANTHROPIC_BASE_URL` 含 `/claude/project/<id>/`

此测试为本改动的**回归保护**。`model_mapper.rs::test_concrete_target_model_passes_through_unchanged` 保留原样。

### 5. CHANGELOG

`Unreleased` 加一条 `Fixed`：撤回 `85fba5bc` 的项目别名改写、改回直接写具体模型名。理由：别名是 cc-switch 自定义短别名，CC 不识别 → `/model` 菜单显示原始别名串；`is_configured_target` 守卫已足以保证路由正确，别名改写多余且引入显示问题。

## 不在范围

- **现存已写入别名的项目文件：** 路由仍正确（守卫兜底）；显示需用户在项目设置里重选一次 provider 触发 `write_claude_to_project` 覆盖。不做一次性自动迁移（YAGNI）。CHANGELOG 写明。
- **`[1M]` 显示：** opus 档菜单显示 `glm-5.2[1M]`（provider 原值含 1M 标记）。可接受，本次不美化。
- **全局 takeover 显示：** 全局仍写别名，行为不变。如需统一另开 follow-up。

## 验证

- `pnpm typecheck` / `pnpm format:check` / `pnpm test:unit`（前端无改动，预期通过）
- `cargo fmt` / `cargo clippy` / `cargo test`（src-tauri）：重点关注反转后的项目测试 + model_mapper 守卫测试 + 删除函数后无残留引用
- 手动：绑定一个有多档具体名的 provider 到项目，确认 CC `/model` 菜单显示真实模型名、且请求正确路由（usage 归因正常）
