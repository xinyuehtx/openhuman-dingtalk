### dingtalk-dws-integration ###
移除 Skills 页面中现有的 Composio 集成网格，新增钉钉 DWS (DingTalk Workspace CLI) 工具集成，包括 Rust 后端工具实现和 React 前端 UI 展示。


## 背景

本项目是 OpenHuman 的钉钉定制 fork (`openhuman-dingtalk`)。当前 Skills 页面展示了 118+ 个 Composio OAuth 集成（Gmail、Notion、GitHub 等），这些对钉钉场景无用。需要：

1. 移除视图层所有 Composio 集成
2. 集成钉钉 DWS (DingTalk Workspace CLI) — 钉钉官方 CLI 工具，提供 19 个产品、209 条命令，覆盖 AI 表格/日历/通讯录/群聊/待办/审批/考勤等全套能力
3. 在 UI 层添加 DWS 集成的配置和管理界面

---

## 第一步：移除 Skills 页面的 Composio 集成网格

### 1.1 修改 `app/src/pages/Skills.tsx`

- 移除所有 Composio 相关的 import：`ComposioConnectModal`、`composioToolkitMeta`、`KNOWN_COMPOSIO_TOOLKITS`、`useComposioIntegrations`、`canonicalizeComposioToolkitSlug`、`deriveComposioState` 等
- 移除 `ComposioConnectorTile` 组件
- 移除 `composioStatusLabel`、`composioStatusColor`、`composioSortRank` 等辅助函数
- 移除所有 Composio 相关的 state（`composioModalToolkit`、`composioToolkits`、`composioConnectionByToolkit`、`composioError` 等）
- 移除 `composioCatalogToolkits`、`composioGridEntries`、`composioFilteredEntries`、`composioSortedEntries` 等 memo
- 移除 JSX 中的 Composio 集成网格区域（包含 `{t('skills.integrations')}` 标题、搜索栏、分类过滤器和图标网格的整个 `<div>`）
- 移除 `ComposioConnectModal` 的渲染
- 保留 Channels 网格、Built-in Skills、Discovered Skills 等其他功能

### 1.2 简化 `SkillCategoryFilter` 相关逻辑

- `availableCategories` 的计算中移除 Composio 相关的分类收集逻辑
- 移除 `SkillSearchBar` 和 `SkillCategoryFilter` 组件（它们主要用于 Composio 集成的搜索和过滤），或者保留给其他用途

---

## 第二步：新增钉钉 DWS 工具定义

### 2.1 在 `app/src/utils/toolDefinitions.ts` 中添加 DWS 工具

新增一个 `DingTalk` 工具分类，并添加 dws 工具定义：

```typescript
// 新增分类
export type ToolCategory = 'System' | 'Files' | 'Vision' | 'Web' | 'Memory' | 'Automation' | 'DingTalk';

// 在 TOOL_CATEGORIES 数组中添加
'DingTalk'

// 在 TOOL_CATALOG 中添加 dws 工具条目
{
  id: 'dws',
  displayName: '钉钉 DWS',
  description: '通过 DingTalk Workspace CLI 管理钉钉产品能力：AI表格、日历、通讯录、群聊、待办、审批、考勤、文档、云盘等。',
  category: 'DingTalk',
  defaultEnabled: true,
  rustToolNames: ['dws'],
}

// 在 CATEGORY_DESCRIPTIONS 中添加
DingTalk: '钉钉工作台集成 (DWS CLI)',
```

### 2.2 在 Rust 后端添加 DWS 工具实现

在 `src/openhuman/tools/impl/` 下新建 `dws/` 模块：

**`src/openhuman/tools/impl/dws/mod.rs`**:
- 定义 `DwsTool` 结构体，实现 `Tool` trait
- 工具名为 `dws`
- 功能：通过 `tokio::process::Command` 调用本地安装的 `dws` CLI 二进制
- 参数：接受 `command`（完整的 dws 子命令字符串）
- 自动追加 `--format json` 确保结构化输出
- 安全校验：验证命令前缀必须是 `dws`，防止命令注入

**核心实现逻辑（参考 dws SKILL.md 的指导）**：
```rust
pub struct DwsTool;

#[async_trait]
impl Tool for DwsTool {
    fn name(&self) -> &str { "dws" }
    fn description(&self) -> &str {
        "Execute DingTalk Workspace CLI (dws) commands to manage DingTalk services: ..."
    }
    // 通过 shell 执行 dws 命令并返回结果
    async fn call(&self, params: Value) -> ToolResult { ... }
}
```

### 2.3 注册 DWS 工具

在 `src/openhuman/tools/ops.rs` 或对应的工具注册位置，将 `DwsTool` 加入工具注册表。

---

## 第三步：在 Skills 页面添加钉钉 DWS 集成卡片

### 3.1 新建 DWS 集成区域组件

在 `app/src/pages/Skills.tsx` 中，在原 Composio 网格的位置，添加一个 DWS 集成卡片区域：

- 使用类似 Channel 卡片的风格，显示钉钉 DWS 工具
- 展示 DWS 的状态（是否已安装、是否已登录）
- 提供安装引导和登录入口

### 3.2 DWS 状态检测 Hook

新建 `app/src/features/dws/useDwsStatus.ts`：
- 通过 core RPC 调用 shell 执行 `dws --version` 检测是否已安装
- 执行 `dws auth status --format json` 检测登录状态
- 返回状态：`not_installed` | `not_authenticated` | `authenticated`

### 3.3 DWS 设置面板

新建 `app/src/components/dws/DwsSetupCard.tsx`：
- 展示 DWS 安装状态和认证状态
- 提供安装指引（链接到 dws 的安装命令）
- 提供登录指引（引导执行 `dws auth login`）
- 展示 DWS 支持的产品列表概览（19 个产品）

### 3.4 在 Skills 页面集成

在 `app/src/pages/Skills.tsx` 中 Channels 网格之后、其他分组之前，插入 DWS 集成区域：

```tsx
{/* DingTalk DWS Integration */}
<div className="rounded-2xl border ...">
  <h2>钉钉工作台 (DWS)</h2>
  <p>通过 DingTalk Workspace CLI 管理钉钉全产品能力</p>
  <DwsSetupCard />
</div>
```

---

## 第四步：在 ToolsPanel 中展示 DWS 工具开关

由于第二步已在 `toolDefinitions.ts` 中添加了 DWS 工具定义，`ToolsPanel`（`app/src/components/settings/panels/ToolsPanel.tsx`）将自动渲染 DingTalk 分类下的 `dws` 开关，无需额外修改。

---

## 涉及文件清单

| 文件 | 操作 |
|------|------|
| `app/src/pages/Skills.tsx` | 修改：移除 Composio 集成网格，新增 DWS 集成区域 |
| `app/src/utils/toolDefinitions.ts` | 修改：添加 DingTalk 分类和 dws 工具定义 |
| `src/openhuman/tools/impl/dws/mod.rs` | 新建：DWS CLI 工具实现 |
| `src/openhuman/tools/impl/mod.rs` | 修改：注册 dws 模块 |
| `src/openhuman/tools/ops.rs` | 修改：注册 DwsTool |
| `app/src/features/dws/useDwsStatus.ts` | 新建：DWS 状态检测 Hook |
| `app/src/components/dws/DwsSetupCard.tsx` | 新建：DWS 设置卡片组件 |


updateAtTime: 2026/5/20 16:27:15

planId: 0ed3ba39-f964-4a04-a96b-8e452df7855c