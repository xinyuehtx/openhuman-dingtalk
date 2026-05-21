### memory-tree-chinese-obsidian ###
让 memory tree 和 wiki 的生成内容对 Obsidian 更友好：Summary 使用中文记录、文件名在 Summary 层使用中文、tag 语义化中文化，并按标题/热门实体/时间分别构建 content/topic/global 树。


## 背景

当前 memory tree 生成的所有 Summary 内容、文件名、tag 都是英文的，不利于在 Obsidian 中进行中文知识管理。用户需求：

1. Summary 必须使用**中文**进行抽象记录
2. 文件名在 chunk 层可以是 `类型+id`，但 **Summary 层必须使用中文**
3. Tag 必须使用有意义的**中文**标签
4. Content 树优先使用数据标题（文档标题、听记标题、会议标题）
5. Topic 树用热门实体定义
6. Global 树用时间定义

## 涉及的关键文件

- `src/openhuman/memory/tree/tree_source/summariser/llm.rs` — LLM summariser 的 system prompt
- `src/openhuman/memory/tree/content_store/compose.rs` — Summary 文件的 front-matter 和 alias 组成
- `src/openhuman/memory/tree/content_store/paths.rs` — Summary 文件路径和文件名生成
- `src/openhuman/memory/tree/content_store/tags.rs` — Tag 的 slugify 和格式
- `src/openhuman/memory/tree/score/extract/llm.rs` — LLM entity extractor 的 prompt
- `src/openhuman/memory/tree/tree_source/bucket_seal.rs` — Seal 时 compose input 的构建
- `src/openhuman/memory/tree/tree_global/digest.rs` — Global tree 日摘要构建
- `src/openhuman/memory/tree/tree_topic/registry.rs` — Topic tree 的 scope 定义

---

## 任务清单

### 1. Summariser System Prompt 改为中文输出

**文件**: `src/openhuman/memory/tree/tree_source/summariser/llm.rs`

修改 `system_prompt()` 函数，要求模型以中文进行总结：

```rust
fn system_prompt(_budget: u32, structured_facets: bool) -> String {
    let base = "你是一位精准的摘要生成器。请将用户提供的内容总结为一段连贯的中文段落，\
     保留具体事实、决策和时间顺序。不要编造事实。\n\
     \n\
     请先输出中文摘要正文。";
    // ...
}
```

- 将 base prompt 改为中文指令
- structured facets 部分的指令也需要中文化（facet key/value 保持英文不变，因为是结构化数据）
- `build_user_prompt` 中的 contribution header 保持 `[id]` 格式不变

### 2. Entity Extractor Prompt 中文化 — 让 tag 输出中文

**文件**: `src/openhuman/memory/tree/score/extract/llm.rs`

修改 `build_system_prompt()` 函数：
- 要求模型以中文输出 entity 的 `surface` 值（如人名用中文、组织名用中文）
- entity `kind` 保持英文枚举不变（`Person`, `Organization` 等），因为是代码枚举
- topic 输出要求使用中文标签

### 3. Tag 格式支持中文

**文件**: `src/openhuman/memory/tree/content_store/tags.rs`

当前 `slugify_tag_value` 和 `slugify_tag_kind` 将所有非 ASCII 字符替换为 `-`，这会丢失中文。需要：

- 修改 `slugify_tag_value()`: 保留 CJK 字符（Unicode `\u4e00-\u9fff` 等范围），不对中文做 slugify
- 修改 `slugify_tag_kind()`: 同理保留中文字符
- `entity_tag()` 输出格式从 `person/Alice-Smith` 变为 `person/张三` 或 `人物/张三`

**注意**: Obsidian 支持中文 tag，所以 `#人物/张三` 是合法的。

### 4. Summary 文件名使用中文

**文件**: `src/openhuman/memory/tree/content_store/paths.rs`

当前 `summary_filename()` 生成纯英文 ID 格式的文件名。需要引入一个新的机制，让 Summary 文件名可以使用中文标题。

方案：
- 在 `SummaryComposeInput` 中新增一个可选字段 `display_title: Option<&'a str>`
- `summary_rel_path` / `summary_filename` 增加一个带 title 的重载版本
- 当 `display_title` 有值时，文件名使用 `{中文标题}-L{level}.md` 格式
- 当 `display_title` 无值时，回退到当前的 ID 格式

**文件名安全性**: 中文字符在 macOS/Linux/Windows (NTFS) 上都是合法的文件名字符，只需要过滤 `\/:*?"<>|` 等特殊字符。

**文件**: `src/openhuman/memory/tree/content_store/compose.rs`

- `SummaryComposeInput` 新增 `display_title: Option<&'a str>` 字段

### 5. Summary Alias（别名）中文化

**文件**: `src/openhuman/memory/tree/content_store/compose.rs`

修改 `build_summary_alias()` 函数：

```rust
fn build_summary_alias(r: &SummaryComposeInput<'_>) -> String {
    let date_range = format_date_range(r.time_range_start, r.time_range_end);
    match r.tree_kind {
        SummaryTreeKind::Source => {
            let scope_short = scope_short_label(r.tree_scope);
            // 如果有 display_title，优先用标题
            if let Some(title) = r.display_title {
                format!("L{} · {} · {}", r.level, title, date_range)
            } else {
                format!("L{} · {} · {} 条子记录 · {}", r.level, scope_short, r.child_count, date_range)
            }
        }
        SummaryTreeKind::Global => {
            format!("L{} · 全局日报 · {}", r.level, date_range)
        }
        SummaryTreeKind::Topic => {
            let entity = r.tree_scope.split_once(':').map(|(_, v)| v).unwrap_or(r.tree_scope);
            format!("L{} · 主题：{} · {} 条子记录", r.level, entity, r.child_count)
        }
    }
}
```

### 6. Content 树（Source Tree）使用数据标题构建

**文件**: `src/openhuman/memory/tree/tree_source/bucket_seal.rs`

在 seal 时构建 `SummaryComposeInput`，需要从 source tree 的 scope 和子节点中提取标题信息：

- 对于 `document` 类型：scope 本身通常包含文档标题信息，或从 chunk 的内容中提取 `# 标题`
- 对于 `chat` 类型：使用频道/群组名作为标题
- 对于 `email` 类型：使用参与者名称或主题

在 `SummaryComposeInput` 构建时填充 `display_title`。

### 7. Topic 树使用热门实体名称

**文件**: `src/openhuman/memory/tree/tree_topic/registry.rs`

Topic tree 的 scope 已经是 entity_id（如 `person:张三`），当构建 `SummaryComposeInput` 时：
- 从 entity_id 中提取实体名称作为 `display_title`
- 在 `bucket_seal.rs` 中，当 `tree.kind == TreeKind::Topic` 时，从 `tree.scope` 解析实体表面形式

### 8. Global 树使用时间标题

**文件**: `src/openhuman/memory/tree/tree_global/digest.rs` 和 `seal.rs`

Global tree 的标题策略：
- L0 (日): `2026年5月21日`
- L1 (周): `2026年5月第3周`
- L2 (月): `2026年5月`
- L3 (年): `2026年`

在构建 `SummaryComposeInput` 时将中文时间字符串作为 `display_title`。

### 9. 更新测试

需要更新以下测试文件以适配中文化改动：
- `src/openhuman/memory/tree/content_store/compose.rs` 中的测试
- `src/openhuman/memory/tree/content_store/tags.rs` 中的测试
- `src/openhuman/memory/tree/content_store/paths.rs` 中的测试
- `src/openhuman/memory/tree/tree_source/summariser/llm.rs` 中的测试
- `app/src/utils/tauriCommands/memoryTree.test.ts` 中相关测试
- `app/src/components/intelligence/MemoryGraph.tsx` 中的 `openSummaryInObsidian` 函数（summary 路径拼接逻辑）

### 10. 前端 Obsidian 深链接适配

**文件**: `app/src/components/intelligence/MemoryGraph.tsx`

`openSummaryInObsidian` 中拼接路径时使用的 `slugify` 和路径构建逻辑需要适配中文文件名。确保 `obsidian://open?path=...` URI 中的中文路径正确 URL 编码。

---

## 改动范围估算

| 任务 | 复杂度 | 文件数 |
|------|--------|--------|
| Summariser prompt 中文化 | 低 | 1 |
| Extractor prompt 中文化 | 低 | 1 |
| Tag 格式支持中文 | 中 | 1 |
| Summary 文件名中文化 | 中 | 2-3 |
| Alias 中文化 | 低 | 1 |
| Content 树标题提取 | 中 | 2 |
| Topic 树实体名称 | 低 | 1 |
| Global 树时间标题 | 低 | 2 |
| 测试更新 | 中 | 4-5 |
| 前端深链接适配 | 低 | 1 |

**总计**: 约 12-15 个文件需要修改


updateAtTime: 2026/5/21 15:32:47

planId: a5aca29f-4093-476f-84a0-5a064a44b31d