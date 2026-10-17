//! Portable-mode path resolution.
//!
//! Release builds keep everything next to the executable so the whole app can be
//! moved between machines by copying one folder. Debug builds instead use a
//! fixed location in the source tree.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};

pub struct Paths {
    pub data_dir: PathBuf,
    pub storage: PathBuf,
    pub assets: PathBuf,
    pub db: PathBuf,
    /// 窗口尺寸/位置状态。跟随便携数据目录, 而不是 Tauri 的 app config
    /// 目录 —— 后者会让状态在整份拷贝移动到别的机器后错位。
    pub window_state: PathBuf,
    /// Set the first time data lands outside the executable's directory because
    /// that directory was read-only.
    pub relocated_notice: bool,
}

pub fn resolve() -> anyhow::Result<Paths> {
    let (data_dir, relocated_notice) = data_dir()?;
    let storage = data_dir.join("storage");
    let assets = storage.join("assets");
    fs::create_dir_all(&assets)
        .with_context(|| format!("创建数据目录失败: {}", assets.display()))?;

    Ok(Paths {
        db: data_dir.join("reader.db"),
        window_state: data_dir.join("window-state.json"),
        data_dir,
        storage,
        assets,
        relocated_notice,
    })
}

/// Returns the data directory and whether the user should be told it moved.
fn data_dir() -> anyhow::Result<(PathBuf, bool)> {
    if cfg!(debug_assertions) {
        return Ok((dev_root().join(".dev-data"), false));
    }

    let portable = exe_dir()?.join("data");
    if is_writable(&portable) {
        cleanup_legacy_fallback_data(&portable);
        return Ok((portable, false));
    }

    let fallback = fallback_data_dir()?.join("reader").join("data");
    let first_time = !fallback.exists();
    Ok((fallback, first_time))
}

/// 早期版本在安装目录不可写时退回到 `%LOCALAPPDATA%\reader\data`。现在只要
/// 便携目录恢复可写，就把这份旧回退数据收回来，避免两个目录各自保存一份书库。
///
/// 迁移规则必须保守：
/// - 先把旧目录**独有的**文件合并进当前目录，同名文件一律保留当前版本；
/// - 合并失败就整体放弃，旧目录原样保留，下次启动重试；
/// - 只有确认旧目录里每个文件在当前目录都已有对应，才移除旧目录。
///
/// 这里绝不能"没有 `reader.db` 就整个删掉"：书库文件、导入的 EPUB 原件都可能
/// 只存在于旧目录里，数据库丢了不等于这些内容也该丢。
#[cfg(target_os = "windows")]
fn cleanup_legacy_fallback_data(current: &Path) {
    let Ok(local_app_data) = fallback_data_dir() else {
        return;
    };
    let legacy = local_app_data.join("reader").join("data");
    cleanup_legacy_data_dir(current, &legacy);
}

#[cfg(target_os = "windows")]
fn cleanup_legacy_data_dir(current: &Path, legacy: &Path) {
    if legacy == current || !legacy.is_dir() {
        return;
    }

    if copy_legacy_data_if_missing(legacy, current).is_err() {
        // 读/写失败时保留旧目录，不要用"删除"来掩盖失败。
        return;
    }

    if !legacy_is_redundant(legacy, current, current.join("reader.db").is_file()) {
        eprintln!(
            "阅读：旧数据目录仍有未能合并的内容，已原样保留：{}",
            legacy.display()
        );
        return;
    }

    let _ = fs::remove_dir_all(legacy);
    remove_empty_legacy_root(legacy);
}

#[cfg(not(target_os = "windows"))]
fn cleanup_legacy_fallback_data(_current: &Path) {}

#[cfg(target_os = "windows")]
fn copy_legacy_data_if_missing(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            // 目标目录已存在也必须继续下钻：早先在这里直接 `continue` 会跳过整棵
            // 子树，旧目录里独有的文件随后随目录删除一起消失。
            copy_legacy_data_if_missing(&entry.path(), &target)?;
        } else if file_type.is_file() {
            if target.exists() {
                continue;
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 旧目录里的每个文件是否都能在当前目录找到**同一份内容**。
///
/// 判据必须是内容而不是名字 + 长度：`storage/assets` 下放的是导入原件的原始
/// 文件名（不按内容寻址），同名等长但内容不同的文件完全可能，只比长度会把
/// "旧目录里唯一的那份原件"误判成冗余，随后随目录删除。
///
/// 唯一放宽的是**顶层数据库文件**：`reader.db` 的取舍规则是"当前库优先"，
/// 两边都有库时旧库本来就要被丢弃，内容不同属预期，不能因此把整个旧目录留下。
/// 那只适用于顶层，子目录里的同名文件仍须内容一致。
///
/// 文件数过多时会逐字节比较, 这是启动路径上的一次性成本; 迁移完成后旧目录
/// 被移除, 后续启动不再付出这个代价。
#[cfg(target_os = "windows")]
fn legacy_is_redundant(legacy: &Path, current: &Path, current_database_wins: bool) -> bool {
    let Ok(entries) = fs::read_dir(legacy) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            return false;
        };
        let name = entry.file_name();
        let target = current.join(&name);
        if file_type.is_dir() {
            if !target.is_dir() || !legacy_is_redundant(&entry.path(), &target, false) {
                return false;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if current_database_wins && is_database_file(&name) {
            // 当前库优先: 旧库只要确实被当前库(或迁移标记)取代即可丢弃。
            // SQLite 的 `-wal` / `-shm` 在新库 checkpoint 后可能不存在, 属正常。
            continue;
        }
        if !files_have_same_content(&entry.path(), &target) {
            return false;
        }
    }
    true
}

/// 两个文件内容是否完全一致(长度不同时直接判否, 避免无谓读取)。
#[cfg(target_os = "windows")]
fn files_have_same_content(left: &Path, right: &Path) -> bool {
    let Ok(left_meta) = fs::metadata(left) else {
        return false;
    };
    let Ok(right_meta) = fs::metadata(right) else {
        return false;
    };
    if !right_meta.is_file() || left_meta.len() != right_meta.len() {
        return false;
    }
    let (Ok(left_bytes), Ok(right_bytes)) = (fs::read(left), fs::read(right)) else {
        return false;
    };
    left_bytes == right_bytes
}

/// SQLite 会在数据库旁边留下 `-wal` / `-shm`, 三者同属"当前库优先"的范围。
#[cfg(target_os = "windows")]
fn is_database_file(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    matches!(name, "reader.db" | "reader.db-wal" | "reader.db-shm")
}

#[cfg(target_os = "windows")]
fn remove_empty_legacy_root(legacy: &Path) {
    if let Some(root) = legacy.parent() {
        if root.read_dir().map(|mut entries| entries.next().is_none()).unwrap_or(false) {
            let _ = fs::remove_dir(root);
        }
    }
}

fn is_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let Ok(mut file) = fs::File::create(&probe) else {
        return false;
    };
    let ok = file.write_all(b"ok").is_ok();
    drop(file);
    let _ = fs::remove_file(&probe);
    ok
}

fn exe_dir() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("无法定位可执行文件路径")?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("可执行文件没有上级目录"))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn fallback_data_dir() -> anyhow::Result<PathBuf> {
    env_path("LOCALAPPDATA")
        .or_else(|| env_path("USERPROFILE").map(|path| path.join("AppData").join("Local")))
        .or_else(|| env_path("APPDATA"))
        .ok_or_else(|| anyhow!("无法定位 Windows 用户数据目录"))
}

#[cfg(target_os = "macos")]
fn fallback_data_dir() -> anyhow::Result<PathBuf> {
    env_path("HOME")
        .map(|path| path.join("Library").join("Application Support"))
        .ok_or_else(|| anyhow!("无法定位 macOS 用户数据目录"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn fallback_data_dir() -> anyhow::Result<PathBuf> {
    if let Some(path) = env_path("XDG_DATA_HOME").filter(|path| path.is_absolute()) {
        return Ok(path);
    }
    env_path("HOME")
        .map(|path| path.join(".local").join("share"))
        .ok_or_else(|| anyhow!("无法定位 Linux 用户数据目录"))
}

/// `desktop/` in the source tree. Only meaningful for debug builds.
fn dev_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("reader-portable-{name}-{nonce}"))
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn legacy_fallback_is_removed_when_current_database_exists() {
        let root = temp_dir("current-wins");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&current.join("reader.db"), "current");
        write_file(&current.join("storage").join("book.txt"), "legacy-storage");
        write_file(&legacy.join("reader.db"), "legacy");
        write_file(&legacy.join("storage").join("book.txt"), "legacy-storage");

        cleanup_legacy_data_dir(&current, &legacy);

        // 当前库优先, 不被旧数据覆盖。
        assert_eq!(fs::read_to_string(current.join("reader.db")).unwrap(), "current");
        // 旧目录里的每个文件都已在当前目录有同名同大小的一份, 才允许移除。
        assert!(!legacy.exists());
        assert!(!root.join("reader").exists());
        let _ = fs::remove_dir_all(root);
    }

    /// 旧目录里**独有**的文件必须先合并到当前目录, 不能被"当前库优先"连带删掉。
    #[test]
    fn legacy_only_files_are_merged_before_removal() {
        let root = temp_dir("legacy-only-merge");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&current.join("reader.db"), "current");
        write_file(&legacy.join("reader.db"), "legacy");
        write_file(&legacy.join("storage").join("assets").join("book.epub"), "legacy-epub");

        cleanup_legacy_data_dir(&current, &legacy);

        assert_eq!(fs::read_to_string(current.join("reader.db")).unwrap(), "current");
        assert_eq!(
            fs::read_to_string(current.join("storage").join("assets").join("book.epub")).unwrap(),
            "legacy-epub",
            "旧目录独有的书库文件必须搬过来",
        );
        assert!(!legacy.exists(), "合并完成后旧目录才可移除");
        let _ = fs::remove_dir_all(root);
    }

    /// 旧目录完全没有数据库(或为 0 字节)时也必须保留其中的书库内容。
    #[test]
    fn legacy_without_database_keeps_its_storage() {
        let root = temp_dir("no-db");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&current.join("reader.db"), "current");
        write_file(&legacy.join("storage").join("assets").join("book.epub"), "legacy-epub");

        cleanup_legacy_data_dir(&current, &legacy);

        assert_eq!(
            fs::read_to_string(current.join("storage").join("assets").join("book.epub")).unwrap(),
            "legacy-epub",
            "没有数据库不代表书库内容可以丢",
        );
        assert!(!legacy.exists());
        let _ = fs::remove_dir_all(root);
    }

    /// 合并失败时宁可保留旧目录, 也不能删掉唯一的一份数据。
    #[test]
    fn legacy_is_kept_when_merge_leaves_unique_files() {
        let root = temp_dir("merge-conflict");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&current.join("reader.db"), "current");
        // 同名但内容不同: 保留当前版本, 旧目录不可判为"已冗余"。
        write_file(&current.join("storage").join("book.txt"), "current-storage");
        write_file(&legacy.join("storage").join("book.txt"), "legacy-storage");

        cleanup_legacy_data_dir(&current, &legacy);

        assert_eq!(
            fs::read_to_string(current.join("storage").join("book.txt")).unwrap(),
            "current-storage",
            "同名文件必须保留当前版本",
        );
        assert!(legacy.exists(), "旧目录仍有独有内容时必须保留");
        let _ = fs::remove_dir_all(root);
    }

    /// 同名且**长度相同**但内容不同, 同样不能被判成冗余。
    ///
    /// `storage/assets` 下是导入原件的原始文件名(不按内容寻址), 只比名字 + 长度
    /// 会把旧目录里唯一的那份原件当成"已有", 随后随目录删除一起丢掉。
    #[test]
    fn legacy_keeps_same_length_file_with_different_content() {
        let root = temp_dir("same-length");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&current.join("reader.db"), "current");
        // 两边都是 7 字节, 内容不同。
        write_file(&current.join("storage").join("book.txt"), "current");
        write_file(&legacy.join("storage").join("book.txt"), "legacy!");

        cleanup_legacy_data_dir(&current, &legacy);

        assert_eq!(
            fs::read_to_string(current.join("storage").join("book.txt")).unwrap(),
            "current",
            "同名文件必须保留当前版本",
        );
        assert!(
            legacy.exists(),
            "旧目录里等长但不同的文件仍属独有内容, 不能删掉旧目录",
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 旧库的 `-wal` / `-shm` 在当前库 checkpoint 后不存在属正常,
    /// 不能因此把整个旧目录永久留下(每次启动重复扫描)。
    #[test]
    fn legacy_database_sidecar_files_do_not_block_cleanup() {
        let root = temp_dir("db-sidecar");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&current.join("reader.db"), "current");
        write_file(&legacy.join("reader.db"), "legacy");
        write_file(&legacy.join("reader.db-wal"), "wal");
        write_file(&legacy.join("reader.db-shm"), "shm");

        cleanup_legacy_data_dir(&current, &legacy);

        assert_eq!(fs::read_to_string(current.join("reader.db")).unwrap(), "current");
        assert!(!legacy.exists(), "数据库副文件缺失不应阻塞旧目录清理");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_fallback_is_migrated_when_current_database_is_missing() {
        let root = temp_dir("legacy-wins");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&legacy.join("reader.db"), "legacy-db");
        write_file(&legacy.join("storage").join("book.txt"), "legacy-storage");

        cleanup_legacy_data_dir(&current, &legacy);

        assert_eq!(fs::read_to_string(current.join("reader.db")).unwrap(), "legacy-db");
        assert_eq!(
            fs::read_to_string(current.join("storage").join("book.txt")).unwrap(),
            "legacy-storage",
        );
        assert!(!legacy.exists());
        assert!(!root.join("reader").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn empty_legacy_fallback_is_removed_without_touching_current() {
        let root = temp_dir("empty-legacy");
        let current = root.join("current").join("data");
        let legacy = root.join("reader").join("data");
        write_file(&current.join("reader.db"), "current");
        fs::create_dir_all(&legacy).unwrap();
        write_file(&legacy.join("window-state.json"), "{}");

        cleanup_legacy_data_dir(&current, &legacy);

        assert_eq!(fs::read_to_string(current.join("reader.db")).unwrap(), "current");
        // 旧目录里的 window-state.json 会先被搬到当前目录, 随后才移除旧目录。
        assert_eq!(
            fs::read_to_string(current.join("window-state.json")).unwrap(),
            "{}",
        );
        assert!(!legacy.exists());
        assert!(!root.join("reader").exists());
        let _ = fs::remove_dir_all(root);
    }
}
