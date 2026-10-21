//! Web 访问的运行时控制: 启停、状态查询、配置应用。
//!
//! 放在库侧而不是桌面壳, 是因为命令注册只能在
//! `reader_rust::api::invoke_handler()` 的 `generate_handler!` 里一次性完成 ——
//! 桌面壳无法再往里面追加命令。
//!
//! 关键约束: **改配置必须重启监听**。只写文件不重启, 用户会看到"已开启"却
//! 连不上。`apply` 因此先停旧服务再起新的; 新配置起不来时回滚到旧配置并尽力
//! 把旧服务拉回来, 否则用户会把端口改成被占用的值后失去原有服务。
//!
//! 这里的函数是 **async** 的, 对应的 IPC 命令也是 `async fn`。早先它们是同步
//! 函数并在内部用 `tauri::async_runtime::block_on` 起服务: 同步命令跑在 Tauri
//! 自己的线程上, 那次 block_on 会把该线程连同整个窗口一起按住几百毫秒, 而且
//! 一旦调用点落在 tokio worker 上会直接 panic("Cannot block the current
//! thread from within a runtime")。改成 await 之后既没有阻塞也没有这个隐患。
//!
//! 锁的用法: `std::sync::Mutex` 只包住"取下旧句柄 / 存回新句柄"这两步,
//! **中间不跨 await**, 因此不需要换成 `tokio::sync::Mutex`(那会让 `is_running`
//! 这类同步查询也跟着变成异步)。

use super::settings::{settings_path, WebSettings};
use super::WebServer;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

/// 运行中的 Web 服务槽位 + 数据目录。
pub struct WebServerState {
    server: Mutex<Option<WebServer>>,
    data_dir: PathBuf,
}

impl WebServerState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            server: Mutex::new(None),
            data_dir,
        }
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    pub fn is_running(&self) -> bool {
        self.server.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub lan: bool,
    /// 可直接复制给手机访问的地址。
    pub url: Option<String>,
    /// 启动失败原因(端口被占用等)。前端要如实展示, 否则用户只会看到开关弹回去。
    pub error: Option<String>,
}

impl WebStatus {
    fn build(settings: &WebSettings, running: bool, error: Option<String>) -> Self {
        let url = if running { access_url(settings) } else { None };
        Self {
            enabled: settings.enabled,
            running,
            port: settings.port,
            lan: settings.lan,
            url,
            error,
        }
    }
}

/// 本机在局域网中可被访问的 IPv4 地址(用于展示访问地址)。
///
/// 不引第三方 crate: 用一个 UDP socket "连接" 到公网地址再读本地端点,
/// 内核据此选出默认路由对应的网卡地址, 不会真的发包。
fn lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("223.5.5.5:53").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

fn access_url(settings: &WebSettings) -> Option<String> {
    if !settings.enabled || settings.port == 0 {
        return None;
    }
    if settings.lan {
        let ip = lan_ip()?;
        Some(format!("http://{ip}:{}", settings.port))
    } else {
        Some(format!("http://127.0.0.1:{}", settings.port))
    }
}

/// 把旧服务摘出来并停掉。**不持有锁**做这件事, 好让调用方接着 await 启动。
fn take_and_stop(slot: &Mutex<Option<WebServer>>) {
    let taken = slot.lock().ok().and_then(|mut guard| guard.take());
    if let Some(server) = taken {
        server.stop();
    }
}

impl WebServerState {
    /// 按给定设置启动服务(若已启用); `enabled == false` 时**停止已运行的服务**。
    ///
    /// 这里曾经是个 bug: 未启用就直接 `return Ok(())`, 没去动槽位里的旧服务,
    /// 于是关掉开关后端口仍在监听 —— 配置写着"未启用", 实际却还能访问。
    /// 开关的语义必须是"关掉 = 停止监听", 所以先无条件停掉旧服务, 再决定是否启动。
    ///
    /// **同端口重启**(改"允许局域网"、改回同一个端口时都会发生): 旧 listener 的
    /// 释放由 axum 的 graceful shutdown 负责, 它会在收到信号后立刻 `drop(listener)`,
    /// 但那要等 spawned task 被调度到。早先 `start` 在发完信号后立刻 bind, 抢在
    /// 释放之前必然 `AddrInUse`, 而 `apply` 的回滚路径又会用同一个端口再试一次 ——
    /// 连失败两次之后用户就彻底没有 Web 服务了。现在 bind 带退避重试。
    async fn start(
        &self,
        state: &crate::api::AppState,
        settings: &WebSettings,
        static_source: Option<super::assets::StaticSource>,
    ) -> Result<(), String> {
        take_and_stop(&self.server);

        if !settings.enabled {
            tracing::info!("web reader disabled, stopped listening");
            return Ok(());
        }

        let options = settings.to_options();
        options.validate()?;

        let server = WebServer::start(state.clone(), options, static_source)
            .await
            .map_err(|err| format!("{err:#}"))?;
        if let Ok(mut slot) = self.server.lock() {
            *slot = Some(server);
        }
        Ok(())
    }
}

/// 读取当前状态。
pub fn status(state: &WebServerState) -> WebStatus {
    let settings = WebSettings::load(&settings_path(state.data_dir()));
    let running = state.is_running();
    WebStatus::build(&settings, running, None)
}

/// 应用一组新设置: 落盘并重启服务。
pub async fn apply(
    state: &WebServerState,
    app_state: &crate::api::AppState,
    settings: WebSettings,
    static_source: Option<super::assets::StaticSource>,
) -> WebStatus {
    let path = settings_path(state.data_dir());
    let previous = WebSettings::load(&path);

    // 先落盘: 即使启动失败, 用户下次改端口时看到的也是他刚填的值。
    if let Err(err) = settings.save(&path) {
        let mut status = WebStatus::build(&previous, state.is_running(), None);
        status.error = Some(format!("保存配置失败: {err}"));
        return status;
    }

    match state.start(app_state, &settings, static_source.clone()).await {
        Ok(()) => WebStatus::build(&settings, state.is_running(), None),
        Err(err) => {
            // 回滚: 恢复旧设置并尽力把旧服务拉回来。
            let _ = previous.save(&path);
            let restore = state.start(app_state, &previous, static_source).await;
            let mut status = WebStatus::build(&previous, state.is_running(), None);
            status.error = Some(match restore {
                Ok(()) => err,
                Err(restore_err) => format!("{err}；恢复原设置也失败: {restore_err}"),
            });
            status
        }
    }
}

/// 启动时按已保存的配置自动开启(仅当上次是启用状态)。
///
/// 失败不能拦住应用启动: Web 只是附加能力, 桌面端必须可用。
pub async fn autostart(
    state: &WebServerState,
    app_state: &crate::api::AppState,
    static_source: Option<super::assets::StaticSource>,
) {
    let settings = WebSettings::load(&settings_path(state.data_dir()));
    if !settings.enabled {
        return;
    }
    if let Err(err) = state.start(app_state, &settings, static_source).await {
        tracing::warn!("web reader autostart failed: {err}");
        eprintln!("阅读：Web 访问启动失败：{err}");
    }
}

// ───────────────────────── IPC 命令 ─────────────────────────
//
// 这两个命令注册在 `crate::api::invoke_handler()` 里, 与其它命令同批次。

/// 读取 Web 访问的当前状态。
#[tauri::command]
pub fn get_web_status(state: tauri::State<'_, WebServerState>) -> WebStatus {
    status(&state)
}

/// 保存并应用新的 Web 访问设置。
#[tauri::command]
pub async fn apply_web_settings(
    state: tauri::State<'_, WebServerState>,
    app_state: tauri::State<'_, crate::api::AppState>,
    app: tauri::AppHandle,
    settings: WebSettings,
) -> Result<WebStatus, String> {
    let static_source = resolve_static_source(&app);
    Ok(apply(&state, &app_state, settings, static_source).await)
}

/// 决定 Web 服务的静态资源来源。
///
/// 优先用 Tauri 内嵌资源 —— release 构建把 `frontend/dist` 编进了 exe
/// (`frontendDist`), 运行时磁盘上没有 `dist/` 目录, 只有 `asset_resolver()`
/// 能读到。开发期若显式存在 `frontend/dist`, 则改用磁盘目录, 这样改完前端
/// 只要 `npm run build` 就生效, 不必重编 Rust。
pub fn resolve_static_source(app: &tauri::AppHandle) -> Option<super::assets::StaticSource> {
    use tauri::Manager;
    if let Ok(resource_dir) = app.path().resource_dir() {
        let dist = resource_dir.join("dist");
        if dist.is_dir() {
            return Some(super::assets::StaticSource::Directory(dist));
        }
    }
    Some(super::assets::StaticSource::Embedded(app.clone()))
}
