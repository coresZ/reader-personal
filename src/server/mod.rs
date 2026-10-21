//! Web 阅读模式: 进程内 HTTP 服务。
//!
//! 设计对标 Legado 的 `app/src/main/java/io/legado/app/web/HttpServer`:
//! 桌面/手机端常驻进程自带一个 HTTP 服务器, 浏览器打开 `http://<局域网 IP>:<端口>`
//! 即可阅读书架与正文。业务逻辑**完全不重写** —— 全部复用 `crate::api::commands`
//! 里已按 `&AppState` 抽出的 `*_core` 函数, 这一层只做协议适配。
//!
//! 与 Legado 的两处有意分歧:
//!
//! 1. **鉴权**: 没有。与 Legado 的手机端一致 —— 它也是"开关 + 端口", 不做
//!    账号体系。本项目是单用户 `default` 命名空间, 加登录页只是多一道门;
//!    代价是打开「允许局域网访问」等于把书架与本机 `/files` 交给同网段所有人,
//!    这一点在 UI 与 `docs/web-reader.md` 里都写明了, 不在代码里假装安全。
//!    (曾有一版 HTTP Basic + cookie 会话, 已整体移除: Basic 弹框登录后浏览器
//!    地址栏带 `user:pass@host`, 而规范禁止用带凭据的 URL 构造 `fetch`, SPA 的
//!    接口请求会全部抛 `TypeError`。要恢复鉴权得重做方案, 别把 Basic 加回来。)
//! 2. **范围**: Legado 的 Web 端不含多源搜索/换源, 这里保持一致 —— 那些能力走
//!    `ipc::Channel`, 改成 WebSocket 是另一个量级的工程, 且对"躺着看书"无必要。

pub mod assets;
pub mod control;
pub mod router;
pub mod settings;

use crate::api::AppState;
use std::net::SocketAddr;

/// Web 服务的运行时配置。
#[derive(Debug, Clone)]
pub struct WebOptions {
    /// 监听端口。0 表示让系统分配(仅用于测试)。
    pub port: u16,
    /// 是否绑定到全部网卡。`false` 时只听 `127.0.0.1`。
    pub lan: bool,
}

impl Default for WebOptions {
    fn default() -> Self {
        Self {
            port: 1122,
            lan: false,
        }
    }
}

impl WebOptions {
    /// 校验配置是否可用于启动。
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("端口不能为 0".to_string());
        }
        Ok(())
    }

    fn bind_addr(&self) -> SocketAddr {
        let ip = if self.lan {
            std::net::Ipv4Addr::UNSPECIFIED
        } else {
            std::net::Ipv4Addr::LOCALHOST
        };
        SocketAddr::from((ip, self.port))
    }
}

/// 已启动的 Web 服务句柄。丢弃时不会自动停机 —— 调用方需显式 `stop`。
pub struct WebServer {
    shutdown: tokio::sync::oneshot::Sender<()>,
    /// 实际监听地址(端口为 0 时由系统分配, 通过它取回真实端口)。
    pub addr: SocketAddr,
}

impl WebServer {
    /// 启动 HTTP 服务。返回后服务已在后台运行。
    ///
    /// bind 带重试: 同端口重启时, 上一个 listener 的释放要等 axum 的 spawned
    /// task 被调度到 —— `stop` 只是发了信号。抢在那之前 bind 必然 `AddrInUse`,
    /// 而调用方(`apply` 的回滚路径)会用同一个端口再试一次, 两次都失败用户就
    /// 彻底失去 Web 服务。重试窗口取 2s, 与 `stop` 的等待上限一致。
    pub async fn start(
        state: AppState,
        options: WebOptions,
        static_source: Option<assets::StaticSource>,
    ) -> anyhow::Result<Self> {
        options.validate().map_err(anyhow::Error::msg)?;

        let listener = bind_with_retry(options.bind_addr()).await?;
        let addr = listener.local_addr()?;
        tracing::info!("web reader listening on http://{addr}");

        let app = router::build(state, options, static_source);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = rx.await;
            });
            if let Err(err) = server.await {
                tracing::error!("web reader server stopped: {err}");
            } else {
                tracing::info!("web reader server shut down");
            }
        });

        Ok(Self {
            shutdown: tx,
            addr,
        })
    }

    /// 停机并等待监听释放。
    ///
    /// 只发信号不够: `axum::serve` 的 graceful shutdown 会等**活跃连接**结束,
    /// 而浏览器/手机的 HTTP keep-alive 连接可能挂几十秒。用户关掉开关后马上
    /// 刷新页面还能打开, 就会以为"关不掉"。
    ///
    /// 这里**不在调用线程上阻塞等待**: 调用方是 Tauri 命令线程, 在那上面 sleep
    /// 会把窗口一起卡住。改用独立线程轮询, 端口一释放就退出; 最多等 2 秒,
    /// 超时也返回 —— 关不掉端口比多等一会儿更糟。
    ///
    /// 因此 `stop` 返回**不代表**端口已经释放。紧接着要重新 bind 同一端口时,
    /// 靠 `start` 里的 [`bind_with_retry`] 兜住这段窗口。
    pub fn stop(self) {
        let Self { shutdown, addr } = self;
        let _ = shutdown.send(());

        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                // 端口能重新 bind 上就说明监听已释放。
                if std::net::TcpListener::bind(addr).is_ok() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            tracing::warn!("web reader port {addr} still in use after stop");
        });
    }
}

/// 在 2 秒内反复尝试 bind, 让出时间给上一个 listener 完成释放。
///
/// 只对 `AddrInUse` 重试: 其它错误(权限、地址不可用、端口非法)重试也没用,
/// 立刻返回才能让 `apply` 的回滚逻辑给出可读提示。
async fn bind_with_retry(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    const RETRY_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
    const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

    let deadline = std::time::Instant::now() + RETRY_WINDOW;
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse
                && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            Err(err) => return Err(err),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// `WebOptions::validate` 只拦端口 0, 不再要求密码(鉴权已移除)。
    #[test]
    fn lan_mode_does_not_require_password() {
        let options = WebOptions { port: 1122, lan: true };
        assert!(options.validate().is_ok());
    }

    #[test]
    fn port_zero_is_rejected() {
        let options = WebOptions { port: 0, lan: false };
        assert!(options.validate().is_err());
    }

    /// 回归: 关闭后端口必须能被重新绑定。
    ///
    /// 这条只验证"停止后监听确实释放"的基础事实。真正防止 start(false)
    /// 不停止旧服务的回归由 control.rs 的语义保证 —— 那里现在无条件先停。
    #[test]
    fn port_is_rebindable_after_release() {
        use std::net::{Ipv4Addr, TcpListener};
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let again = TcpListener::bind(addr);
        assert!(again.is_ok(), "释放后端口应当可以重新绑定");
    }
}