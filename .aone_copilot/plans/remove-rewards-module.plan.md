### remove-rewards-module

仅移除「奖励」模块的视图层：路由、页面组件、底部导航入口和 Home 页面入口按钮，保留 API 服务、类型定义、i18n、mock、文档等不动。

## 目标

最小化改动范围，仅移除用户可见的奖励模块入口和页面渲染，保留底层服务、类型、翻译等不动，降低改动风险。

---

### 1. 删除文件

**页面**

- `app/src/pages/Rewards.tsx`
- `app/src/pages/__tests__/Rewards.test.tsx`

**组件目录**（整个目录）

- `app/src/components/rewards/`

---

### 2. 修改文件

**`app/src/AppRoutes.tsx`**

- 删除 `import Rewards from './pages/Rewards';`（第14行）
- 删除 `/rewards` 路由块（约第118-124行）

**`app/src/components/BottomTabBar.tsx`**

- 删除 `id: 'rewards'` 的整个 tab 配置项（约第93-108行）

**`app/src/pages/Home.tsx`**

- 删除 "Earn rewards" 按钮块（`navigate('/rewards')` 对应的 `<button>` 元素，约第298-320行）

---

### 3. 不在本次改动范围内（保留）

- `app/src/services/api/rewardsApi.ts` 及其测试
- `app/src/types/rewards.ts`
- `app/src/store/__tests__/rewardsSlice.test.ts`
- 所有 i18n 翻译文件（`rewards.*` key 保留）
- `scripts/mock-api/routes/user.mjs`（mock 路由保留）
- E2E 测试文件（`rewards-unlock-flow.spec.ts`、`rewards-progression-persistence.spec.ts`）
- `app/test/e2e/specs/navigation.spec.ts`
- `AGENTS.md` / `CLAUDE.md` / `docs/TEST-COVERAGE-MATRIX.md`
- `app/src/components/settings/SettingsHome.tsx`（"Billing & Rewards" 文案保留）
- Rust 后端相关代码

---

### 4. 验证

- `pnpm typecheck` 确保无 TS 类型错误
- `pnpm lint` 确保无 lint 错误

updateAtTime: 2026/5/20 20:55:09

planId: de5e3dcf-cfb1-4116-b84d-4467e377b389
