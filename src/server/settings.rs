//! Web 阅读模式的配置持久化。
//!
//! 与 `window-state.json` 一样放在便携数据目录下, 而不是 Tauri 的 app config
//! 目录 —— "拷一个文件夹带走全部状态"是这个项目的既定约定。
//!
//! 只有三项: 是否启用、端口、是否允许局域网。与手机端 Legado 的 HTTP Server
//! 对齐 —— 它也是"开关 + 端口", 不做账号体系。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 默认端口。与 Legado 的 `webPort` 默认值一致, 方便两个产品换着用时记一个数。
pub const DEFAULT_PORT: u16 = 1122;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSettings {
    /// 是否启用 Web 访问。
    #[serde(default)]
    pub enabled: bool,
    /// 监听端口。
    #[serde(default = "default_port")]
    pub port: u16,
    /// 是否绑定全部网卡(局域网访问)。
    #[serde(default)]
    pub lan: bool,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            lan: false,
        }
    }
}

impl WebSettings {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(err) => {
                // 静默回落会让用户看到"Web 访问自己关了"却毫无线索。写坏的
                // 原因几乎都是上一次写入被中断(见 `save`), 留一条日志。
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "web settings 解析失败, 已回落到默认值(Web 访问关闭)"
                );
                Self::default()
            }
        }
    }

    /// 原子写: 先写同目录临时文件, 再 rename 覆盖。
    ///
    /// 早先这里是 `std::fs::write` —— 它会先截断目标再写。进程在两步之间退出
    /// (关机、任务管理器结束、崩溃), 留下的就是一个 0 字节文件, 下次启动
    /// `load` 解析失败, 用户会看到"Web 访问莫名其妙变回关闭"。
    ///
    /// 临时名带进程 id: 便携版可能被复制成两份同时跑, 各自写也不会互相截断。
    /// rename 在 Windows 上覆盖已存在文件是允许的(同目录、同卷)。
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, body)?;
        match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(err) => {
                // rename 失败时把临时文件收掉, 免得攒一堆 .tmp-*。
                let _ = std::fs::remove_file(&tmp);
                Err(err.into())
            }
        }
    }

    /// 转成服务器所需的运行时选项。
    pub fn to_options(&self) -> super::WebOptions {
        super::WebOptions {
            port: self.port,
            lan: self.lan,
        }
    }
}

/// 配置文件的固定位置。
pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("web-settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_loopback() {
        let s = WebSettings::default();
        assert!(!s.enabled);
        assert!(!s.lan);
        assert_eq!(s.port, DEFAULT_PORT);
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let s = WebSettings::load(Path::new("does-not-exist.json"));
        assert!(!s.enabled);
    }

    #[test]
    fn roundtrips_through_disk() {
        let dir = std::env::temp_dir().join(format!("reader-web-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("web-settings.json");

        let settings = WebSettings {
            enabled: true,
            port: 1234,
            lan: true,
        };
        settings.save(&path).unwrap();

        let loaded = WebSettings::load(&path);
        assert!(loaded.enabled);
        assert_eq!(loaded.port, 1234);
        assert!(loaded.lan);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("reader-web-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("web-settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(!WebSettings::load(&path).enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧版配置里带 username/password/sessionFile, 读进来必须被忽略而不是报错,
    /// 否则升级后用户会突然发现 Web 访问被重置成"未启用"。
    #[test]
    fn legacy_fields_are_ignored() {
        let dir = std::env::temp_dir().join(format!("reader-web-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("web-settings.json");
        std::fs::write(
            &path,
            r#"{"enabled":true,"port":9000,"lan":true,"username":"reader","password":"x"}"#,
        )
        .unwrap();

        let loaded = WebSettings::load(&path);
        assert!(loaded.enabled);
        assert_eq!(loaded.port, 9000);
        assert!(loaded.lan);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归: 写入必须是"临时文件 + rename", 不能留下半截文件。
    ///
    /// 直接测"原子性"很难, 但可以测它**没有**留下的东西: 保存成功后目录里
    /// 只该有目标文件, 不该有 `.tmp-*` 残留; 同时文件内容必须是完整 JSON。
    #[test]
    fn save_leaves_no_temp_files_behind() {
        let dir = std::env::temp_dir().join(format!("reader-web-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("web-settings.json");

        WebSettings {
            enabled: true,
            port: 4321,
            lan: false,
        }
        .save(&path)
        .unwrap();

        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        assert_eq!(names, vec!["web-settings.json".to_string()], "目录里不该有临时文件残留");

        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("落盘内容必须完整可解析");
        assert_eq!(parsed["port"], 4321);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 覆盖写同一路径也要成功 —— Windows 上 `rename` 到已存在的文件是合法的,
    /// 这条锁住"换端口时能反复保存"这个日常路径。
    #[test]
    fn save_overwrites_existing_file() {
        let dir = std::env::temp_dir().join(format!("reader-web-overwrite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("web-settings.json");

        for port in [1111u16, 2222] {
            WebSettings {
                enabled: true,
                port,
                lan: false,
            }
            .save(&path)
            .unwrap();
            assert_eq!(WebSettings::load(&path).port, port);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
