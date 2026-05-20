<h1 align="center">OpenHuman 钉钉</h1>

<p align="center">
 🇺🇸 <a href="./README.md">English</a> | 🇨🇳 <a href="./README.zh-CN.md">简体中文</a>
</p>

A fork of [OpenHuman](https://github.com/tinyhumansai/openhuman) focused on
**local self-host + DingTalk (钉钉) integration**.

## What this fork adds

- **Custom LLM, no OpenHuman backend required.** Configure any
  OpenAI-compatible `inference_url` + `api_key` (e.g. an internal Qwen / DeepSeek
  endpoint) and chat works locally without signing in to the upstream OpenHuman
  cloud. The welcome / onboarding flow is bypassed in this mode so you go
  straight to the orchestrator agent.
- **DingTalk (钉钉) tool surface.** Built-in integrations for DingTalk's
  AI-table / calendar / contacts / group-chat / approvals / docs / drive / mail
  / sheets / wiki APIs, callable as native agent tools.
- **Lightweight fork release pipeline.** No upstream production CI dependencies
  (Apple notarize, Sentry, GHCR, GitHub App). Local manual builds via
  [`scripts/release-fork.sh`](./scripts/release-fork.sh) (macOS arm64) and
  [`scripts/release-fork.ps1`](./scripts/release-fork.ps1) (Windows x64),
  uploaded straight to this fork's GitHub Releases.

Everything else — agent harness, memory tree, Tauri shell, channels, tools —
inherits from upstream OpenHuman as-is.

## Upstream links

This fork tracks `tinyhumansai/openhuman`. For deep architecture, full feature
docs, and community, see:

- Upstream repo: <https://github.com/tinyhumansai/openhuman>
- Official docs: <https://tinyhumans.gitbook.io/openhuman/>
- Discord: <https://discord.tinyhumans.ai/>
- Reddit: <https://www.reddit.com/r/tinyhumansai/>
- Creator: [@senamakel](https://x.com/intent/follow?screen_name=senamakel)

## Install

Pre-built bundles for this fork are published at
<https://github.com/xinyuehtx/openhuman-dingtalk/releases/latest>.

```bash
# macOS / Linux (one-shot installer)
curl -fsSL https://raw.githubusercontent.com/xinyuehtx/openhuman-dingtalk/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://raw.githubusercontent.com/xinyuehtx/openhuman-dingtalk/main/scripts/install.ps1 | iex
```

> The bundles are **unsigned**. macOS users will need to right-click the app on
> first launch and choose **Open** to bypass Gatekeeper. Windows users will see
> SmartScreen — click *More info → Run anyway*.

## Develop

Prereqs: Node.js 24+, pnpm 10.10+, Rust 1.93.0 (`rustfmt` + `clippy`), CMake,
Ninja, ripgrep, and the [platform desktop build prerequisites](https://tinyhumans.gitbook.io/openhuman/developing/getting-set-up).

```bash
git clone git@github.com:xinyuehtx/openhuman-dingtalk.git
cd openhuman-dingtalk
git submodule update --init --recursive
pnpm install

pnpm dev:app          # full Tauri desktop dev (CEF runtime)
pnpm dev              # Vite dev server only (web UI)
pnpm typecheck        # TypeScript check
pnpm lint             # ESLint
pnpm test             # Vitest (frontend)
pnpm test:rust        # cargo test (core)
cargo check --bin openhuman-core
```

Architecture and conventions: [`CLAUDE.md`](./CLAUDE.md) and
[`AGENTS.md`](./AGENTS.md). The narrative architecture and per-domain docs
under [`gitbooks/developing/`](./gitbooks/developing/) all still apply
unchanged.

## Release (fork-internal)

Local manual flow — no CI, no signing. See
[`RELEASE_FORK.md`](./RELEASE_FORK.md) for the full runbook.

```bash
# macOS arm64 — produces an unsigned .dmg, uploads to a draft release
./scripts/release-fork.sh

# Windows x64 — runs on a Windows host, uploads .msi + .exe to the same tag
pwsh ./scripts/release-fork.ps1
```

## License

[GNU GPL v3](./LICENSE), inherited from upstream OpenHuman.
