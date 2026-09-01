# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

CC Switch is a cross-platform desktop app (Tauri 2) that manages provider/MCP/Skills/Prompt configuration for seven AI CLI tools — Claude Code, Claude Desktop, Codex, Gemini CLI, OpenCode, OpenClaw, and Hermes — from a single GUI. Instead of editing JSON/TOML/.env by hand, users pick a preset and the app writes the right live config file for the target tool.

## Commands

The frontend (React/TS) and backend (Rust) are independent toolchains that meet at the Tauri IPC boundary. Run frontend tooling from the repo root, Rust tooling from `src-tauri/`.

```bash
pnpm install                  # install JS deps
pnpm dev                      # dev mode (hot reload, launches Tauri + Vite)
pnpm build                    # production build
pnpm tauri build --debug      # debug build of the native binary

pnpm typecheck                # tsc --noEmit (no separate build step for TS)
pnpm format                   # prettier --write
pnpm format:check             # prettier --check (CI gate)

pnpm test:unit                # vitest run (frontend)
pnpm test:unit:watch          # vitest watch
pnpm test:unit <pattern>      # run a single file/test by path or name substring
pnpm test:unit -- --coverage  # with coverage
```

```bash
# from src-tauri/
cargo fmt
cargo clippy
cargo test
cargo test <name>                       # single test by name substring
cargo test --features test-hooks        # tests guarded by the test-hooks feature
```

**PR gates** (all three must pass before submitting): `pnpm typecheck`, `pnpm format:check`, `pnpm test:unit`.

Toolchain versions: Rust MSRV is **1.85** (declared in `Cargo.toml`); CI builds on **stable** via `dtolnay/rust-toolchain@stable` — there is no `rust-toolchain.toml` pin. Tauri CLI 2.8+; Node 18+; pnpm 8+. There is a single root `package.json` (not a JS monorepo) — `pnpm-workspace.yaml` only carries pnpm config (`onlyBuiltDependencies`, `allowBuilds`); its `packages:` list is empty.

## Architecture

### Layered backend (Rust) — `src-tauri/src/`

```
commands/   Tauri IPC layer — one file per domain (provider.rs, mcp.rs, proxy.rs, usage.rs, …)
  → services/    Business logic (provider/, mcp.rs, proxy.rs, skill.rs, usage_stats.rs, …)
    → database/  SQLite via rusqlite
        ├── mod.rs      Database struct (SCHEMA_VERSION constant lives here)
        ├── schema.rs   table DDL + schema migrations
        ├── dao/        one DAO file per table (providers, mcp, prompts, skills, settings, …)
        ├── migration.rs  legacy config.json → SQLite migration
        └── backup.rs   snapshot/import/export
```

- **SSOT**: `~/.cc-switch/cc-switch.db` (SQLite) holds all syncable data. Device-local UI prefs live separately in the Tauri Store / `settings.json`.
- **Database connection** is a single `Mutex<Connection>` shared across threads (`store.rs::AppState.db: Arc<Database>`). Always acquire it through the `lock_conn!` macro — never `.lock().unwrap()`. DAO methods are `impl Database`.
- **Schema changes**: bump `SCHEMA_VERSION` in `database/mod.rs` **and** add a migration step in `schema.rs::apply_schema_migrations`. A pre-migration `.backup` is taken automatically when `0 < old_version < SCHEMA_VERSION`.
- **Tests**: use `Database::memory()` (in-memory DB with schema + seed pricing) for fast, isolated DAO tests.

### AppState & managed state

`AppState` (`store.rs`) holds `db`, `proxy_service`, and `usage_cache`, and is `app.manage()`d in `lib.rs::run()`. Additional long-lived services are managed separately and injected as `tauri::State`: `SkillServiceState`, `CopilotAuthState` (`RwLock<CopilotAuthManager>`), `CodexOAuthState`. When you add a service that needs to survive across commands, follow this pattern.

### Frontend (React 18 + TS) — `src/`

Data flow for any feature:

```
components/<feature>/   UI
  → hooks/              business logic, mutations, side effects (e.g. useProviderActions)
    → lib/api/          typed Tauri invoke() wrappers, one file per domain
      → Rust commands   (the giant invoke_handler! list in lib.rs)
```

- `lib/query/` configures TanStack Query; hooks wrap queries/mutations and own cache invalidation (e.g. `useAddProviderMutation`).
- `@/*` path alias → `src/*` (configured in both `tsconfig.json` and `vitest.config.ts`).
- `config/*ProviderPresets.ts` holds the built-in provider presets per tool; `i18n/locales/` holds zh/zh-TW/en/ja strings.

### Adding a new Tauri command (end-to-end checklist)

1. Write the `#[tauri::command]` fn in `commands/<domain>.rs` (re-exported via `commands/mod.rs`'s `pub use <domain>::*`).
2. **Register it** in the `tauri::generate_handler![…]` list in `lib.rs::run()` — unregistered commands silently fail to resolve from JS.
3. Add a typed wrapper in `src/lib/api/<domain>.ts` and export from `src/lib/api/index.ts`.
4. Wire a hook in `src/hooks/` and consume it from the component.
5. If the command is hit in frontend tests, add a handler in `tests/msw/tauriMocks.ts`.

**Naming convention**: command identifiers are `snake_case` (e.g. `add_provider`); Tauri maps JS `camelCase` arg keys to Rust `snake_case` params, so `invoke("add_provider", { addToLive })` arrives as `add_to_live`. Match this when naming.

### Live-config dual-way sync (core concept)

Each supported tool has its own live config file(s) on disk (e.g. `~/.claude/settings.json`, `~/.codex/auth.json`, `~/.gemini/settings.json`). CC Switch treats the DB as the source of truth and **writes the active provider to the live file on switch** (atomic temp-file + rename), and **backfills from the live file when the user edits the currently-active provider**. Path resolution for each tool lives in `src-tauri/src/config.rs` (`get_home_dir()` + per-tool `get_*_config_dir()`). `AppType` (`app_config.rs`) enumerates the seven tools; some are "additive mode" (OpenCode/OpenClaw) where import is idempotent and runs on every startup.

### Local proxy subsystem — `src-tauri/src/proxy/`

A local reverse proxy (axum + hyper) that can hot-switch providers, convert API formats, run auto-failover with a circuit breaker, and "take over" an app's live config (inject a placeholder pointing at the proxy, back up the original, restore on stop). On startup and exit the app detects takeover residue (`has_any_live_backup` / `detect_takeover_in_live_configs`) and restores live configs so CLI tools are never left in a broken state. If you touch proxy state, be aware of the crash-recovery paths in `lib.rs` (`restore_proxy_state_on_startup`, `cleanup_before_exit`).

### Deep links

`ccswitch://` URLs are parsed (`deeplink/parser.rs`) and broadcast to the frontend as `deeplink-import` / `deeplink-error` Tauri events; the frontend handles the rest. Entry points differ by platform (single-instance args on Win/Linux, `on_open_url` + macOS `RunEvent::Opened`).

## Conventions

- **每次有修改必须写 CHANGELOG.md（同一提交内）.** Fork changes go into the top `## [Unreleased] — Project Workspace & Proxy 项目级路由` block (Added / Changed / Fixed / Tech debt); upstream merges add a dated `同步上游` note there covering what was merged and how the schema/migration chain was renumbered. No behavior-affecting commit ships without its changelog entry — this includes merge follow-up fixes, schema version bumps, and release builds.
- **Comments are bilingual.** Much of the Rust codebase has Chinese comments — this is established project style; match the surrounding file when editing (don't strip or translate existing comments).
- **Atomic writes everywhere** for any file the user's CLI tools depend on: write to a temp path, then rename.
- **Don't delete the active provider / don't leave a tool with zero configs** — "minimal intrusion": even if uninstalled, the CLI tool must keep working.
- Frontend tests mock the Tauri bridge with **MSW** (`tests/msw/`); `tests/setupTests.ts` wires `server`, `tauriMocks`, and a minimal i18n instance. Component tests use `@testing-library/react` in `jsdom`.

## User data locations (runtime)

- DB: `~/.cc-switch/cc-switch.db` · Settings: `~/.cc-switch/settings.json` · Backups: `~/.cc-switch/backups/`
- Logs: `~/.cc-switch/logs/cc-switch.log` (overwritten on each launch) · Crash log: `~/.cc-switch/crash.log`
- `CC_SWITCH_TEST_HOME` env var overrides the home dir for test isolation (see `config.rs::get_home_dir`).

## build
see BUILD-WINDOWS.md

## Fork & upstream sync

This repo is a fork: `origin` = `kaohum/cc-switch` (this repo), `upstream` = `farion1231/cc-switch` (the source of truth to track and merge from).

- **Merge, don't rebase.** The fork carries a large local feature ("Project Workspace" — per-project Claude provider binding + proxy project-level routing) across 30+ commits. Sync with `git merge upstream/main`; history already uses merge commits and rebasing would rewrite all of it.
- **CHANGELOG.md always conflicts on upstream merge — this is expected, not a mistake.** Upstream cuts a new `## [x.y.z] - <date>` block at the top of CHANGELOG.md on each release; this fork keeps its own `## [Unreleased] — Project Workspace & Proxy 项目级路由` block at the top too, so both insert at the same point and Git can't auto-merge. Resolution: **keep both blocks** — the fork's `[Unreleased]` stays first (Keep a Changelog convention), then the new upstream `[x.y.z]` block, then the existing older versions. Drop nothing.
- **Fork release tags** use the `v<x.y.z>-projects` suffix (e.g. `v3.16.5-projects`) to distinguish from upstream's `v<x.y.z>`.
- **No CI release workflow.** `.github/workflows/release.yml` was removed (fork has no signing cert and GitHub Actions billing is locked); release builds are produced locally per `BUILD-WINDOWS.md`. The remaining `.github/workflows/{ci,claude,stale}.yml` belong to upstream and are intentionally kept (removing them would cause recurring conflicts on every upstream merge).
