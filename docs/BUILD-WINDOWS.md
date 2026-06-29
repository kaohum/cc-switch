# Windows 构建手册（CC-Switch Fork）

> 提炼自 2026-06-29 实战踩坑后的最终可用方法。出 `CC Switch_x64.msi` 安装包。
>
> 适用：Windows 10/11，从源码 build Tauri 应用出 MSI 安装包。

---

## 一、环境要求

| 工具 | 版本要求 | 验证命令 |
|------|---------|---------|
| **Node.js** | 18+（推荐 20+） | `node --version` |
| **pnpm** | 8+（通过 corepack 启用） | `pnpm --version` |
| **Rust** | 1.85+（项目用 1.95） | `cargo --version` |
| **MSVC C++ Build Tools** | VS 2019/2022 + "Desktop development with C++" workload | cl.exe 在 PATH |
| **WebView2 Runtime** | Windows 11 自带；Win10 需装 | — |

### 安装 pnpm（如果只有 node）
```powershell
corepack enable pnpm   # Node 16.9+ 自带 corepack
pnpm --version         # 应输出 8+
```

### 安装 Rust
- 下载 https://win.rustup.rs/x86_64 → `rustup-init.exe`
- 装 stable：`rustup-init.exe -y --default-host x86_64-pc-windows-msvc --profile minimal`
- 项目有 `rust-toolchain.toml`，进项目目录后 rustup 自动装 `1.95.0`

---

## 二、三个关键坑（踩过必看）

### 坑 1：`protocol: http response missing version`

**现象**：`tauri build` 编译成功，但 bundle 阶段下载 WiX 时报 `failed to bundle project: protocol: http response missing version`。

**根因**：tauri 用 reqwest 下载 WiX 工具。reqwest **走系统代理**，代理返回的 HTTP 响应非法。

**修复**：bundle 时设 `NO_PROXY=*` 让 reqwest 直连（GitHub 直连是通的）。

### 坑 2：WiX 工具缓存路径

**现象**：即使手动放了 WiX 到 `~/.tauri/WixTools/`，tauri 仍然下载。

**根因**：tauri 实际用 `%APPDATA%\tauri\WixTools\`（即 `C:\Users\<你>\AppData\Roaming\tauri\WixTools\`），不是 `~/.tauri/`。

**修复**：见下文「手动准备 WiX」（如果不想每次联网）。

### 坑 3：`createUpdaterArtifacts` 拖累 bundle

**现象**：bundle 报 `__TAURI_BUNDLE_TYPE variable not found in binary` warning + 后续步骤连锁失败。

**根因**：fork 无代码签名证书，updater artifacts 没用还添乱。

**修复**：`src-tauri/tauri.conf.json` 改 `"createUpdaterArtifacts": false`（已在 fork 改）。

---

## 三、一键 build 流程（每次发布走这个）

```powershell
# 1. 进项目目录
cd E:\Projects\cc-switch

# 2. 确保 cargo 在 PATH（PowerShell）
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

# 3. 关代理（关键！reqwest 直连 GitHub）
$env:NO_PROXY = '*'
$env:no_proxy = '*'
Remove-Item Env:HTTPS_PROXY -ErrorAction SilentlyContinue
Remove-Item Env:HTTP_PROXY -ErrorAction SilentlyContinue

# 4. 装依赖（首次或 lockfile 变更后）
pnpm install

# 5. 出 MSI（首次约 10-12 分钟，增量后约 3-5 分钟）
pnpm tauri build --bundles msi
```

**输出位置**：
```
src-tauri\target\release\bundle\msi\CC Switch_3.16.4_x64_en-US.msi
```

**复制到桌面方便分发**：
```powershell
Copy-Item "src-tauri\target\release\bundle\msi\*.msi" "$env:USERPROFILE\Desktop\"
```

### bash / git-bash 版本

```bash
cd /e/Projects/cc-switch
export PATH="$HOME/.cargo/bin:$PATH"
export NO_PROXY='*'
export no_proxy='*'
unset HTTPS_PROXY HTTP_PROXY https_proxy http_proxy
pnpm tauri build --bundles msi
```

> ⚠️ **不要用 `.bat` 跑 pnpm**：corepack 装的 pnpm 是 `.ps1`，cmd 跑不了。必须用 PowerShell 或 bash。

---

## 四、手动准备 WiX（首次 / 离线环境）

如果 `pnpm tauri build --bundles msi` 在 `Downloading wix314-binaries.zip` 卡住或失败，手动下：

```powershell
# 1. 下载（走代理或直连，看哪个通）
$url = "https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip"
$dest = "$env:APPDATA\tauri\WixTools"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Invoke-WebRequest -Uri $url -OutFile "$env:TEMP\wix314.zip"
Expand-Archive -Path "$env:TEMP\wix314.zip" -DestinationPath $dest -Force

# 2. 验证 candle.exe + light.exe 在位
Test-Path "$dest\candle.exe"   # True
Test-Path "$dest\light.exe"    # True
```

之后 `tauri build` 会用本地 WiX，不再下载。

---

## 五、出 NSIS 版（备选，如果 MSI 还是失败）

NSIS 是另一种 Windows 安装器格式：

```powershell
$env:NO_PROXY = '*'
pnpm tauri build --bundles nsis
# 输出：src-tauri\target\release\bundle\nsis\CC Switch_3.16.4_x64-setup.exe
```

NSIS 也需要下载工具（nsis-cli），同样需要 `NO_PROXY=*`。

---

## 六、Portable exe（最简，不需要打包工具）

如果 WiX/NSIS 都搞不定，cc-switch.exe 本身是 portable（前端内嵌）：

```
src-tauri\target\release\cc-switch.exe   # 直接复制到任何目录运行
```

`pnpm tauri build --bundles msi` 即使 bundle 失败，**exe 也会先生成**（编译阶段就出来了）。直接用 exe 也能跑，只是没有安装器（用户手动放快捷方式）。

---

## 七、故障排查

| 症状 | 原因 | 修复 |
|------|------|------|
| `cargo metadata ... program not found` | cargo 不在 PATH | `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"` |
| `protocol: http response missing version` | reqwest 走系统代理 | `$env:NO_PROXY = '*'`（坑 1） |
| `duplicate column name: project_id` | migrate 重复加列 | 用 `add_column_if_missing`（已在代码修） |
| `__TAURI_BUNDLE_TYPE variable not found` | updater artifacts 配置 | `tauri.conf.json` 设 `createUpdaterArtifacts: false` |
| `Downloading wix314-binaries.zip` 卡住 | 网络代理拦截 | 手动下 WiX（第四节）|
| SmartScreen 警告 | fork 无代码签名 | 点「更多信息」→「仍要运行」（正常） |
| `error: linker 'link.exe' not found` | MSVC 未装/不在 PATH | 装 VS 2019/2022 + Desktop C++ workload |

---

## 八、验证安装包

build 完成后，在**干净环境**（卸载旧版 CC Switch）测试：

1. 双击 `.msi` 安装
2. SmartScreen → 「更多信息」→「仍要运行」
3. 启动 CC Switch
4. 顶部工具栏应有 📁 **项目** 图标（新功能入口）
5. 现有数据（`~/.cc-switch/cc-switch.db`）保留，schema 自动迁移 v11→v13

---

## 九、CI 自动 build（GitHub Actions）

本仓库 `.github/workflows/release.yml` 配置了 3 平台 CI（ubuntu/windows/macos）。

```bash
# push tag 触发（自动出 Win+Mac+Linux）
git tag v3.16.4-projects
git push origin v3.16.4-projects

# 或手动触发
"/c/Program Files/GitHub CLI/gh.exe" workflow run release.yml --repo kaohum/cc-switch
```

> ⚠️ **CI 当前阻塞**：GitHub 账号 billing 锁定（"account locked due to a billing issue"）。
> 解决：去 https://github.com/settings/billing 处理（更新卡 / 降 Free plan）。
> 解决后 CI 自动恢复，无需改代码。

---

**最后更新**：2026-06-29
**维护者**：陈昊然（基于 farion1231/cc-switch fork）
