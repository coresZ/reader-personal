# 搜索分页 hasMore：通用适配方案

## 现象（搬山人示例）

```text
GET /api/search?keyword=皇&page=1
→ model.data 24 条，totalCount=40，pageSize=24
```

客户端只拿到第一页后结束。根因不限于这一种 JSON 结构。

## 根因

1. `search_book_multi_sse` 对每个书源写死 `page=1`。
2. `hasMore`/`lastIndex` 表示「下一书源」，不是「当前源下一页」。
3. `ruleSearch` 只解析列表，丢弃分页元数据。

## 设计原则

1. 不绑死某一种字段名（如 `model.totalPage`）。
2. **显式规则 > 通用探测 > 行为启发式**。
3. 宁可多翻一页遇到空列表再停，也不要在第一页非空时误判结束。
4. 同源翻页串行；源与源之间仍可并发；遵守取消与 `maxPages`。

## 判定优先级

```text
hasMore =
  ① 书源 ruleSearch 可选字段（hasMore / totalPage+page）
  ?? ② JSON 全文轻量探测（常见字段名 + data/model/result/pagination 包装）
  ?? ③ 行为：本页 0 条 → false；本页 >0 → true（直到空页）
```

硬顶：`page >= maxPagesPerSource`（默认 10）→ false。  
防死循环：连续两页书目指纹完全相同 → false。

### ② 自动探测字段（大小写不敏感）

| 类型 | 候选名 |
|------|--------|
| 当前页 | currentPage, page, pageNo, pageNum, pageIndex, pn |
| 总页 | totalPage, totalPages, pageCount, pages, lastPage |
| 总数 | total, totalCount, count, totalNum, recordCount |
| 页大小 | pageSize, size, limit, perPage, page_size |
| 布尔还有 | hasMore, has_more, hasNext, has_next |
| 布尔结束 | isEnd, is_end, finished, eof（true ⇒ 无更多） |
| 游标 | next, nextCursor, next_cursor, nextPageToken（非空 ⇒ 有更多） |

探测范围：JSON 根、以及一层 `data` / `model` / `result` / `pagination`。

### ③ 启发式

- `books.is_empty()` → 无更多（主停止条件，兼容 HTML 源）
- 否则 → 有更多（依赖空页结束）

**不用**单独的 `length < pageSize ⇒ 结束` 作为主规则。

## 多源 SSE 行为

每个源：

```text
page = 1
while page <= maxPages:
  (books, has_more) = search(source, key, page)
  emit books if not empty
  if !has_more: break
  page += 1
```

`lastIndex` 仍表示书源下标。

## 书源可选增强（二期）

`ruleSearch` 可增加：

- `hasMore`：规则表达式
- `totalPage` / `page`：与自动探测同义，优先于探测

搬山人现有 `searchUrl` 已含 `{{page}}`，**无需改 URL** 即可在引擎修复后工作。

## 验收

1. keyword=皇：可拉到接近 totalCount 的条数。
2. 单源时网络出现 page=1 与 page=2。
3. HTML 探索/搜索：空页停止，无死循环。
4. 取消搜索可打断源内翻页。
5. 多源合并去重不变。

## 落地文件

- 书源：`examples/book-sources/banshanren.json`
- 探测：`src/parser/search_pagination.rs`
- 调用：`search_book_page` + `search_book_multi_sse` 源内翻页
