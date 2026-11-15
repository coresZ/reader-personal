# 搜索分页 hasMore 判定缺陷与改造方案

## 现象

书源「搬山人小说网」搜索：

```text
GET https://www.banshanren.com/api/search?keyword=皇&page=1
```

实际响应（节选语义）：

```json
{
  "result": "success",
  "model": {
    "currentPage": 1,
    "totalCount": 40,
    "totalPage": 2,
    "pageSize": 24,
    "data": [ /* 24 条 */ ]
  }
}
```

- 第一页只返回 **24** 条，总数 **40**，还有第 2 页。
- 当前客户端只消费 `$.model.data` 列表，且多源搜索路径**固定只请求 page=1**，用户看不到后续结果，表现为「没有下一页 / 结果不全」。

## 根因（代码层）

### 1. 多源搜索 SSE 写死第一页

`src/api/commands/book.rs` → `search_book_multi_sse`：

```rust
svc.search_book(&user_ns_value, &source, &k, 1).await;
```

对每个书源只搜 **page=1**。`searchUrl` 里的 `{{page}}` 永远是 1。  
`hasMore` / `lastIndex` 在这条链路上表示的是「还有没有**下一个书源**」，**不是**「当前书源还有没有**下一页结果**」。

### 2. 规则只解析列表，丢弃分页元数据

`ruleSearch.bookList = $.model.data` 只能抽出本页条目。  
响应里的 `totalCount` / `totalPage` / `pageSize` / `currentPage` 没有进入引擎，也就无法据此判断是否还有下一页。

### 3. 发现页「空列表即结束」在部分场景可用，但不统一

`frontend/src/stores/explore.ts` 大致是：

- 本页 `length > 0` → `page++`，继续
- 本页 `length === 0` → `hasMore = false`

这是 Legado 常见的「直到空页」策略，对 HTML 列表源通常够用。  
但**搜索主路径并不走这套翻页**；且若将来用「本页条数 < 固定 pageSize ⇒ 没有更多」而又遇到「末页刚好满页 / 首屏不足一页但仍有 total」等边界，仍会误判。搬山人这种 **JSON 明确给出 totalPage** 的接口，应优先信任服务端元数据。

## 实测对照

| 字段 | page=1（keyword=皇） |
|------|----------------------|
| data.length | 24 |
| pageSize | 24 |
| totalCount | 40 |
| totalPage | （≥2） |
| 客户端行为 | 只拿到 24 条后结束 |

## 改造目标

1. 单书源搜索能按 `{{page}}` 连续翻页，直到没有更多。
2. `hasMore` 判定优先使用接口元数据，其次才是启发式。
3. 多源搜索在「每个源」维度可拉多页（有上限），避免只显示第一页。
4. 不破坏现有 HTML/正则书源的「空页结束」行为。

## 推荐方案（分阶段）

### 阶段 A — 引擎识别 JSON 分页元数据（推荐优先）

在解析搜索响应时（`search_books_json` 或 `search_book` 返回层）尝试读取常见字段：

| 优先级 | 字段示例 | 含义 |
|--------|----------|------|
| 高 | `model.totalPage` + `model.currentPage` | `hasMore = currentPage < totalPage` |
| 高 | `totalPage` / `currentPage` | 同上 |
| 中 | `totalCount` + `pageSize` + `currentPage` | `hasMore = currentPage * pageSize < totalCount` |
| 低 | 本页 `data.length` | `hasMore = length > 0`（继续翻，直到空页） |
| 慎用 | `length >= pageSize` | 仅作补充；满页不一定有下一页，不满页也可能有（少见） |

返回结构建议：

```rust
struct SearchPage {
    books: Vec<SearchBook>,
    has_more: bool,
    current_page: i32,
    total_page: Option<i32>,
}
```

`search_book` 对调用方暴露 `has_more`，而不是只返回 `Vec`。

### 阶段 B — 多源 SSE：源内翻页

`search_book_multi_sse` 对每个源：

```text
page = 1
loop:
  books, has_more = search_book(source, key, page)
  emit books
  if !has_more: break
  page += 1
  if page > max_pages_per_source: break   // 建议默认 5～10，可配置
```

注意：

- 仍按书源并发，但**同一书源内串行翻页**（遵守 `concurrentRate`）。
- `lastIndex` 语义保持「书源下标」；源内页码不塞进 lastIndex。
- 可选：达到全局 `searchSize` 条数后提前结束，避免一次搜爆。

### 阶段 C — 前端（可选增强）

- 单源搜索模式：若后端仍只回一页，UI 提供「加载更多」并传 `page`。
- 展示「已加载 x / 约 y 条」（当有 totalCount 时）。

### 阶段 D — 书源侧兜底（不推荐作为唯一手段）

用超长 `@js` 在 `bookList` 里自己循环请求多页——难维护、难限流、与 App 取消/并发模型冲突。  
书源只需继续提供 `{{page}}`；**分页决策应在 App**。

## 搬山人书源落地

文件：`examples/book-sources/banshanren.json`

- `searchUrl` 已含 `{{page}}`，规则正确。
- 问题在 App 未翻页 / 未读 totalPage，**不必改 searchUrl 才能修**；修好引擎后该源应自动完整。

## 验收用例

1. keyword=`皇`：至少能拿到接近 `totalCount`（40）的条数（允许去重略少）。
2. 仅启用搬山人单源搜索：网络日志出现 `page=1` 与 `page=2`。
3. HTML 探索源：仍在空页停止，不出现死循环。
4. 取消搜索：源内翻页过程可被 `cancel_book_search` 打断。
5. 回归：多源合并去重逻辑不变。

## 实现顺序建议

1. 增加 `SearchPage` + JSON 元数据解析 + 单测（搬山人样例 body）。
2. `search_book_multi_sse` 源内 while has_more。
3. 探索页如有「用 pageSize 误判」的路径一并统一到同一 hasMore 工具函数。
4. （可选）前端加载更多与总数展示。

## 非目标

- 不在此任务改验证码/正文解密。
- 不要求每个书源手写 totalPage 规则字段（通用 JSON 探测即可覆盖搬山人）。
