### add-llm-settings-entry-on-home ###
在首页增加一个 LLM 配置修改入口，让用户在自定义 LLM 模式下可以方便地修改 Base URL、API Key 和 Model 配置。


## 背景

当前用户在首次配置自定义 LLM 后（通过 Welcome 页面的 `LlmSetup` 组件），首页没有任何修改入口。用户无法便捷地更改已保存的 LLM 配置（Base URL / API Key / Model）。

## 实现方案

在首页 Home 组件中，当检测到自定义 LLM 模式时，显示一个配置摘要卡片 + 修改按钮。点击后展开内联编辑表单（复用 `LlmSetup` 的逻辑），或导航到独立的修改页面。

推荐方案：**内联展示当前配置摘要 + 点击展开编辑**，体验更流畅。

---

### Step 1: 将 `LlmSetup` 组件改为支持"编辑模式"

**文件**: `app/src/pages/LlmSetup.tsx`

- 新增可选 prop `mode?: 'setup' | 'edit'`，默认 `'setup'`
- 编辑模式下：
  - 标题改为 "LLM Configuration"
  - 按钮文字改为 "Save"
  - 不显示 Logo（节省空间）
  - 支持外层控制是否显示外层容器样式（通过 `compact?: boolean` prop）

```tsx
interface LlmSetupProps {
  onComplete: (settings: LlmSettings) => void;
  mode?: 'setup' | 'edit';
  compact?: boolean;
}
```

### Step 2: 在首页添加 LLM 配置摘要卡片

**文件**: `app/src/pages/Home.tsx`

在主卡片下方、banner 区域上方，当 `isCustomLlmMode === true` 时，渲染一个配置摘要卡片：

- 显示当前 Model 名称和 Base URL（部分遮蔽）
- 右侧显示一个"Edit"图标按钮或展开箭头
- 点击后展开内联的 `LlmSetup` 编辑表单，或导航到 `/settings/llm`

方案 A（推荐 - 内联展开）：
```tsx
{isCustomLlmMode && (
  <LlmConfigCard
    onEdit={() => setShowLlmEditor(true)}
  />
)}
{showLlmEditor && (
  <LlmSetup mode="edit" compact onComplete={handleLlmEditComplete} />
)}
```

方案 B（导航到独立页面）：
- 新增路由 `/settings/llm` 指向 LlmSetup 的编辑模式

**推荐方案 A**，因为更简洁、不需要新路由。

### Step 3: 创建 `LlmConfigCard` 组件

**文件**: `app/src/components/home/LlmConfigCard.tsx`（新建）

- 从 `getStoredLlmSettings()` 读取当前配置
- 显示格式：
  - Model: `qwen-max` 
  - Endpoint: `https://dashscope...` (truncated)
  - API Key: `sk-****` (masked)
- 右侧有一个编辑（铅笔）图标按钮

```tsx
interface LlmConfigCardProps {
  onEdit: () => void;
}
```

### Step 4: 整合到 Home 页面

**文件**: `app/src/pages/Home.tsx`

- import `LlmConfigCard` 和修改后的 `LlmSetup`
- 新增 state: `const [showLlmEditor, setShowLlmEditor] = useState(false)`
- 在 CTA 按钮下方、banner 上方添加：
  - 默认显示 `LlmConfigCard`（摘要）
  - 点击 Edit 后切换为 `LlmSetup`（编辑模式）
  - 保存成功后折叠回摘要
- `handleLlmEditComplete` 回调：关闭编辑器、不导航

### Step 5: 国际化文案

**文件**: 相关 i18n JSON 文件

添加：
- `home.llmConfig.title`: "LLM Configuration"
- `home.llmConfig.edit`: "Edit"
- `home.llmConfig.model`: "Model"
- `home.llmConfig.endpoint`: "Endpoint"
- `home.llmConfig.saved`: "Settings saved"

---

## 任务清单

- [ ] 1. 修改 `LlmSetup` 组件，支持 `mode` 和 `compact` props
- [ ] 2. 创建 `LlmConfigCard` 组件展示当前配置摘要
- [ ] 3. 在 `Home.tsx` 中集成配置摘要卡片和内联编辑功能
- [ ] 4. 添加国际化文案
- [ ] 5. 验证功能：确保编辑后 RPC 同步到 Rust core


updateAtTime: 2026/5/22 11:15:12

planId: 13b1cf1e-2841-429b-8edc-a1d4badcf4a3