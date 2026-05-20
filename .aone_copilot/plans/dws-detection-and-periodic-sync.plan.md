### dws-detection-and-periodic-sync ###
修复 DWS CLI 本地检测问题，并增加定时拉取钉钉数据功能（含 UI「立即拉取」按钮、内容类型选择开关、配置持久化）。


## 实施计划

### 问题 1：修复 DWS 本地检测

#### 根因

`useDwsStatus.ts` 通过 core RPC 的 `shell` 工具执行 `which dws` 检测。`ShellTool`（`src/openhuman/tools/impl/system/shell.rs`）只将 `SAFE_ENV_VARS`（含 `PATH`）传递给子进程，但 Rust core 进程启动时继承的 `PATH` 通常不包含用户 shell profile（`.zshrc` / `.bashrc`）中追加的自定义路径（如 `~/.qoderwork/bin`）。

用户机器上 dws 在 `/Users/huangtengxiao/.qoderwork/bin/dws`，该路径虽然在用户终端 PATH 中，但通过 Tauri 或 `cargo run` 启动 core 时不一定继承完整用户 PATH。

#### 修复方案

**方案 A（推荐）：在 `useDwsStatus.ts` 的检测命令中增加常见安装路径的 fallback**

修改 `app/src/features/dws/useDwsStatus.ts` 中的 `checkStatus` 函数，将 `which dws` 改为更健壮的多路径探测：

```typescript
// 同时检查 which + 常见安装路径
const whichResult = await executeShellCommand(
  'which dws 2>/dev/null || command -v dws 2>/dev/null || ' +
  '([ -x "$HOME/.qoderwork/bin/dws" ] && echo "$HOME/.qoderwork/bin/dws") || ' +
  '([ -x "/usr/local/bin/dws" ] && echo "/usr/local/bin/dws") || ' +
  '([ -x "$HOME/.local/bin/dws" ] && echo "$HOME/.local/bin/dws")'
);
```

**方案 B（补充）：在 `DwsTool` 和 `ShellTool` 执行前扩展 PATH**

修改 `src/openhuman/tools/impl/dws/mod.rs`，在执行 `dws` 命令前，将已知的常见安装目录追加到 PATH 中：

```rust
// 在执行命令前，构建扩展后的 PATH
let home = std::env::var("HOME").unwrap_or_default();
let extra_paths = [
    format!("{}/.qoderwork/bin", home),
    format!("{}/.local/bin", home),
    "/usr/local/bin".to_string(),
];
let current_path = std::env::var("PATH").unwrap_or_default();
let extended_path = format!("{}:{}", extra_paths.join(":"), current_path);
```

然后在 `tokio::process::Command` 构建时通过 `.env("PATH", &extended_path)` 注入。

#### 涉及文件

- `app/src/features/dws/useDwsStatus.ts` — 前端检测逻辑
- `src/openhuman/tools/impl/dws/mod.rs` — DwsTool 的 PATH 扩展

---

### 问题 2：定时拉取钉钉内容

#### 2.1 Rust 后端：DWS Sync 模块

新建 `src/openhuman/tools/impl/dws/sync.rs`，参考 `src/openhuman/composio/periodic.rs` 的模式实现：

```rust
// src/openhuman/tools/impl/dws/sync.rs
// 定时拉取调度器 + 手动触发逻辑

pub struct DwsSyncScheduler { ... }

/// 可拉取的 DingTalk 数据类型
pub enum DwsSyncCategory {
    Calendar,   // 日历事件
    Todo,       // 待办
    Contact,    // 通讯录
    Attendance, // 考勤
    Approval,   // 审批
    Report,     // 日志
    Mail,       // 邮箱
    Doc,        // 文档
    Chat,       // 群消息
}
```

核心功能：
- `start_periodic_sync()` — 启动定时任务（`OnceLock` 防重入）
- `sync_now(categories: Vec<DwsSyncCategory>)` — 立即拉取指定类别
- 每个类别对应一个 `dws` CLI 命令（如 `dws calendar event list --format json`）
- 拉取的数据存入 memory（通过现有的 memory 模块）

#### 2.2 Rust 后端：DWS Sync 配置

在 `src/openhuman/config/schema/` 下新增 DWS 同步配置，或扩展现有的 `tools` 配置：

新建 `src/openhuman/config/schema/dws.rs`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DwsSyncConfig {
    /// 是否启用定时同步
    pub enabled: bool,
    /// 同步间隔（分钟），默认 30
    pub interval_minutes: u32,
    /// 启用的同步类别
    pub categories: DwsSyncCategories,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DwsSyncCategories {
    pub calendar: bool,
    pub todo: bool,
    pub contact: bool,
    pub attendance: bool,
    pub approval: bool,
    pub report: bool,
    pub mail: bool,
    pub doc: bool,
    pub chat: bool,
}
```

将其接入 `Config` 顶层（`src/openhuman/config/schema/types.rs`）：

```rust
pub struct Config {
    // ...existing fields...
    #[serde(default)]
    pub dws_sync: DwsSyncConfig,
}
```

#### 2.3 Rust 后端：RPC 接口

新建 `src/openhuman/tools/impl/dws/rpc.rs` 或在现有 schemas 中注册：

- `openhuman.dws_sync_now` — 立即拉取（接收 categories 列表参数）
- `openhuman.dws_sync_config_get` — 获取同步配置
- `openhuman.dws_sync_config_update` — 更新同步配置（持久化到 config.toml）

#### 2.4 前端 UI

**修改 `app/src/components/dws/DwsSetupCard.tsx`**，在"已连接"状态下增加：

1. **「立即拉取」按钮** — 调用 `openhuman.dws_sync_now` RPC
2. **同步类别 Switcher 列表** — 基于 `DWS_PRODUCTS` 常量，每项对应一个 toggle 开关
3. **同步状态指示** — 上次同步时间、同步中动画

**新建 `app/src/features/dws/useDwsSyncConfig.ts`**：

```typescript
export interface DwsSyncConfig {
  enabled: boolean;
  interval_minutes: number;
  categories: Record<string, boolean>;
}

export function useDwsSyncConfig() {
  // 调用 openhuman.dws_sync_config_get 获取当前配置
  // 调用 openhuman.dws_sync_config_update 更新配置
  // 返回 config, updateConfig, syncNow, syncing 等状态
}
```

#### 2.5 配置持久化

配置存储在 Rust core 的 `config.toml` 中（`[dws_sync]` section），通过现有的 `config.save()` 机制持久化。前端通过 RPC 读写配置，不在 localStorage 中额外存储——这是 Rust core 的配置系统已有的标准做法。

---

### 文件变更总览

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/openhuman/tools/impl/dws/mod.rs` | 修改 | 扩展 PATH，增加 `mod sync; mod rpc;` |
| `src/openhuman/tools/impl/dws/sync.rs` | 新建 | 定时拉取调度器 |
| `src/openhuman/tools/impl/dws/rpc.rs` | 新建 | RPC handlers (sync_now / config_get / config_update) |
| `src/openhuman/config/schema/dws.rs` | 新建 | DwsSyncConfig 配置结构 |
| `src/openhuman/config/schema/mod.rs` | 修改 | 增加 `mod dws; pub use dws::*;` |
| `src/openhuman/config/schema/types.rs` | 修改 | Config 结构加 `dws_sync` 字段 |
| `app/src/features/dws/useDwsStatus.ts` | 修改 | 修复检测命令，增加多路径探测 |
| `app/src/features/dws/useDwsSyncConfig.ts` | 新建 | 同步配置管理 hook |
| `app/src/components/dws/DwsSetupCard.tsx` | 修改 | 增加同步 UI（立即拉取按钮 + 类别开关） |

### 执行顺序

1. 修复 DWS 检测（前端 + Rust 双管齐下）
2. 新建配置 schema（Rust `dws.rs`，接入 Config）
3. 实现同步调度器（`sync.rs`）
4. 注册 RPC 接口（`rpc.rs`）
5. 前端 hook + UI 改造
6. 测试验证（`cargo check` + 前端 lint/typecheck）


updateAtTime: 2026/5/20 17:31:53

planId: 4bb7aaef-a0cd-42ca-bb0d-87888e544745