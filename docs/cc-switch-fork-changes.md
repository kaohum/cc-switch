# CC-Switch Fork 修改记录（Project Workspace & Proxy 项目级路由）

> 完整的工作记录，便于下次继续修改时回顾决策、环境、进度、未解决问题。
>
> **关联文档**：`D:\work\information\dev\cc-switch-fork-plan.md`（开发计划）· `E:\Projects\cc-switch\CHANGELOG.md`（用户视角）· `E:\Projects\cc-switch\.superpowers\sdd\progress.md`（SDD 进度 + 全局约束）

---

## 1. 仓库状态

| 项 | 值 |
|----|----|
| 上游 | `https://github.com/farion1231/cc-switch` |
| 我的 fork | `https://github.com/kaohum/cc-switch` |
| 本地路径 | `E:\Projects\cc-switch` |
| 功能分支 | `feature/project-workspace`（已 merge 到 `main`） |
| 当前 HEAD | `main` @ `9565c473`（GH） |
| base commit | `61d7ac01`（v3.16.4） |
| 9 个 commit | `1b6ab477` → `c4d343da` → `6a9af903` → `98ef6a0c` → `20472d93` → `5edd50cb` → `17935b08` → `bbf71a93` → `9565c473`（CI 修复） |

---

## 2. 功能范围（已实现）

### 2.1 核心能力
- **项目工程目录管理**：CRUD + 软删除/恢复 + 按 sort_index 排序
- **项目级 Claude Provider 绑定**：一个项目 → 一个 provider（引用全局 provider 池，不复制）
- **settings.local.json 写入**（合并模式）：保留 hooks/plugins/permissions 等用户原有配置
- **本地代理项目级路由**（方案 A）：每个项目独立路由到自己的 provider + 完整统计 + 格式转换
- **多实例**：多项目同时跑，互不干扰

### 2.2 UI 集成
- 独立「项目」标签页（顶部工具栏 📁 图标）
- 项目列表 + 详情（双栏布局）
- Provider 卡片显示使用此 provider 的工程标签（点击 → 项目设置）
- 用量明细表加「工程」列
- i18n：en / zh / zh-TW / ja

---

## 3. 实现里程碑（6 个）

| 里程碑 | 内容 | commit |
|--------|------|--------|
| **M1 数据层** | schema v11→v12（projects 表 + 2 索引） + `Project` struct + 8 个 DAO 方法 + `current_project_id` settings 字段 | `c4d343da` |
| **M1 服务/命令** | `ProjectService`（接受 `&Database` 可单测） + 11 个 Tauri command | `6a9af903` |
| **M2 写入** | `write_claude_to_project`：复用 `live.rs::build_effective_settings_with_common_config` + sanitize + 备份 .bak + 原子写 | `98ef6a0c` |
| **M3 启动器** | `open_project_terminal` 复用 `open_provider_terminal` | `20472d93` |
| **M4 前端** | `Project`/`RequestLogDetail` types + `useProjects` hook + `ProjectsPage` + `ProviderSelectForProject` + `ProjectFormDialog` | `5edd50cb` + `17935b08` |
| **M5 UI 集成** | App.tsx View 路由 + 工具栏 FolderKanban + 4 locale i18n | `17935b08` |
| **M6 文档** | CHANGELOG Unreleased section | `bbf71a93` |

后续迭代（在 Unreleased 块中）：

| 编号 | 内容 | commit |
|------|------|--------|
| **方案 A 后端** | `ProviderRouter::select_providers_for_project` + `RequestContext` 透传 project_id + proxy 路由 `/claude/project/{id}/v1/messages` + write_claude_to_project proxy 模式 | 合并进 98ef6a0c |
| **B1-B5 调整** | usage 归因 project_id（schema v13 + logger 透传）+ RequestLogDetail 加 project_name + 工程列 + provider 卡片工程标签 + 自定义命令 | 合并进 17935b08 |
| **修复合并模式** | settings.json → settings.local.json（Claude Code 官方约定，gitignore 默认不污染）+ 合并模式保留原配置 | 合并进 bbf71a93 |
| **修复 cwd + 持久化 + toast** | open_project_terminal 改用 `launch_terminal_running` 强制 cd + customCommand localStorage 持久化 + toast/settings UI 文案改 settings.local.json | 合并进 bbf71a93 |
| **Provider 标签跳转** | chip 改 emit `ccs-open-project-settings` 事件 → App.tsx listen → 跳项目页 + localStorage 记 id → ProjectsPage 选中 | 合并进 bbf71a93 |
| **CI workflow** | 简化 `.github/workflows/release.yml`（fork 友好无签名 secrets） + 关 `createUpdaterArtifacts` | `9565c473` |

---

## 4. 关键文件清单

### 新增（10 个）

| 文件 | 说明 |
|------|------|
| `src-tauri/src/database/dao/projects.rs` | Project struct + 8 个 DAO 方法 + 9 单元测试 |
| `src-tauri/src/services/project.rs` | ProjectService + CreateProjectRequest/UpdateProjectRequest + 12 单元测试 + write_claude_to_project + 4 写入测试 |
| `src-tauri/src/commands/project.rs` | 11 个 Tauri command + 3 validate_path 单元测试 |
| `src/components/projects/ProjectsPage.tsx` | 主页面（双栏布局） |
| `src/components/projects/ProjectFormDialog.tsx` | 新建/编辑对话框 |
| `src/components/projects/ProviderSelectForProject.tsx` | Claude provider 下拉选择器 |
| `src/hooks/useProjects.ts` | useState + sonner toast 风格的 hook |
| `src/lib/api/projects.ts` | 11 个 Tauri command 的 invoke 包装 |
| `src/types/project.ts` | TypeScript 类型定义 |
| `.github/workflows/release.yml` | CI（3 平台 build） |

### 修改（22 个）

| 文件 | 关键改动 |
|------|---------|
| `src-tauri/src/database/mod.rs` | `SCHEMA_VERSION: 11 → 12 → 13`，导出 `Project` |
| `src-tauri/src/database/schema.rs` | projects 表 + 2 索引 + `migrate_v11_to_v12` + `migrate_v12_to_v13`（用 `add_column_if_missing` 幂等） + `create_tables_on_conn` 加列 |
| `src-tauri/src/database/dao/mod.rs` | `pub mod projects` |
| `src-tauri/src/database/tests.rs` | migration_v11_to_v12 测试 |
| `src-tauri/src/settings.rs` | `current_project_id: Option<String>`（设备级，预留给未来） |
| `src-tauri/src/lib.rs` | 注册 11 个新 command 到 generate_handler |
| `src-tauri/src/commands/mod.rs` | mod project + pub use project |
| `src-tauri/src/services/mod.rs` | `pub use project::{CreateProjectRequest, ProjectService, UpdateProjectRequest}` |
| `src-tauri/src/proxy/provider_router.rs` | `select_providers_for_project` + 3 单元测试 |
| `src-tauri/src/proxy/handler_context.rs` | `RequestContext::new` 加 `project_id: Option<&str>` + struct 加 `project_id: Option<String>` 字段 |
| `src-tauri/src/proxy/handlers.rs` | `handle_messages_for_app` 加 `project_id` + `handle_project_messages` + `strip_prefix: Option<String>`（动态化以支持项目路径） |
| `src-tauri/src/proxy/server.rs` | 路由 `/claude/project/:project_id/v1/messages` |
| `src-tauri/src/proxy/response_processor.rs` | `log_usage_internal` 加 `project_id` + `spawn_log_usage` 从 ctx 读取（其他 5 处暂传 None） |
| `src-tauri/src/proxy/usage/logger.rs` | `RequestLog` 加 `project_id` + `log_with_calculation`/`log_error_with_context` 加 `project_id` 参数 |
| `src-tauri/src/services/usage_stats.rs` | `RequestLogDetail` 加 `project_name` + 2 个 SELECT 加 `(SELECT name FROM projects WHERE id = l.project_id) as project_name` |
| `src-tauri/src/commands/project.rs` | `open_project_terminal` 加 `customCommand: Option<String>` 参数；统一用 `launch_terminal_running(cd + cmd)` |
| `src-tauri/tauri.conf.json` | `createUpdaterArtifacts: false` |
| `src/App.tsx` | View 加 "projects" + 工具栏 FolderKanban + 路由 + 标题 + listen `ccs-open-project-settings` |
| `src/components/providers/ProviderCard.tsx` | `useQuery(listProjects)` + filter `claudeProviderId === provider.id` → 蓝色工程 chip；click → `emit('ccs-open-project-settings', p.id)` |
| `src/components/usage/RequestLogTable.tsx` | 在 provider 列前加「工程」列 |
| `src/types/usage.ts` | `RequestLog.projectName?: string` |
| `src/i18n/locales/{en,zh,zh-TW,ja}.json` | projects section（28 key）+ usage.project/noProject |
| `CHANGELOG.md` | Unreleased section |
| `.gitignore` | 移除 `.github` 排除（启用 CI） |

---

## 5. 数据库 Schema 演进

### v11 → v12
- 加 `projects` 表：
  ```sql
  CREATE TABLE projects (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      path TEXT NOT NULL,
      description TEXT,
      claude_provider_id TEXT,
      created_at INTEGER NOT NULL,
      updated_at INTEGER NOT NULL,
      last_written_at INTEGER,
      deleted_at INTEGER,
      sort_index INTEGER,
      icon TEXT,
      icon_color TEXT
  );
  CREATE INDEX idx_projects_deleted ON projects(deleted_at);
  CREATE INDEX idx_projects_sort ON projects(deleted_at, sort_index);
  ```

### v12 → v13
- `proxy_request_logs` 加列：
  ```sql
  ALTER TABLE proxy_request_logs ADD COLUMN project_id TEXT;
  CREATE INDEX idx_request_logs_project ON proxy_request_logs(project_id);
  ```

### ⚠️ 迁移陷阱（必看）
- `migrate_v12_to_v13` **必须用 `add_column_if_missing`**（不是裸 `ALTER TABLE ADD COLUMN`）
- 原因：`create_tables_on_conn` 已用裸 ALTER 加列（`let _` 忽略错误）；老库升级时 migrate 再次裸 ALTER → `duplicate column name: project_id`
- 修复 commit 在 bbf71a93 块内

---

## 6. Proxy 项目级路由（方案 A）架构

### 路由设计
```
项目 A: <A 项目根>/.claude/settings.local.json
  ANTHROPIC_BASE_URL = http://127.0.0.1:15721/claude/project/<A-id>
  ANTHROPIC_AUTH_TOKEN = ccs-project-<A-id>

项目 B: <B 项目根>/.claude/settings.local.json
  ANTHROPIC_BASE_URL = http://127.0.0.1:15721/claude/project/<B-id>
  ANTHROPIC_AUTH_TOKEN = ccs-project-<B-id>
```

### 请求流
1. claude CLI 在 `<A 项目根>` 启动 → 读 `settings.local.json` → 用 `localhost:15721/claude/project/<A-id>` 走 proxy
2. proxy `handle_project_messages` 提取 `project_id` → `RequestContext.project_id` → `select_providers_for_project(A-id)` → 路由到 A 的 provider
3. 格式转换 / 统计 / 故障转移复用现有 proxy 逻辑

### 关键实现
- `ProviderRouter::select_providers_for_project(project_id)`：从 projects 表读 `claude_provider_id` → `get_provider_by_id` → `vec![provider]`
- `RequestContext::new` 加 `project_id: Option<&str>` 参数：有 project_id → `select_providers_for_project`，无 → `select_providers(app_type_str)`
- `handle_messages_for_app` 加 `project_id` 参数透传
- `handle_project_messages`：axum `Path<String>` 提取 project_id → 调 `handle_messages_for_app`（动态 `strip_prefix` 改为 `String`）
- `server.rs` 路由：`/claude/project/:project_id/v1/messages`

---

## 7. 关键复用点

| 复用 | 来源 | 用途 |
|------|------|------|
| `build_effective_settings_with_common_config` | `services/provider/live.rs:483` | 构造 settings（+ common config merge） |
| `sanitize_claude_settings_for_live` | `services/provider/live.rs:24` | 去除 `api_format` 等内部字段 |
| `config::write_json_file` | `config.rs` | 原子写（temp + rename） |
| `Database::memory()` | `database/mod.rs:181` | 单元测试 in-memory db |
| `lock_conn!` 宏 | `database/mod.rs:62` | Mutex 加锁 |
| `AppError::localized` | `error.rs:87` | 中英错误信息 |
| `chrono::Utc::now().timestamp_millis()` | chrono | 时间戳 |
| `uuid::Uuid::new_v4().to_string()` | uuid | ID 生成 |
| `TanStack Query useQuery` | `@tanstack/react-query` | ProviderCard 查 projects |
| `tauri listen/emit` | `@tauri-apps/api/event` | 跨组件导航事件 |

---

## 8. 测试覆盖

### 后端单元测试（新增 31 个）
- 9 个 DAO 测试（`database/dao/projects.rs`）
- 12 个 Service 测试（`services/project.rs`）
- 4 个 write_claude_to_project 测试（`services/project.rs`）
- 1 个 create_with_provider 自动写测试
- 3 个 validate_project_path 测试（`commands/project.rs`）
- 3 个 select_providers_for_project 测试（`proxy/provider_router.rs`）
- + 现有 1700+ 测试无回归（基线 8 个失败是项目固有问题：anchored_upgrade_windows × 6, env var × 1, proxy start × 1）

### 前端测试
- 371 个现有 vitest 测试无回归
- typecheck TC=0

### 端到端测试
- 手动实测：项目根 settings.local.json 生成 + 合并保留 + proxy 路由 + 多项目多 provider

---

## 9. 环境与工具

### 本机环境
| 工具 | 版本 | 路径 |
|------|------|------|
| Node | v24.13.0 | `C:\Program Files\nodejs\node.exe` |
| pnpm | 11.9.0（corepack shim） | `C:\Program Files\nodejs\pnpm.ps1` |
| Rust | 1.95.0（rustup + 1.96.0 stable） | `C:\Users\chenhaoran\.cargo\bin\` |
| MSVC | VS 2019 Community | `C:\Program Files (x86)\Microsoft Visual Studio\2019\Community` |
| WiX | 3.14（手动下载） | `~/AppData/Roaming/tauri/WixTools/` |
| GitHub CLI | v2.95.0 | `C:\Program Files\GitHub CLI\gh.exe` |

### 关键环境注意
- ⚠️ **bash 工具 + PowerShell 语法**：`$env:Path = ...` 在 bash 不工作。要么用 bash 自身的 `export PATH=...`，要么用 PowerShell 工具
- ⚠️ **pnpm 是 PowerShell shim**：`.bat` 文件（用 cmd 解释）跑不了 `pnpm`——必须用 PowerShell 或 bash
- ⚠️ **GitHub Actions 缓存路径**：`~/AppData/Roaming/tauri/WixTools/`（不是 `~/.tauri/`）
- ⚠️ **WiX 下载需要 `NO_PROXY=*`**：tauri 用 reqwest，**走系统代理**会失败（`protocol: http response missing version`）。GitHub 直连是通的

### PATH 注意
bash 工具每次调用后 PATH 重置。每条 cargo/pnpm 命令前需 `export PATH="$HOME/.cargo/bin:$PATH"`，或先检查 `which cargo`。

---

## 10. 用户确认的设计决策（不变更）

| 决策 | 备注 |
|------|------|
| **方案 A**（项目根 settings.json → settings.local.json 合并） | 用户最早选；实测有效 |
| **真正多实例**（多 Claude 进程同时跑不同 provider） | 用户场景；方案 A + proxy 路由解决 |
| **独立「项目」标签页** | 顶层 UI 入口，与 Providers/MCP 松耦合 |
| **provider 引用全局池** | 一项目 = 一 provider_id（不复制 provider） |
| **MVP 仅 Claude** | Codex/Gemini/OpenCode 等后续 |
| **i18n 4 locale** | en / zh / zh-TW / ja |
| **设备级项目路径** | 不同机器项目路径不同，不随云同步 |
| **启动器：仅打开终端** | 用户实际接受了"启动 claude"（更完整） |
| **路径校验宽松** | 创建允许不存在；写入/启动时检查 |
| **写时机自动** | 选定 provider 立即触发 |
| **前端布局：Provider 卡片工程标签点击 → 项目设置**（不是终端）| 用户后期修正 |

---

## 11. 已知问题 / 后续待办

### 高优先级
- **B1 usage 归因覆盖不全**：5 处流式/转换路径闭包穿透暂传 `None`
  - 位置：`src-tauri/src/proxy/handlers.rs` line 556 / 945 / 1060 + `src-tauri/src/proxy/response_processor.rs` line 520 / 546
  - 修复：闭包内逐层 `let project_id = ctx.project_id.clone();` + 透传到 `log_usage(project_id)`
  - 影响：流式请求工程列可能为空
- **Mac build 阻塞**：GitHub 账号 billing 锁定，CI 失败（"account locked due to a billing issue"）
  - 解决：去 https://github.com/settings/billing 处理（更新卡 / 降 Free plan）
  - 解决后：`gh workflow run release.yml --repo kaohum/cc-switch` 重新触发

### 中优先级
- **写 settings.local.json 的 env 段可能丢失 proxy 字段**：proxy 模式下 `effective` 已含 ANTHROPIC_BASE_URL，sanitize 后再覆盖——已处理但需要回归测试
- **软删项目没清理 settings.local.json**：项目软删后 `<项目根>/.claude/settings.local.json` 残留（不是 bug，但要文档说明）
- **项目「重新写入」时 env 段整体覆盖**：`env` 仍整体替换（provider 配置整体性，避免切换时残留旧 key）；其他 top-level 字段改为深度合并（见下方「已修复」）。

### 已修复（2026-06-29 续）
- **✅ settings.local.json 只写 env、漏掉 common config 的其他字段**（原「内容不全」bug）：
  - **现象**：项目根 `settings.local.json` 只有 `env`，`effortLevel` / `enabledPlugins` / `extraKnownMarketplaces` / `statusLine` / `includeCoAuthoredBy` / `mcpServers` 等 common config 字段全部丢失 → 在项目里跑 claude 时插件 / statusline / MCP 全没了。
  - **根因**：`write_claude_to_project` 旧逻辑只 `existing.insert("env", sanitized_env)`，把 `build_effective_settings_with_common_config` 合并出的其他 top-level 字段全丢弃；而全局切换 `write_live_snapshot` 是写整个对象的——两者不一致。
  - **修复**：新增 `merge_full_settings_into_existing`（`services/project.rs`），语义：
    1. `env` 整体替换（切换 provider 不残留旧 `ANTHROPIC_*` key）；
    2. 其他 top-level 字段深度合并进 existing（同名冲突 common config 优先，复用 `json_deep_merge`）；
    3. existing 独有、cc-switch 未管理的字段（用户自加的 `hooks` / `enabledMcpjsonServers` 等）原样保留。
  - **改动文件**：`services/project.rs`（合并逻辑 + 文档注释 + 新测试）、`services/provider/live.rs`（`json_deep_merge` 由私有提为 `pub(crate)`）、`services/provider/mod.rs`（re-export `json_deep_merge`）。
  - **测试**：新增 `write_claude_to_project_merges_full_provider_and_common_config`（seed provider + common_config_claude snippet + 含旧 env/用户字段的 existing，断言非 env 字段写入 / env 整体替换清残留 / 深度合并 union / existing 独有保留）；顺手把 `write_claude_to_project_creates_settings_json_with_provider_env` 的断言改成 proxy 无关（原断言 `BASE_URL=="https://x.example"` 在 `enableLocalProxy:true` 下必挂——属环境相关**既有**问题，经 `git stash` 验证非本次回归）。
  - ⚠️ **已落盘的项目文件不会自动重写**：需在 App 里对该项目重新绑定/重写 provider（或调 `write_project_claude_settings` 命令）才会按新语义重新生成 `settings.local.json`。

### 低优先级
- **多 provider 路径聚合**：`/claude/project/{id}/v1/models`、`/claude/project/{id}/v1/chat/completions` 等其他路径暂未加（暂不需要）
- **provider 删除时项目引用悬挂**：service 层 `set_claude_provider` 用 `db.update_project_provider` 保留 id，但 provider 真删时项目引用仍指向不存在 id——`select_providers_for_project` 会 `NoProvidersConfigured` 错误（fail-fast，没优雅处理）
- **settings.rs `current_project_id` 字段**加了但未使用（留作未来）
- **ProviderCard 工程 chip 没有「不在窗口」提示**：多项目绑同一 provider 时只显示项目名，hover tooltip 才有路径
- **Mac build tag 冲突**：上游已用 `v3.16.4`，我用 `v3.16.4-projects` 区分

---

## 12. 下次继续开发的步骤

### A. 修剩余 5 处 usage 归因
1. 读 5 个暂传 None 的位置
2. 闭包内加 `let project_id = ctx.project_id.clone();`
3. `log_usage(project_id)` / `log_usage_internal(project_id)` 透传
4. dev 实测：项目目录跑流式请求 → 用量页「工程」列非空
5. 跑 `cargo test --lib` 全绿
6. 跑 `cargo clippy` 无新增 warning

### B. Mac build（解锁 billing 后）
1. 用户解锁 GitHub billing
2. `gh workflow run release.yml --repo kaohum/cc-switch`
3. 监控 `https://github.com/kaohum/cc-switch/actions`
4. 完成后 `https://github.com/kaohum/cc-switch/releases` 拿 .dmg

### C. 添加 Codex 项目级支持（MVP 扩展）
- Codex 通过 `CODEX_HOME` 环境变量隔离 config 目录
- 需在写项目根时设 `CODEX_HOME=<项目虚拟目录>` + 复制 provider 配置
- UI 增加「Codex provider」绑定字段

### D. 自动检测 cwd 切换项目
- Tauri 注册文件 watcher
- 监听 terminal cwd 变化 → 自动激活匹配项目
- 实现复杂，Windows/macOS/Linux 行为差异大

---

## 13. Git 操作备忘

```bash
# 本机工作流
cd /e/Projects/cc-switch
export PATH="$HOME/.cargo/bin:$PATH"

# 拉最新
git pull origin main

# build 本地 MSI（需 NO_PROXY=*）
unset HTTPS_PROXY HTTP_PROXY https_proxy http_proxy
export NO_PROXY='*'
export no_proxy='*'
pnpm tauri build --bundles msi

# 测试
cd src-tauri && cargo test --lib
cd /e/Projects/cc-switch && pnpm test:unit
pnpm typecheck

# 触发 CI
"/c/Program Files/GitHub CLI/gh.exe" workflow run release.yml --repo kaohum/cc-switch

# push tag 触发 release
git tag v3.16.4-projects
git push origin v3.16.4-projects
```

---

## 14. 关键术语映射

| 中文 | 英文 | 用途 |
|------|------|------|
| 项目工程目录 | Project Workspace | 用户工作目录 |
| 供应商/Provider | Provider | AI 服务商配置 |
| 多实例 | Multi-instance | 多项目多 provider 同时跑 |
| 合并模式 | Merge mode | 写 settings.local.json 时只覆盖 env 段 |
| 写时机 | Write timing | 选定 provider 立即触发 |
| 启动器 | Launcher | 打开终端并启动 claude |
| 归因 | Attribution | usage 记录到具体项目 |
| 直连 | Direct connection | 不经 proxy |
| 路由 | Routing | proxy 按 path/app 选 provider |

---

**写于 2026-06-29，会话名"cc-switch拓展"。**
