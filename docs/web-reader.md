# Web 阅读模式

在桌面应用里启动一个**进程内 HTTP 服务**，用手机或平板的浏览器打开本机地址即可阅读书架与正文。设计对标 [Legado（阅读 3.0）的 HTTP Server](https://github.com/gedoor/legado)，范围也与之对齐。

## 快速开始

1. 桌面端打开「设置 → 应用 → Web 访问」；
2. 打开「启用 Web 访问」；
3. 如需手机访问，再打开「允许局域网访问」；
4. 端口与开关改动立刻生效（端口在输入框失焦时提交，避免边打字边重启监听）；
5. 手机与电脑连同一个 WiFi，用浏览器打开界面上显示的地址。

默认端口 `1122`，与 Legado 的 `webPort` 默认值一致。

## 安全：没有鉴权

这是刻意的设计决定，不是漏做：

- **与 Legado 的手机端一致**：它就是"开关 + 端口"，不做账号体系；
- 单用户应用（全部数据在 `default` 命名空间），登录页只是多一道门；
- 想躺着看书，不该先在手机上折腾一套凭据。

代价必须说清楚：

| 配置 | 谁能访问 |
|---|---|
| 默认（`127.0.0.1`） | 只有本机 |
| 打开「允许局域网访问」（`0.0.0.0`） | **同一网络里的任何人**，无需密码 |

所以：只在可信网络（家里 WiFi）打开局域网开关；咖啡厅、公司、宿舍这类共享网络里请留在回环。关掉开关即停止监听并释放端口。

> 曾经有过一版 HTTP Basic + cookie 会话（`src/server/auth.rs`），已整体移除。Basic 那条路实测不可行：浏览器弹框登录后地址栏会带 `user:pass@host`，而规范禁止用带凭据的 URL 构造 `fetch`，SPA 的接口请求会全部抛 `TypeError`。要恢复鉴权请照这个教训重做，而不是把 Basic 加回来。

## 提供的能力

| 能力 | 说明 |
|---|---|
| 书架 | 含分组、缓存章节数、阅读进度 |
| 书籍详情 | 书籍信息、封面、目录 |
| 目录 | 本地 TXT / EPUB / PDF 与网络书源 |
| 正文 | 含本地书与远程书源 |
| 阅读进度 | 回写书架 |
| 书签 / 替换规则 | 阅读页内的日常操作，读写都支持 |
| 封面 / PDF / EPUB 资源 | 复用桌面端的 `reader://` 路由实现 |
| 书源增删改 | 通过 Web 端页面管理 |
| WiFi 传本地书 | TXT / EPUB / PDF 上传，上限 50 / 80 / 100 MB |

### 有意不提供

多源搜索、换源、书源调试、备份恢复、WebDAV、听书、缓存整本、更新器、编辑书籍信息、分组管理、删除书籍。

这些能力在桌面端依赖 `tauri::ipc::Channel`（流式进度）或 `AppHandle`（窗口、文件对话框），改造成 WebSocket / 上传表单是另一个量级的工程。**对应的按钮在 Web 端已经隐藏**，而不是留着点了报 404。

## 架构

```
浏览器 ──HTTP──> src/server/router.rs ──> crate::api::commands::*_core ──> src/service/*
桌面端 ──IPC───> #[tauri::command]      ──┘
```

关键点：**业务逻辑只有一份**。`src/api/commands/*.rs` 里的每个命令都被拆成

- `xxx_core(&AppState, req) -> Result<ApiResponse<T>, AppError>` —— 真正实现；
- `#[tauri::command] pub async fn xxx(...)` —— 三行薄壳，转调 core。

HTTP handler 直接调 core，因此桌面端与浏览器在本地书、书源回退这些边界上不会行为分叉。新增接口时**先抽 core 再挂路由**；直接调 service 的那条路已经踩过坑（`saveBook` 漏掉了书源候选过滤与统计迁移）。

### 模块

| 文件 | 职责 |
|---|---|
| `src/server/mod.rs` | `WebOptions` / `WebServer` 生命周期（含同端口重启的 bind 重试） |
| `src/server/router.rs` | axum 路由表，每个 handler 都是 core 的薄适配 |
| `src/server/assets.rs` | 静态资源（Tauri 内嵌资源或磁盘目录）+ SPA 回退 |
| `src/server/settings.rs` | `web-settings.json` 读写（原子写） |
| `src/server/control.rs` | 启停、状态查询、配置应用（IPC 命令） |
| `src/util/path_safe.rs` | 路径校验，`/files` 与静态资源共用 |

### 前端

`frontend/src/api/transport/` 抽象出两个实现：

- `tauri.ts` —— `invoke()`，走 IPC；
- `http.ts` —— `fetch()`，走 HTTP。

`invoke.ts` 只保留协议无关部分（路径归一化、信封解包、错误归一化），运行时按 `__TAURI_INTERNALS__` 是否存在选择实现。**同一份 `dist` 既能装进 Tauri，也能被浏览器直接打开**。

运行环境判定统一收在 `api/transport`，三个函数别混用：

- `isTauriRuntime()` —— 是不是桌面应用；
- `isWebUiMode()` —— 产品语义上的"Web 精简模式"（非 Tauri，含 Vite dev server）；
- `isWebReaderMode()` —— 是否真由 `src/server` 托管（决定资源地址写相对还是绝对）。

Vite dev server（5173）里没有后端：界面该按 Web 精简，但资源仍要拼绝对地址去连桌面端的 `reader://` —— 这就是为什么需要两个函数而不是一个。

## 已知限制

- **不做鉴权**：见上文「安全」。
- **F11 全屏在浏览器里是空操作**：桌面端走 Tauri 窗口 API，浏览器需改用 `requestFullscreen()`。
- **上传整个读进内存**：上限 50 / 80 / 100 MB，加上整份 `field.bytes()`，同时两三个人传大 PDF 会明显吃内存。

## 验证

`cargo test --lib -j 4 server::` 覆盖配置读写、静态资源路径、上传体积上限、路径穿越回归。

真实浏览器验证清单（已手工走通）：

1. 访问 `/` → 书架渲染（封面、章数、进度、分组）；
2. 点书进阅读页 → 正文渲染、翻页正常；
3. 重启应用 → 进度仍在；
4. `GET /files?path=..%5C..%5CWindows%5Cwin.ini` → 400（路径穿越被拒）；
5. 上传 3MB 的 TXT → 成功（不再被 axum 的 2MB 默认上限拦成 413）。
