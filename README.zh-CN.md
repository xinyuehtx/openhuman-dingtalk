<h1 align="center">OpenHuman 钉钉</h1>

<p align="center">
 🇺🇸 <a href="./README.md">English</a> | 🇨🇳 <a href="./README.zh-CN.md">简体中文</a>
</p>

[OpenHuman](https://github.com/tinyhumansai/openhuman) 的 fork，聚焦
**本地自部署 + 钉钉集成**。

## 本 fork 在上游基础上增加了什么

- **自定义 LLM，免登录上游。** 配置任意 OpenAI 兼容 endpoint（如内部 Qwen /
  DeepSeek 接口）的 `inference_url` + `api_key` 即可在本地直接对话，不需要登录
  OpenHuman 云端。该模式下 onboarding/welcome 流程会被跳过，进入聊天即落到
  orchestrator agent。
- **钉钉工具集。** 内置钉钉 AI 表格 / 日历 / 通讯录 / 群聊与机器人 / 审批 /
  文档 / 云盘 / 邮箱 / 在线电子表格 / 知识库等能力，作为原生 agent tool 暴露。
- **轻量 fork 发版流程。** 不依赖上游的 production CI（Apple 公证 / Sentry /
  GHCR / GitHub App 等都不需要）。本地手工发版：
  [`scripts/release-fork.sh`](./scripts/release-fork.sh)（macOS arm64）、
  [`scripts/release-fork.ps1`](./scripts/release-fork.ps1)（Windows x64），
  产物直传 fork 仓库的 GitHub Releases。

agent harness、Memory Tree、Tauri shell、channels、tools 等其余部分
完全继承上游 OpenHuman。

## 上游友链

本 fork 跟随 `tinyhumansai/openhuman`。完整架构文档、功能介绍、社区入口请见上游：

- 上游仓库：<https://github.com/tinyhumansai/openhuman>
- 官方文档：<https://tinyhumans.gitbook.io/openhuman/>
- Discord：<https://discord.tinyhumans.ai/>
- Reddit：<https://www.reddit.com/r/tinyhumansai/>
- 作者：[@senamakel](https://x.com/intent/follow?screen_name=senamakel)

## 安装

本 fork 的预编译包发布于
<https://github.com/xinyuehtx/openhuman-dingtalk/releases/latest>。

```bash
# macOS / Linux 一键安装
curl -fsSL https://raw.githubusercontent.com/xinyuehtx/openhuman-dingtalk/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://raw.githubusercontent.com/xinyuehtx/openhuman-dingtalk/main/scripts/install.ps1 | iex
```

> 安装包**未签名**。macOS 首次打开请右键 → **打开**，绕过 Gatekeeper；Windows
> 会触发 SmartScreen，点 *更多信息 → 仍要运行* 即可。

## 开发

环境要求：Node.js 24+、pnpm 10.10+、Rust 1.93.0（`rustfmt` + `clippy`）、
CMake、Ninja、ripgrep，以及[平台桌面构建依赖](https://tinyhumans.gitbook.io/openhuman/developing/getting-set-up)。

```bash
git clone git@github.com:xinyuehtx/openhuman-dingtalk.git
cd openhuman-dingtalk
git submodule update --init --recursive
pnpm install

pnpm dev:app          # 完整 Tauri 桌面开发（CEF 运行时）
pnpm dev              # 仅启动 Vite 前端
pnpm typecheck        # TypeScript 检查
pnpm lint             # ESLint
pnpm test             # Vitest 前端测试
pnpm test:rust        # cargo test（Rust 核心）
cargo check --bin openhuman-core
```

架构与编码约定见 [`CLAUDE.md`](./CLAUDE.md) 与 [`AGENTS.md`](./AGENTS.md)。
[`gitbooks/developing/`](./gitbooks/developing/) 下的整体架构和分领域文档
继续适用，未做改动。

## 发版（fork 内部）

本地手工流程，无 CI、不签名。完整步骤见
[`RELEASE_FORK.md`](./RELEASE_FORK.md)。

```bash
# macOS arm64 —— 产出未签名 .dmg 并上传到草稿 release
./scripts/release-fork.sh

# Windows x64 —— 在 Windows 机器上跑，上传 .msi + .exe 到同一 tag
pwsh ./scripts/release-fork.ps1
```

## License

[GNU GPL v3](./LICENSE)，继承自上游 OpenHuman。
