# 项目级 Claude 模型显示修复 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让项目级 Claude 路由在 proxy 模式下直接写 provider 的具体模型名（而非 cc-switch 自定义的 `claude-*` 别名），使 Claude Code `/model` 菜单显示真实模型名（如 `glm-4.5-air`），路由 correctness 由既有 `is_configured_target` 守卫承担。

**Architecture:** 撤回 commit `85fba5bc` 里「项目 writer 别名改写」这半道修复——删除 `write_claude_to_project` proxy 分支对 `rewrite_claude_model_env_to_aliases` 的调用，并清理该函数（仅项目路径使用，删后无引用）及其两个单测。同 commit 加的 `model_mapper::is_configured_target` 守卫保留并升为主力路由机制。

**Tech Stack:** Rust（src-tauri），tauri，serde_json，serial_test，tempfile。前端无改动。

## Global Constraints

- **提交策略：** 用户偏好自行决定提交节奏（既有 commit 均直接在 `main`）。本计划的 `git add/commit` 步骤**默认不自动 commit**——执行者完成所有代码改动后，由用户统一决定如何提交 / 是否开分支。`git add` 仅用于暂存以便用户复核 `git diff --cached`。
- **注释风格：** 双语注释（中文为主），match 周围文件既有风格（见 `project.rs` / `proxy.rs`）。
- **PR 门禁（提交前必过）：** src-tauri 侧 `cargo fmt` + `cargo clippy --all-targets` + `cargo test`；前端 `pnpm typecheck` + `pnpm format:check` + `pnpm test:unit`（本改动不涉及前端，预期全绿）。
- **不删：** `model_mapper::is_configured_target`（升为主力）、`build_claude_takeover_model_fields` / `push_claude_takeover_role_fields` / `CLAUDE_MODEL_OVERRIDE_ENV_KEYS`（全局 takeover 仍在用）。
- **Toolchain：** Rust MSRV 1.85；命令在 `src-tauri/` 下跑（cargo），前端命令在 repo 根跑（pnpm）。

---

## File Structure

| 文件 | 责任 | 本计划动作 |
|---|---|---|
| `src-tauri/src/services/project.rs` | `write_claude_to_project` 写项目 settings.local.json | proxy 分支删别名改写调用；反转一个单测 |
| `src-tauri/src/services/proxy.rs` | 代理服务（含 takeover 字段构造） | 删死函数 `rewrite_claude_model_env_to_aliases` + 其 2 单测 |
| `src-tauri/src/proxy/model_mapper.rs` | 模型名映射 + `is_configured_target` 守卫 | **不改** |
| `CHANGELOG.md` | 变更日志 | 修订 `[Unreleased] > Fixed` 一条目 |

---

### Task 1: 项目 proxy 分支改回写具体模型名（TDD）

**Files:**
- Modify: `src-tauri/src/services/project.rs` — `write_claude_to_project` 的 proxy 分支（约 373–383 行）；测试 `write_claude_to_project_proxy_mode_rewrites_models_to_claude_aliases`（1088–1152 行）
- Modify: `CHANGELOG.md` — `[Unreleased] > Fixed`「项目路由模型错路由（核心 bug）」条目（49–51 行）

**Interfaces:**
- Consumes: `model_mapper::ModelMapping::is_configured_target`（既有，不变）承担路由；provider env（`ANTHROPIC_DEFAULT_*_MODEL` 具体名）原样落盘。
- Produces: 项目 `settings.local.json` proxy 模式 env 含具体模型名（无 `claude-*` 别名、无 `*_NAME`），`ANTHROPIC_BASE_URL/TOKEN` 指向 `…/claude/project/<id>/`。

- [ ] **Step 1: 反转单测（先写让它失败）**

把 `project.rs` 里整个测试（含上方 4 行 `///` doc 注释，起于 1088 行）替换为下面这段。函数名从 `..._rewrites_models_to_claude_aliases` 改为 `..._keeps_concrete_model_names`：

```rust
    /// proxy 模式下，项目 settings.local.json 必须保留 provider 的具体模型名原样写入
    /// （不改写成 claude-* 别名），这样 Claude Code /model 菜单显示真实模型名（如
    /// glm-4.5-air）。路由 correctness 由 model_mapper 的 is_configured_target 幂等
    /// 保护承担。仅 BASE_URL/TOKEN 覆盖为项目级代理端点。
    #[test]
    #[serial]
    fn write_claude_to_project_proxy_mode_keeps_concrete_model_names() {
        let home = TempHome::new();
        // 强制启用本地代理（项目级路由依赖 proxy 模式）。
        // AppSettings 用 serde rename_all="camelCase"，故键名是 enableLocalProxy。
        home.write_settings(&json!({ "enableLocalProxy": true }));

        let db = Database::memory().expect("memory db");
        let dir = TempDir::new().expect("tmp");
        let project_path = dir.path().to_string_lossy().to_string();

        // 复刻用户真实 GLM provider 配置（haiku=glm-4.5-air，default=glm-5.2）
        let provider = Provider::with_id(
            "glm".into(),
            "glm".into(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/anthropic",
                    "ANTHROPIC_AUTH_TOKEN": "tok-glm",
                    "ANTHROPIC_MODEL": "glm-5.2",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-4.5-air",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.1",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.2[1M]"
                }
            }),
            None,
        );
        db.save_provider(AppType::Claude.as_str(), &provider)
            .expect("save provider");

        let project = ProjectService::create(&db, req("SLG", &project_path)).expect("create");
        ProjectService::set_claude_provider(&db, &project.id, Some("glm")).expect("bind");

        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".claude").join("settings.local.json"))
                .expect("read"),
        )
        .expect("parse");
        let env = &v["env"];

        // 具体模型名原样保留（CC /model 菜单据此显示真实名）
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "glm-4.5-air");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "glm-5.1");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "glm-5.2[1M]");
        // 不写 claude-* 别名（cc-switch 自定义短别名 CC 不识别）
        assert!(
            !env["ANTHROPIC_DEFAULT_HAIKU_MODEL"]
                .as_str()
                .unwrap_or("")
                .starts_with("claude-"),
            "不应再写入 claude-* 别名，实际: {:?}",
            env["ANTHROPIC_DEFAULT_HAIKU_MODEL"]
        );
        // 不写 *_NAME 字段（具体名自显示，无需 _NAME）
        assert!(
            env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME").is_none(),
            "不应写入 _NAME 字段"
        );
        // ANTHROPIC_MODEL（default 档具体名）保留，不再被移除
        assert_eq!(env["ANTHROPIC_MODEL"], "glm-5.2");
        // BASE_URL 指向项目级代理端点
        assert!(
            env["ANTHROPIC_BASE_URL"]
                .as_str()
                .is_some_and(|u| u.contains(&format!("/claude/project/{}", project.id))),
            "BASE_URL 应指向项目级代理端点，实际: {:?}",
            env["ANTHROPIC_BASE_URL"]
        );
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run（在 `src-tauri/`）:
```bash
cargo test write_claude_to_project_proxy_mode_keeps_concrete_model_names -- --nocapture
```
Expected: **FAIL**——当前代码仍把模型名改写成别名，`assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "glm-4.5-air")` 不成立（实际为 `claude-haiku-4-5`）。

- [ ] **Step 3: 删除别名改写调用（最小实现）**

在 `project.rs` 的 `write_claude_to_project` proxy 分支，把这段（注释 + 调用 + 其后一行 `let listen`）：

```rust
            // 与全局 takeover 同构：把 provider 的具体模型名改写成 claude-* 别名，
            // 真名存 _NAME。Claude Code 据此发送别名，由代理映射到项目绑定 provider
            // 的真实模型——避免具体名落到 model_mapper 的 default 兜底被错误改写
            // （项目路由下「glm-4.5-air → glm-5.2」错路由的根因）。幂等的 model_mapper
            // 兜底仍在，但走别名后命中快速关键字分支，且与全局行为一致。
            // 仅改写 *_MODEL/*_NAME，BASE_URL/TOKEN 在下方另行覆盖为项目端点。
            crate::services::proxy::ProxyService::rewrite_claude_model_env_to_aliases(
                &mut sanitized,
            );
            let listen = futures::executor::block_on(db.get_global_proxy_config()).ok();
```

替换为：

```rust
            // 保留 provider 的具体模型名原样写入——Claude Code /model 菜单据此显示
            // 真实模型名（如 glm-4.5-air）；路由 correctness 由 model_mapper 的
            // is_configured_target 幂等保护承担（发出的具体名命中已配置目标即透传）。
            // 不写 claude-* 别名：那是 cc-switch 自定义短别名，CC 不识别会显示原始串。
            let listen = futures::executor::block_on(db.get_global_proxy_config()).ok();
```

- [ ] **Step 4: 运行测试，确认通过**

Run（在 `src-tauri/`）:
```bash
cargo test write_claude_to_project_proxy_mode_keeps_concrete_model_names -- --nocapture
```
Expected: **PASS**.

- [ ] **Step 5: fmt + clippy + 全量测试（确认无回归）**

Run（在 `src-tauri/`）:
```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
Expected: 全绿。`rewrite_claude_model_env_to_aliases` 是 `pub(crate)` 且仍被自己的两个单测引用，故本步**不会**触发 dead_code 警告（Task 2 删除函数+其测试后才彻底清除）。

- [ ] **Step 6: 修订 CHANGELOG 条目**

`CHANGELOG.md` `[Unreleased] > Fixed` 下，把「项目路由模型错路由（核心 bug）」整条（49–51 行）替换为反映最终设计的版本。

找到（49–51 行）:
```markdown
- **项目路由模型错路由（核心 bug）**：自定义工程 + 自定义 provider 下，Claude Code 发出的**具体模型名**被 `model_mapper` 当成未知模型落到 `ANTHROPIC_MODEL`（default 档），**真实路由到错误模型**——实测 SLG 项目 `glm-4.5-air → glm-5.2`、Information 项目 `minimax-m2.5 → MiniMax-M3`（路由到更贵的 opus 档，已实际多花费用；全局 provider 因写 `claude-*` 别名而不受影响）。根因：项目 `settings.local.json` 直接写入 provider 的具体模型名（`ANTHROPIC_DEFAULT_HAIKU_MODEL=glm-4.5-air`），Claude Code 据此发送具体名；而全局 takeover 会改写成 `claude-*` 别名由代理映射——项目 writer 缺这步。两道互补修复：
  - **`model_mapper` 幂等保护**：入参已是某档已配置的目标模型时原样透传（新增 `is_configured_target`，含 `[1M]` 后缀剥离比对），不再走 default 兜底。放在关键字匹配**之后**——`claude-*` 别名（全局常见路径）零额外开销；检查本身至多 5 次 `eq_ignore_ascii_case`、零堆分配、短路。
  - **项目 writer 与全局 takeover 同构**：proxy 模式下复用新抽出的 `ProxyService::rewrite_claude_model_env_to_aliases`，把具体模型名改写成 `claude-*` 别名（真名存 `*_NAME`），由代理映射。新写入的项目 settings.local.json 走别名 + 快速关键字分支；**现存文件**由幂等保护兜底（或在项目设置里重选 provider 触发重写）。
```

替换为:
```markdown
- **项目路由模型错路由（核心 bug）**：自定义工程 + 自定义 provider 下，Claude Code 发出的**具体模型名**被 `model_mapper` 当成未知模型落到 `ANTHROPIC_MODEL`（default 档），**真实路由到错误模型**——实测 SLG 项目 `glm-4.5-air → glm-5.2`、Information 项目 `minimax-m2.5 → MiniMax-M3`（路由到更贵的 opus 档，已实际多花费用）。修复（守卫为主，直接写具体名）：
  - **`model_mapper` 幂等保护（主力）**：入参已是某档已配置的目标模型时原样透传（新增 `is_configured_target`，含 `[1M]` 后缀剥离比对），不再走 default 兜底。放在关键字匹配**之后**——`claude-*` 别名（全局常见路径）零额外开销；检查本身至多 5 次 `eq_ignore_ascii_case`、零堆分配、短路。
  - **项目 writer 直接写具体模型名**：proxy 模式下保留 provider 的具体模型名原样写入（仅覆盖 `BASE_URL/TOKEN` 指向项目端点），由上述守卫承担路由——Claude Code `/model` 菜单据此显示真实模型名（如 `glm-4.5-air`）。**不写 `claude-*` 别名**：那是 cc-switch 自定义短别名，CC 不识别、菜单会显示原始别名串。`85fba5bc` 曾额外用「别名改写」一道，现确认守卫单独即足、别名改写多余且引入显示问题，故撤回并删多余的 `rewrite_claude_model_env_to_aliases`。**现存已写入别名的项目文件**仍由守卫保证路由正确；显示需在项目设置重选 provider 触发覆盖。
```

- [ ] **Step 7: 暂存（不自动 commit）**

```bash
git add src-tauri/src/services/project.rs CHANGELOG.md
```

---

### Task 2: 删除死代码 `rewrite_claude_model_env_to_aliases` + 其两个单测

**前提：** Task 1 已删除唯一调用点（`project.rs`）。删后该函数仅被自己的两个单测引用。

**Files:**
- Modify: `src-tauri/src/services/proxy.rs` — 函数 `rewrite_claude_model_env_to_aliases`（256–276 行，含 doc 注释）；两个单测（2871–2915 行）

**Interfaces:**
- 删除 `pub(crate) fn rewrite_claude_model_env_to_aliases`（无外部消费者）。
- **保留** `build_claude_takeover_model_fields` / `push_claude_takeover_role_fields` / `CLAUDE_MODEL_OVERRIDE_ENV_KEYS`（全局 takeover 仍用）。

- [ ] **Step 1: 删除函数定义（含 doc 注释）**

在 `proxy.rs` 删除 256–276 行整段（含尾随空行，使删除后 `build_claude_takeover_model_fields` 闭合 `}` 与下一个 fn `push_claude_takeover_role_fields` 之间保留一个空行）：

```rust
    /// 把 Claude 配置 env 里的具体模型名改写成稳定的 `claude-*` 角色别名，
    /// 真实模型名存入对应 `*_NAME` 字段。供「项目级路由」在 proxy 模式下复用
    /// 全局 takeover 的同名逻辑——让 Claude Code 始终发送别名，由本地代理映射到
    /// 当前供应商的真实模型，避免具体名落到 [`model_mapper`] 的 default 兜底。
    ///
    /// 仅改写 `*_MODEL` / `*_NAME` / legacy `ANTHROPIC_MODEL` 等键，**不动**
    /// `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`（项目侧由调用方另行设置端点与令牌）。
    /// 调用方负责确保仅在 proxy 模式下调用：直连模式下必须保留具体模型名。
    pub(crate) fn rewrite_claude_model_env_to_aliases(config: &mut Value) {
        // 必须先 snapshot：build_ 内部读取 env，之后的 remove/insert 不能影响它
        let fields = Self::build_claude_takeover_model_fields(config);
        let Some(env) = config.get_mut("env").and_then(Value::as_object_mut) else {
            return;
        };
        for key in CLAUDE_MODEL_OVERRIDE_ENV_KEYS {
            env.remove(key);
        }
        for (key, value) in fields {
            env.insert(key.to_string(), Value::String(value));
        }
    }

```

- [ ] **Step 2: 删除其两个单测**

在 `proxy.rs` 测试模块，删除两个测试（从上一个测试 `managed_account_claude_takeover_..._keeps_auth_token` 闭合 `}`（2869）后的空行起，到 `rewrite_claude_model_env_to_aliases_noop_without_model_env` 闭合 `}`（2915）止）。删除后，2869 的 `}` 与下一个测试 `managed_account_claude_takeover_sources_copilot_models_from_provider`（原 2917）之间保留一个空行：

```rust

    #[test]
    fn rewrite_claude_model_env_to_aliases_matches_global_takeover_semantics() {
        // 项目级路由复用全局 takeover 的模型别名改写：具体模型名 → claude-* 别名，
        // 真名存 _NAME；ANTHROPIC_MODEL 移除；BASE_URL/TOKEN 不动（由项目侧另行设置）。
        let mut config = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/anthropic",
                "ANTHROPIC_AUTH_TOKEN": "tok-glm",
                "ANTHROPIC_MODEL": "glm-5.2",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-4.5-air",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.1",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.2[1M]"
            }
        });
        ProxyService::rewrite_claude_model_env_to_aliases(&mut config);

        let env = config["env"].as_object().expect("env present");
        // 具体名 → 标准 claude-* 别名（opus 带 [1M]）
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-haiku-4-5");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "claude-sonnet-4-6");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "claude-opus-4-8[1M]");
        // 真实模型名存入 _NAME
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME"], "glm-4.5-air");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL_NAME"], "glm-5.2");
        // ANTHROPIC_MODEL 被移除（交给代理 default 档兜底，与全局 takeover 一致）
        assert!(env.get("ANTHROPIC_MODEL").is_none());
        // BASE_URL / TOKEN 不受影响（项目侧自行覆盖为本地代理端点）
        assert_eq!(
            env["ANTHROPIC_BASE_URL"],
            "https://open.bigmodel.cn/api/anthropic"
        );
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "tok-glm");
    }

    #[test]
    fn rewrite_claude_model_env_to_aliases_noop_without_model_env() {
        // 没有配置模型映射时不应崩、不应误删其它 env
        let mut config = json!({
            "env": { "ANTHROPIC_BASE_URL": "https://x", "ANTHROPIC_AUTH_TOKEN": "t" }
        });
        ProxyService::rewrite_claude_model_env_to_aliases(&mut config);
        let env = config["env"].as_object().unwrap();
        assert_eq!(env["ANTHROPIC_BASE_URL"], "https://x");
        assert_eq!(env.len(), 2, "无模型字段时不应当插入任何别名");
    }
```

- [ ] **Step 3: 构建确认无残留引用**

Run（在 `src-tauri/`）:
```bash
cargo build
```
Expected: 编译通过。若遗漏任何调用点，会报 `cannot find function rewrite_claude_model_env_to_aliases` —— 此时补删遗漏点。

- [ ] **Step 4: fmt + clippy + 全量测试**

Run（在 `src-tauri/`）:
```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
Expected: 全绿，无 dead_code 警告。重点确认：`model_mapper::test_concrete_target_model_passes_through_unchanged`（守卫主力）仍通过；`write_claude_to_project_proxy_mode_keeps_concrete_model_names`（Task 1）仍通过。

- [ ] **Step 5: 前端 PR 门禁（确认无前端波及）**

Run（repo 根）:
```bash
pnpm typecheck
pnpm format:check
pnpm test:unit
```
Expected: 全绿（`SetProviderResult` 等前端类型不变；本改动纯后端）。

- [ ] **Step 6: 暂存（不自动 commit）**

```bash
git add src-tauri/src/services/proxy.rs
```

---

### Task 3: 实跑人工验证（显示 + 路由 + 归因）

**Files:** 无（运行时验证，非自动化）。

- [ ] **Step 1: 起开发版，绑定一个多档具体名的 provider 到某项目**

`pnpm dev`（或 `pnpm tauri build --debug` 装出的版本），在「项目」页选一个真实项目，绑定一个 provider（如 GLM，env 含 `ANTHROPIC_DEFAULT_HAIKU_MODEL=glm-4.5-air` / `ANTHROPIC_DEFAULT_OPUS_MODEL=glm-5.2[1M]` 等），确保本地代理已启用。

- [ ] **Step 2: 检查写出的 settings.local.json**

打开 `<项目根>/.claude/settings.local.json`，确认 `env`：
- `ANTHROPIC_DEFAULT_HAIKU_MODEL = glm-4.5-air`（具体名，**非** `claude-haiku-4-5`）
- 无 `ANTHROPIC_DEFAULT_*_MODEL_NAME` 字段
- `ANTHROPIC_BASE_URL` 指向 `http://127.0.0.1:<port>/claude/project/<id>/`
- `ANTHROPIC_MODEL = glm-5.2`（保留，未被移除）

- [ ] **Step 3: 在项目根起 Claude Code，查 `/model` 菜单**

在该项目根目录跑 `claude`，`/model` 查看每个档位：应显示真实模型名（如 `glm-4.5-air`、`glm-5.1`、`glm-5.2[1M]`），**不再是** `claude-haiku-4-5` 这类原始别名串。

- [ ] **Step 4: 跑一轮对话，确认路由 + usage 归因**

发一条流式消息，确认：
- 响应正常，模型路由到所选档位（不串档、不落到错误模型）；
- cc-switch「用量明细」里该请求「工程」列命中本项目、模型列显示具体名、计费正常。

---

## Self-Review

**1. Spec 覆盖：**
- §1 核心改动（删别名调用）→ Task 1 Step 3 ✓
- §2 保留 `is_configured_target` 守卫 → Global Constraints + 不改 `model_mapper.rs` ✓
- §3 清理死代码（函数 + 2 测试）→ Task 2 Step 1–2 ✓
- §4 测试反转 → Task 1 Step 1 ✓
- §5 CHANGELOG → Task 1 Step 6 ✓
- 边界（不做自动迁移 / `[1M]` 不美化 / 全局 takeover 不动）→ Global Constraints + CHANGELOG 文案 ✓

**2. 占位符扫描：** 无 TBD/TODO/「类似上」；每个代码步骤含可直接照贴的具体代码，每个验证步骤含确切命令与预期输出 ✓

**3. 类型/命名一致：**
- 新测试名 `write_claude_to_project_proxy_mode_keeps_concrete_model_names` 在 Task 1 Step 1/2/4 与 Self-Review 一致 ✓
- 被删函数名 `rewrite_claude_model_env_to_aliases` 在 Task 1（调用方删除）与 Task 2（定义+测试删除）一致 ✓
- `is_configured_target`、`CLAUDE_MODEL_OVERRIDE_ENV_KEYS` 等保留项命名一致 ✓

**4. 对 spec 的一处合理细化：** spec §5 写「Unreleased 加一条 Fixed」，计划改为「修订既有 `[Unreleased] > Fixed` 条目整体」——因该条目处于未发布区（可变），原条目描述的正是被撤回的别名改写行为，整体修订比新增一条更避免自相矛盾。属实现层面对 spec 文案的合理细化，语义不变。
