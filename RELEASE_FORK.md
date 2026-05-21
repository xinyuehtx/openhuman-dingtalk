# Fork 发版指南 (xinyuehtx/openhuman-dingtalk)

本仓库是 [tinyhumansai/openhuman](https://github.com/tinyhumansai/openhuman) 的 fork，做了钉钉相关定制 + 自定义 LLM 改造。fork 不复用上游的 production / staging CI 流水线（依赖 Apple Developer ID、Tauri 私钥、Sentry、GHCR 等 fork 拿不到的资源），而是走轻量的本地手工发版。

## TL;DR

| 平台                        | 在哪台机器       | 跑什么                            |
| --------------------------- | ---------------- | --------------------------------- |
| macOS Apple Silicon (arm64) | macOS arm64 本机 | `./scripts/release-fork.sh`       |
| Windows x64                 | Windows x64 本机 | `pwsh ./scripts/release-fork.ps1` |

两条都把产物上传到 `xinyuehtx/openhuman-dingtalk` 上的同一个 release tag（默认 `v<version>`，version 取自 `app/package.json`）。

## 关键约定

- **Release 不签名**。macOS 用户首次打开需右键 → 打开（一次性 Gatekeeper 提示）；Windows 用户会看到 SmartScreen 提示，点"更多信息 → 仍要运行"。
- **关闭了自动更新**。`app/src-tauri/tauri.conf.json` 的 `plugins.updater.active = false`。装好的 app 不会自动检查 / 拉取更新，确保不会被上游 latest.json 拉走。
- **install 脚本指向 fork**。`scripts/install.sh` 与 `scripts/install.ps1` 中的 `REPO` 已改为 `xinyuehtx/openhuman-dingtalk`。

## 准备一次性环境

### macOS arm64 上（首次）

```bash
brew install gh jq
gh auth login                # 选 SSH 或 HTTPS，scope 含 repo
pnpm install                 # 安装 JS 依赖
pnpm tauri:ensure            # 确保 vendored CEF-aware tauri-cli 在 ~/.cargo/bin
```

### Windows x64 上（首次）

```powershell
winget install GitHub.cli
gh auth login
pnpm install
pnpm tauri:ensure
```

需要先装好 Visual Studio Build Tools（带 "Desktop development with C++"）和 Rust（`rustup`，与 `rust-toolchain.toml` 同步）。

## 发版步骤

### 1. 决定版本号

```bash
# 查看当前
jq -r .version app/package.json
# 例如 0.55.0
```

如果要 bump：手动改 `app/package.json` / `app/src-tauri/tauri.conf.json` / `app/src-tauri/Cargo.toml` / `Cargo.toml` 同步成新版本，commit 一次。也可以用上游脚本：`node scripts/release/bump-version.js patch`（要求 git working tree clean），但它不会推送 tag。

### 2. 在 macOS arm64 机器上构建并上传

```bash
./scripts/release-fork.sh                # 默认从 app/package.json 读 version
# 或者预演一次：
./scripts/release-fork.sh --dry-run
# 或者只上传现有 build 产物：
./scripts/release-fork.sh --skip-build --tag v0.55.0
```

脚本会：

1. 跑 `cargo tauri build --target aarch64-apple-darwin --bundles app dmg`
2. 找 `target/aarch64-apple-darwin/release/bundle/dmg/OpenHuman_<v>_aarch64.dmg`
3. 创建 / 复用 GitHub Release（**草稿**状态），上传 dmg + sha256 sidecar

### 3. 在 Windows x64 机器上构建并上传

```powershell
.\scripts\release-fork.ps1               # 默认 tag 与 mac 步骤一致
.\scripts\release-fork.ps1 -DryRun
.\scripts\release-fork.ps1 -SkipBuild -Tag v0.55.0
```

产物：`OpenHuman_<v>_x64_en-US.msi` 和 `OpenHuman_<v>_x64-setup.exe`，上传到同一 tag。

### 4. 把 release 从 draft 翻转为 published

```bash
gh release edit v<version> --repo xinyuehtx/openhuman-dingtalk --draft=false
```

或在 GitHub UI 上点 "Publish release"。一旦 published，`scripts/install.sh` 与 `install.ps1` 通过 `releases/latest` API 就能拿到。

## 安装脚本（用户视角）

发布之后，使用者按下面命令安装最新版：

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/xinyuehtx/openhuman-dingtalk/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://raw.githubusercontent.com/xinyuehtx/openhuman-dingtalk/main/scripts/install.ps1 | iex
```

install 脚本会先尝试 `latest.json`（本仓库不发，会 404 → 警告），然后 fallback 到 `releases/latest` API，根据 OS+ARCH 选 `aarch64.dmg` / `x64-setup.exe` / `x64*.msi` 中的正确产物下载安装。

> 注意：当前 fork 仅发布 macOS arm64 + Windows x64。如果一台机器是 macOS x86_64 / Linux x86_64，install 脚本会报"未找到对应平台的资产"。要扩展平台时在对应机器上跑同名脚本即可。

## 常见问题

### Q: 用户装上后会不会被自动更新拉到上游？

不会。tauri.conf.json 里 `plugins.updater.active = false`，app 启动时根本不检查更新。endpoint 也已指向 fork（双重保险）。

### Q: macOS Gatekeeper 报"无法验证开发者"

这是不签名的预期行为。让用户：

- 按住 Control 点 OpenHuman.app → 选 **打开** → 再次确认 → 之后正常启动；
- 或在 终端：`xattr -dr com.apple.quarantine /Applications/OpenHuman.app`。

### Q: 如果上游的资源（Sentry / Apple 证书 / TAURI_SIGNING_PRIVATE_KEY）以后准备好了

切回上游 production 流程：把 `tauri.conf.json` 的 `plugins.updater.active` 改回 `true`、endpoint 改回 fork（保持 fork 自己控制更新），在 fork 里配齐 secrets，启用 `release-production.yml`。届时再决定是否生成 latest.json + minisign 签名。

### Q: 我不想每次都手工 bump 版本

可以在 fork 上启用一份 `release-staging.yml` 的精简版（只构建不签名不 Sentry），但那已经超出"C 方案：不做 CI"的范围；如果要走那条路，单独提一次。
