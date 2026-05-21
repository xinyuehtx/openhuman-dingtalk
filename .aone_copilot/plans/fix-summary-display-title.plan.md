### fix-summary-display-title ###
修复 Summary 文件名使用无意义的 opaque ID（如 doc-9bN7RYPWdPxOBgrQuqjK0Ab5WZd1wyK0-L1）的问题，改为使用「文档标题（截断）-短hash-L级别」格式生成可读的文件名。


## 问题根因

Summary 文件名由 `bucket_seal.rs` 中的 `scope_short_label_for_title` 函数生成 `display_title`。对于 `document:` 类型的 tree scope，该函数只是简单取冒号后的部分（如 `composio-notion-page-9bN7RYPWdPxOBgrQuqjK0Ab5WZd1wyK0`），而不是文档的人类可读标题。

文档的真正标题（如"项目设计文档"）存在于：
- chunk 的 markdown 内容中（`# <title>` 开头，由 `canonicalize/document.rs` 生成）
- ingest 时的 `DocumentInput.title` 字段

但这个标题没有被存储到 tree 的 `scope` 字段中，也没有其他途径传递到 seal 阶段。

### 数据流分析

```
DocumentInput.title = "项目设计文档"
    ↓ canonicalise()
CanonicalisedSource.markdown = "# 项目设计文档\n\n..."  (标题在 body 中)
    ↓ persist() → ingest
source_id = "document:composio-notion-page-<opaque_id>"  (无标题信息)
    ↓ get_or_create_source_tree()
tree.scope = "document:composio-notion-page-<opaque_id>"  (无标题信息)
    ↓ seal → scope_short_label_for_title()
display_title = "composio-notion-page-<opaque_id>"  ← 无意义！
    ↓ sanitize_display_title()
filename = "composio-notion-page-<opaque_id>-L1.md"  ← 无意义！
```

## 修复方案

改进 `scope_short_label_for_title` 函数，当 scope 中取到的 label 看起来像 opaque ID（长度过长 + 无中文/有意义的词汇）时，fallback 到从该 tree 下最近的 chunk 内容中提取文档标题（`# <title>` header）。

同时，按照用户的建议，采用「**文档标题（截断）-短hash**」的命名格式，使得文件名既可读又唯一。

### 具体步骤

#### Step 1: 修改 `scope_short_label_for_title` — 增加从 chunk 内容提取标题的能力

**文件**: `src/openhuman/memory/tree/tree_source/bucket_seal.rs`

当前 `scope_short_label_for_title` 是一个纯函数，只解析 scope 字符串。问题在于 document 的 scope 中没有人类可读的信息。

修改 `bucket_seal.rs` 中构造 `display_title_owned` 的逻辑块（约 L569-593），对 **非 Topic** 的 Source 树增加一个新路径：

```rust
// 对 Source 树中的 document 类型：
// 1. 先尝试从 scope 中取 label（现有逻辑）
// 2. 如果 label 看起来像 opaque ID，fallback 到从已缓冲的
//    子节点（chunk）内容中提取 H1 标题
// 3. 如果还是取不到，使用截断的 scope + 短 hash
```

具体方案：在 `seal_one_level` 函数中，构造 `display_title_owned` 时：

1. 调用现有的 `scope_short_label_for_title` 获取 label
2. 检查 label 是否"可读"（新增一个 `is_opaque_id` 判断函数 — 如果 label 长度 > 40 或匹配 `composio-*-page-*` 等 opaque 模式则判定为不可读）
3. 如果不可读，从 `node.child_ids` 对应的 chunk 内容中提取 `# <title>` 的第一行作为标题
4. 最终 display_title 格式: `{title_truncated}-{short_hash}` 其中:
   - `title_truncated`: 标题截断到 30 个字符
   - `short_hash`: scope 的 SHA256 前 6 位 hex

#### Step 2: 新增辅助函数

**文件**: `src/openhuman/memory/tree/tree_source/bucket_seal.rs`

```rust
/// 判断一个 label 是否看起来像 opaque ID（不可读）
fn is_opaque_label(label: &str) -> bool

/// 从 chunk 内容列表中提取第一个 H1 标题
fn extract_h1_title_from_children(children_content: &[&str]) -> Option<String>

/// 构建「标题-短hash」格式的 display title
fn build_readable_display_title(title: &str, scope: &str) -> String
```

#### Step 3: 修改 `sanitize_display_title` 增加长度限制

**文件**: `src/openhuman/memory/tree/content_store/paths.rs`

当前 `sanitize_display_title` 对标题长度没有硬性限制。需要增加截断逻辑（按字符数截断到 50 字符），避免文件名过长。

```rust
pub(crate) fn sanitize_display_title(title: &str, level: u32) -> String {
    let sanitised: String = title
        .chars()
        .map(|c| match c { ... })
        .collect();
    let trimmed = sanitised.trim().trim_matches('-');
    if trimmed.is_empty() {
        format!("untitled-L{level}")
    } else {
        // 截断到 50 个字符
        let truncated: String = trimmed.chars().take(50).collect();
        let truncated = truncated.trim_end_matches('-').trim();
        format!("{truncated}-L{level}")
    }
}
```

#### Step 4: 获取 chunk 内容以提取标题

在 `seal_one_level` 中，`node.child_ids` 包含了子节点的 chunk ID。对于 L1 seal（chunk → summary），需要读取 chunk 内容来提取标题。

已有的 `SummaryInput` 在 `prepare_inputs` 中被构造，包含 chunk 的 `content` 字段。利用已有的 `inputs` 变量（`Vec<SummaryInput>`），在构造 `display_title_owned` 之前，从 `inputs[0].content` 中提取 H1 标题即可。

**文件**: `src/openhuman/memory/tree/tree_source/bucket_seal.rs`

关键修改点在 `seal_one_level` 函数中约 L569 处：

```rust
let display_title_owned: Option<String> = match tree.kind {
    TreeKind::Topic => { /* 现有逻辑不变 */ },
    _ => {
        let label = scope_short_label_for_title(&tree.scope);
        if label.is_empty() {
            None
        } else if is_opaque_label(&label) {
            // 尝试从子节点内容中提取 H1 标题
            let h1_title = inputs.iter()
                .find_map(|inp| extract_h1_title(&inp.content));
            match h1_title {
                Some(title) => Some(build_readable_display_title(&title, &tree.scope)),
                None => Some(build_readable_display_title(&label, &tree.scope)),
            }
        } else {
            Some(label)
        }
    }
};
```

#### Step 5: 添加单元测试

**文件**: `src/openhuman/memory/tree/tree_source/bucket_seal_tests.rs`

- 测试 `is_opaque_label` 正确识别 opaque ID vs 有意义的 label
- 测试 `extract_h1_title` 从 `# Title` markdown 中提取标题
- 测试 `build_readable_display_title` 生成正确的「标题-短hash」格式
- 测试 `sanitize_display_title` 的截断行为

#### Step 6: 运行现有测试确保无回归

```bash
cargo test --lib -p openhuman -- bucket_seal
cargo test --lib -p openhuman -- content_store
cargo test --lib -p openhuman -- paths
```

### 最终效果

修复前: `composio-notion-page-9bN7RYPWdPxOBgrQuqjK0Ab5WZd1wyK0-L1.md`

修复后: `项目设计文档-a3f2b1-L1.md`（标题截断 + scope 的 6 位 short hash + level）

### 注意事项

- 已有的 vault 中旧文件不会受影响（immutability contract），只影响新 seal 的 summary
- 短 hash 保证了不同文档即使标题相同也不会冲突
- 标题截断到合理长度避免文件系统路径限制
- 中文标题保持原样，英文标题保持原样，只做违规字符替换和长度截断


updateAtTime: 2026/5/21 16:27:17

planId: 288a5b54-c49b-4cfe-bff2-570bb6623fda