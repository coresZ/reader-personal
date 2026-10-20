use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::error::AppError;
use crate::model::book::Book;
use crate::model::book_group::BookGroup;
use crate::service::json_document_service::JsonDocumentService;

pub struct BookGroupService {
    docs: Arc<JsonDocumentService>,
    /// 串行化所有「读列表 → 改 → 写回」的操作。
    ///
    /// save_group / delete_group / delete_groups 都是读改写, 而 set_raw 只是
    /// INSERT ... ON CONFLICT DO UPDATE —— 它保证单条写入原子, 但不保证读改写原子。
    /// 两个并发请求会各自读到同一份旧列表再先后写回, 后者覆盖前者:
    /// A 读 [1,2,3]、B 读 [1,2,3]、A 写 [2,3]、B 写 [1,3] → 分组 1 复活。
    /// 备份恢复曾用 Promise.all 并发逐个删分组, 正好撞上这条路径, 表现为
    /// 「恢复后旧分组残留」。BookService 早有 bookshelf_write_lock 保护同一类操作,
    /// 这里补上同一约定。
    write_lock: Arc<Mutex<()>>,
}

impl BookGroupService {
    pub fn new(docs: Arc<JsonDocumentService>) -> Self {
        Self {
            docs,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn get_groups(&self, user_ns: &str) -> Result<Vec<BookGroup>, AppError> {
        self.docs.read_list(user_ns, "book_groups.json").await
    }

    /// 整表覆盖。调用方已完成读改写, 所以这里只需排他写 —— 但**仍然持锁**,
    /// 否则一次整表覆盖可能与另一个读改写交错。
    pub async fn save_groups(
        &self,
        user_ns: &str,
        groups: &Vec<BookGroup>,
    ) -> Result<(), AppError> {
        let _guard = self.write_lock.lock().await;
        self.docs
            .write_list(user_ns, "book_groups.json", groups)
            .await
    }

    pub async fn save_group(&self, user_ns: &str, mut group: BookGroup) -> Result<(), AppError> {
        let _guard = self.write_lock.lock().await;
        let mut groups = self.get_groups(user_ns).await?;
        if group.group_id == 0 {
            group.group_id = Self::next_free_group_id(&groups)?;
        }
        let mut found = false;
        for g in &mut groups {
            if g.group_id == group.group_id {
                g.group_name = group.group_name.clone();
                g.order_no = group.order_no;
                // 改名等只关心名字的调用方不传 book_sort(None), 此时保留原值;
                // 想显式重置为「跟随全局」的前端传 -1(Legado 同义)。
                if group.book_sort.is_some() {
                    g.book_sort = group.book_sort;
                }
                found = true;
                break;
            }
        }
        if !found {
            groups.push(group);
        }
        self.write_groups_locked(user_ns, &groups).await
    }

    pub async fn delete_group(&self, user_ns: &str, group_id: i64) -> Result<(), AppError> {
        let _guard = self.write_lock.lock().await;
        let mut groups = self.get_groups(user_ns).await?;
        groups.retain(|g| g.group_id != group_id);
        self.write_groups_locked(user_ns, &groups).await
    }

    /// 一次删除多个分组。
    ///
    /// 只读一次、只写一次 —— 逐个调用 `delete_group` 会对同一份列表反复读改写,
    /// 删 N 个分组就是 N 次全量重写, 其间失败还会留下删了一半的状态。
    /// 返回实际删掉的条数。
    pub async fn delete_groups(
        &self,
        user_ns: &str,
        group_ids: &[i64],
    ) -> Result<usize, AppError> {
        if group_ids.is_empty() {
            return Ok(0);
        }
        // 整个读改写过程持锁: 并发调用时若各自读到同一份旧列表, 后写的会覆盖先写的。
        let _guard = self.write_lock.lock().await;
        let mut groups = self.get_groups(user_ns).await?;
        let before = groups.len();
        groups.retain(|g| !group_ids.contains(&g.group_id));
        let removed = before - groups.len();
        // 一个都没匹配上时不必重写文件。
        if removed > 0 {
            // 注意用 _locked 版本: save_groups 也拿同一把锁, 在持锁状态下调它会死锁。
            self.write_groups_locked(user_ns, &groups).await?;
        }
        Ok(removed)
    }

    /// 落盘分组列表。**要求调用方已持有 write_lock** —— 读改写方法持锁后必须走
    /// 这个入口, 不能再调 save_groups(它会重复加锁而死锁)。
    async fn write_groups_locked(
        &self,
        user_ns: &str,
        groups: &Vec<BookGroup>,
    ) -> Result<(), AppError> {
        self.docs
            .write_list(user_ns, "book_groups.json", groups)
            .await
    }

    /// 为新分组挑一个「最小未占用的 2 的幂」。
    ///
    /// 书的 `group` 字段是位域(Legado 约定), 筛选用 `book.group & groupId` 判定
    /// 归属 —— 分组 id 必须是单个位(1,2,4,8…), 否则会与别的分组互相串扰:
    /// 旧实现用 `max_id + 1`, 建到第 3 个分组时 id=3(0b11), 一本书加入它后
    /// 会同时命中分组 1、2、3(issue #9: 书同时出现在道藏、佛藏下)。
    ///
    /// 负数是 Legado 的保留/虚拟分组(IdAll=-1 等), 不参与位域, 分配时跳过它们。
    /// 非正数全部 OR 进占用掩码 —— 非 2 幂旧 id(如 3)的每一个位都视为被占用,
    /// 不能只比数值大小: id=3 占的是位 1、2, 候选 1、2 都撞它, 必须从 4 起。
    ///
    /// 上限对齐前端的 32 位位运算(JS 的 `&`/`|` 是 ToInt32 语义): 最高位
    /// 1<<30, 即最多 31 个真实分组, 超出报错而不是静默回绕。
    fn next_free_group_id(groups: &[BookGroup]) -> Result<i64, AppError> {
        let mut occupied: i64 = 0;
        for g in groups {
            if g.group_id > 0 {
                occupied |= g.group_id;
            }
        }
        let mut candidate: i64 = 1;
        loop {
            if candidate > (1i64 << 30) {
                return Err(AppError::BadRequest(
                    "分组数量已达上限(31), 无法再创建新分组".to_string(),
                ));
            }
            if (occupied & candidate) == 0 {
                return Ok(candidate);
            }
            candidate <<= 1;
        }
    }

    /// 一次性修复存量脏数据(幂等, 启动时调用)。
    ///
    /// 旧版给新分组分配 `max_id + 1`, 产生了非 2 幂的 id(如 3、5、6), 而
    /// 书的 `group` 是位域 —— 这些 id 一旦被某本书引用, 书就会"同时出现在
    /// 多个分组"里(issue #9)。本迁移做两件事:
    ///
    /// 1. 书架里 group **精确等于**旧 id 的书换到新 id(先落盘);
    /// 2. 给每个非 2 幂的正数分组 id 换一个空闲的 2 幂新 id(名字/顺序保持,
    ///    后落盘 —— 顺序保证中断后重跑可自愈, 见函数内注释)。
    ///
    /// 负数分组(Legado 保留 id)与本来就是 2 幂的 id 原样保留。已是干净数据时
    /// 不写任何文件(幂等, 重跑无副作用)。
    ///
    /// `read_books` / `write_books` 由调用方注入(bootstrap 里是 BookService 的
    /// 读/写书架), 避免服务间循环依赖。返回是否有迁移发生, 供日志记录。
    pub async fn migrate_non_power_of_two_group_ids<F, R, W, R2>(
        &self,
        user_ns: &str,
        read_books: F,
        write_books: W,
    ) -> Result<bool, AppError>
    where
        F: FnOnce() -> R,
        R: std::future::Future<Output = Result<Vec<Book>, AppError>>,
        W: FnOnce(Vec<Book>) -> R2,
        R2: std::future::Future<Output = Result<(), AppError>>,
    {
        let _guard = self.write_lock.lock().await;
        let groups = self.get_groups(user_ns).await?;

        // 旧 id → 新 id 的映射表。只收「正数且非 2 幂」的。
        let mut remap: std::collections::BTreeMap<i64, i64> = Default::default();
        for g in &groups {
            let id = g.group_id;
            if id > 0 && (id & (id - 1)) != 0 {
                remap.insert(id, 0); // 值稍后填
            }
        }
        if remap.is_empty() {
            return Ok(false); // 干净数据, 不动任何文件
        }

        // 逐个分配新 id: 临时列表 = 保留的 2 幂 id + 已分配的新 id, 复用
        // next_free_group_id 的空位查找。BTreeMap 按 key 升序, 分配结果稳定可测。
        let mut assigned: Vec<(i64, i64)> = Vec::new();
        for old_id in remap.keys().copied().collect::<Vec<_>>() {
            // 候选集 = 所有不参与换 id 的分组(负数、2 幂、以及不在 remap 里的)
            // + 已分配出去的新 id(占位用, 防止两个旧 id 换到同一个新 id)。
            let mut temp: Vec<BookGroup> = groups
                .iter()
                .filter(|g| !remap.contains_key(&g.group_id))
                .cloned()
                .collect();
            // 已分配出去的新 id 也要占位, 防止两个旧 id 换到同一个新 id。
            temp.extend(
                assigned
                    .iter()
                    .map(|(_, new_id)| BookGroup {
                        group_id: *new_id,
                        group_name: String::new(),
                        order_no: 0,
                        book_sort: None,
                    })
                    .collect::<Vec<_>>(),
            );
            let new_id = Self::next_free_group_id(&temp)?;
            remap.insert(old_id, new_id);
            assigned.push((old_id, new_id));
        }

        // 1) 先重写书架: group 精确等于旧 id 的书换到新 id。
        //
        //    判定用**精确相等**而不是位与: 后端 save_book_group_id 是赋值语义,
        //    本应用正常路径下书的 group 恰好等于最后一次移入的分组 id(单值)。
        //    位与会把「只属于分组 1 的书」(group=1) 误判成属于旧 id 3(0b11),
        //    把它错误地迁走。位域叠加值(如 Legado 导入的 1|2=3)恰好撞上非 2
        //    幂 id 时无法与显式移入区分 —— 按显式移入处理(迁移), 丢掉的
        //    1、2 归属可在分组编辑器里重新勾选, 属已知取舍; 不撞上的叠加值
        //    (如 7)原样保留。
        //
        //    写入顺序是「先书架、后分组列表」: 若两步之间进程中断, 分组列表
        //    仍是旧 id → 下次启动迁移重跑(新 id 分配确定性: 同样的 groups 产生
        //    同样的 remap), 书架已等于新 id、不再匹配旧 id → 只重写分组列表,
        //    完全自愈。反过来(先分组后书架)中断会留下「分组干净、书架脏」的
        //    状态, remap 为空导致永不修复。
        let books = read_books().await?;
        let mut changed = false;
        let new_books: Vec<Book> = books
            .into_iter()
            .map(|mut book| {
                let cur = book.group.unwrap_or(0);
                if let Some((_, new_id)) = assigned.iter().find(|(old, _)| *old == cur) {
                    book.group = Some(*new_id);
                    changed = true;
                }
                book
            })
            .collect();
        if changed {
            write_books(new_books).await?;
        }

        // 2) 再重写分组列表: 旧 id 换新 id, 名字/顺序保持。
        let new_groups: Vec<BookGroup> = groups
            .iter()
            .map(|g| {
                match remap.get(&g.group_id) {
                    Some(new_id) => BookGroup {
                        group_id: *new_id,
                        group_name: g.group_name.clone(),
                        order_no: g.order_no,
                        book_sort: g.book_sort,
                    },
                    None => g.clone(),
                }
            })
            .collect();
        self.write_groups_locked(user_ns, &new_groups).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 测试夹具专用的依赖放在测试模块里: 放在文件顶部会让 `cargo build` 报
    // unused import(非测试构建不编译这个模块)。
    use std::path::PathBuf;
    use crate::storage::db;

    async fn create_service() -> (BookGroupService, PathBuf) {
        // 每个测试必须用**独立**目录: 同一进程内 process::id() 是一样的, 并行执行时
        // 会共用同一个 reader.db 与 book_groups.json, 互相覆盖导致随机失败。
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_dir = std::env::temp_dir().join(format!(
            "reader-rust-group-service-{}-{}",
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let database_url = format!("sqlite:{}?mode=rwc", temp_dir.join("reader.db").display());
        let pool = db::init_pool(&database_url).await.unwrap();
        let docs = Arc::new(JsonDocumentService::new(pool, &temp_dir.to_string_lossy()));
        (BookGroupService::new(docs), temp_dir)
    }

    fn group(id: i64, name: &str) -> BookGroup {
        BookGroup {
            group_id: id,
            group_name: name.to_string(),
            order_no: 0,
            book_sort: None,
        }
    }

    fn ids(list: &[BookGroup]) -> Vec<i64> {
        list.iter().map(|g| g.group_id).collect()
    }

    #[tokio::test]
    async fn deletes_every_requested_group_in_one_pass() {
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(1, "玄幻"), group(2, "都市"), group(3, "历史")])
            .await
            .unwrap();

        let removed = service.delete_groups("default", &[1, 3]).await.unwrap();

        assert_eq!(removed, 2);
        assert_eq!(ids(&service.get_groups("default").await.unwrap()), vec![2]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn reports_only_ids_that_actually_existed() {
        // 计数必须反映真实删除数, 否则前端提示会虚报。
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(1, "玄幻")])
            .await
            .unwrap();

        let removed = service
            .delete_groups("default", &[1, 99, 100])
            .await
            .unwrap();

        assert_eq!(removed, 1);
        assert!(service.get_groups("default").await.unwrap().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn empty_selection_is_a_noop() {
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(1, "玄幻")])
            .await
            .unwrap();

        assert_eq!(service.delete_groups("default", &[]).await.unwrap(), 0);
        assert_eq!(ids(&service.get_groups("default").await.unwrap()), vec![1]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn deleting_all_groups_leaves_an_empty_list() {
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(1, "玄幻"), group(2, "都市")])
            .await
            .unwrap();

        assert_eq!(service.delete_groups("default", &[1, 2]).await.unwrap(), 2);
        assert!(service.get_groups("default").await.unwrap().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn keeps_legado_negative_ids_distinguishable() {
        // Legado 用负数作保留分组(IdLocal=-2 等)。批量删除必须能按原值精确匹配,
        // 不能因为「负数看起来像无效值」就被顺手清掉。
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(-2, "本地"), group(1, "玄幻"), group(-1, "全部")])
            .await
            .unwrap();

        let removed = service.delete_groups("default", &[-2]).await.unwrap();

        assert_eq!(removed, 1);
        assert_eq!(
            ids(&service.get_groups("default").await.unwrap()),
            vec![1, -1]
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn concurrent_group_deletions_do_not_resurrect_removed_groups() {
        // 回归测试: 读改写无锁时, 两个并发删除会各自读到同一份旧列表再先后写回,
        // 后写的覆盖先写的 —— 被先删掉的分组「复活」。
        //   A 读 [1,2,3]、B 读 [1,2,3]、A 写 [2,3]、B 写 [1,3]  → 分组 1 又回来了
        // 备份恢复曾用 Promise.all 并发逐个删分组, 正好撞上这条路径, 表现为
        // 「恢复后旧分组残留」。这里并发删 1 和 2, 断言两者都真的消失。
        let (service, dir) = create_service().await;
        service
            .save_groups(
                "default",
                &vec![group(1, "玄幻"), group(2, "都市"), group(3, "历史")],
            )
            .await
            .unwrap();

        // 交替并发删 1 与 2 多轮, 提高交错概率。
        for _ in 0..12 {
            service.save_groups(
                "default",
                &vec![group(1, "玄幻"), group(2, "都市"), group(3, "历史")],
            )
            .await
            .unwrap();
            let (a, b) = tokio::join!(
                service.delete_group("default", 1),
                service.delete_group("default", 2),
            );
            a.unwrap();
            b.unwrap();

            let left = ids(&service.get_groups("default").await.unwrap());
            assert!(
                !left.contains(&1),
                "分组 1 被并发删除后复活了, 剩余: {left:?}"
            );
            assert!(
                !left.contains(&2),
                "分组 2 被并发删除后复活了, 剩余: {left:?}"
            );
            assert_eq!(left, vec![3], "只应剩编号 3 的分组");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn bulk_and_single_deletion_mixed_concurrently_stay_consistent() {
        // 另一条交错路径: 批量删 与 单个删 同时发出。
        let (service, dir) = create_service().await;
        for _ in 0..12 {
            service
                .save_groups(
                    "default",
                    &vec![group(1, "A"), group(2, "B"), group(3, "C"), group(4, "D")],
                )
                .await
                .unwrap();
            let (bulk, single) = tokio::join!(
                service.delete_groups("default", &[1, 2]),
                service.delete_group("default", 3),
            );
            bulk.unwrap();
            single.unwrap();

            let left = ids(&service.get_groups("default").await.unwrap());
            assert_eq!(left, vec![4], "应只剩编号 4 的分组, 实际: {left:?}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    // ─── 新分组 id 必须是 2 的幂(issue #9) ───

    /// id=0 走自动分配, 返回「新建分组」应得的 id。
    async fn created_group_id(service: &BookGroupService, name: &str) -> i64 {
        service
            .save_group("default", group(0, name))
            .await
            .unwrap();
        service
            .get_groups("default")
            .await
            .unwrap()
            .into_iter()
            .find(|g| g.group_name == name)
            .unwrap()
            .group_id
    }

    #[tokio::test]
    async fn fresh_ids_are_powers_of_two() {
        // 旧实现是 max+1: 第 3 个分组拿到 3(0b11), 书加入后同时命中分组 1、2、3
        // —— 正是 issue #9「一本书出现在道藏、佛藏下」的根因。
        let (service, dir) = create_service().await;

        assert_eq!(created_group_id(&service, "道藏").await, 1);
        assert_eq!(created_group_id(&service, "佛藏").await, 2);
        assert_eq!(created_group_id(&service, "第三个").await, 4);
        assert_eq!(created_group_id(&service, "第四个").await, 8);

        let all = ids(&service.get_groups("default").await.unwrap());
        assert_eq!(all, vec![1, 2, 4, 8]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn fresh_id_skips_bits_taken_by_legacy_non_power_ids() {
        // 存量脏数据(旧版 max+1 产生的 id=3)占住了位 1、2 —— 新分配必须绕开,
        // 从 4 开始, 否则又会与新分组的书互相串扰。
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(1, "a"), group(2, "b"), group(3, "脏数据")])
            .await
            .unwrap();

        assert_eq!(created_group_id(&service, "新建").await, 4);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn fresh_id_never_lands_on_bits_of_a_lone_non_power_id() {
        // 回归: 位占用看二进制位而非数值大小。id=3(0b11) 独占时, 候选 1、2
        // 都撞它的位 —— 只有按位判断才拿得到 4, 按数值比较会错拿 1。
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(3, "脏数据")])
            .await
            .unwrap();
        assert_eq!(created_group_id(&service, "新建A").await, 4);

        // [1, 3]: 位 1 被分组 1 与 3 共占, 位 2 被 3 占 —— 候选必须跳到 4。
        let (service2, dir2) = create_service().await;
        service2
            .save_groups("default", &vec![group(1, "a"), group(3, "脏数据")])
            .await
            .unwrap();
        assert_eq!(created_group_id(&service2, "新建B").await, 4);
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(dir2).ok();
    }

    #[tokio::test]
    async fn fresh_id_ignores_legado_negative_ids() {
        // 负数是 Legado 保留分组, 不占位域, 不影响新 id 分配。
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(-2, "本地"), group(-1, "全部")])
            .await
            .unwrap();

        assert_eq!(created_group_id(&service, "新建").await, 1);
        std::fs::remove_dir_all(dir).ok();
    }

    // ─── 存量迁移(issue #9 的数据修复) ───

    fn book_with_group(url: &str, group: i64) -> Book {
        Book {
            name: format!("书{url}"),
            author: "作者".to_string(),
            book_url: url.to_string(),
            origin: "local-txt".to_string(),
            group: Some(group),
            ..Book::default()
        }
    }

    #[tokio::test]
    async fn migration_rewrites_legacy_ids_and_book_bits() {
        // 复现 issue #9 的原始现场: 分组 id 是 1、2、3(旧版 max+1), 书被"移入
        // 分组 3"后 group=3 —— 位域上它同时属于 1、2、3。
        let (service, dir) = create_service().await;
        service
            .save_groups(
                "default",
                &vec![group(1, "道藏"), group(2, "佛藏"), group(3, "新建")],
            )
            .await
            .unwrap();
        let mut books = vec![
            book_with_group("book:a", 3), // 属于分组 3(以及意外的 1、2)
            book_with_group("book:b", 1), // 只属于分组 1
            book_with_group("book:c", 2), // 只属于分组 2
            book_with_group("book:d", 0), // 未分组
        ];
        // 再加一本完全没有 group 字段的书, 迁移不应把它变成「已分组」。
        books.push(Book {
            name: "无分组书".to_string(),
            author: "作者".to_string(),
            book_url: "book:e".to_string(),
            origin: "local-txt".to_string(),
            ..Book::default()
        });
        // 注入内存版书架: 用 Arc<Mutex> 让读写闭包共享同一份数据。
        let shelf = std::sync::Arc::new(tokio::sync::Mutex::new(books.clone()));
        let shelf_reader = shelf.clone();
        let shelf_writer = shelf.clone();
        let migrated = service
            .migrate_non_power_of_two_group_ids(
                "default",
                move || {
                    let shelf = shelf_reader.clone();
                    async move { Ok(shelf.lock().await.clone()) }
                },
                move |next: Vec<Book>| {
                    let shelf = shelf_writer.clone();
                    async move {
                        *shelf.lock().await = next;
                        Ok(())
                    }
                },
            )
            .await
            .unwrap();

        assert!(migrated, "存在非 2 幂 id 时必须执行迁移");
        // 分组列表: 3 → 4(空闲的最小 2 幂), 1、2 原样。
        let groups = service.get_groups("default").await.unwrap();
        assert_eq!(ids(&groups), vec![1, 2, 4]);
        assert_eq!(
            groups.iter().map(|g| g.group_name.clone()).collect::<Vec<_>>(),
            vec!["道藏", "佛藏", "新建"],
        );
        // 书架: group 精确等于旧 id(3)的书迁到 4; 位与命中的单值书(1、2)不动。
        // (位域叠加值若恰好撞上旧 id 会被迁移 —— 见函数注释里的已知取舍。)
        let shelf = shelf.lock().await;
        let by_url = |url: &str| {
            shelf
                .iter()
                .find(|b| b.book_url == url)
                .unwrap()
                .group
                .unwrap_or(0)
        };
        assert_eq!(by_url("book:a"), 4, "group=3(旧 id)的书应迁移到新 id 4");
        assert_eq!(by_url("book:b"), 1, "只属于分组 1 的书不应被误迁");
        assert_eq!(by_url("book:c"), 2, "只属于分组 2 的书不应被误迁");
        assert_eq!(by_url("book:d"), 0);
        assert_eq!(
            shelf
                .iter()
                .find(|b| b.book_url == "book:e")
                .unwrap()
                .group,
            None,
            "从未分组过的书不应被写入 group 字段"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migration_is_idempotent_and_skips_clean_data() {
        // 干净数据(全部 2 幂/负数): 不迁移、不写文件。
        let (service, dir) = create_service().await;
        service
            .save_groups(
                "default",
                &vec![group(1, "道藏"), group(2, "佛藏"), group(-1, "全部")],
            )
            .await
            .unwrap();

        let migrated = service
            .migrate_non_power_of_two_group_ids(
                "default",
                || async { Ok(Vec::new()) },
                |_: Vec<Book>| async { Ok(()) },
            )
            .await
            .unwrap();
        assert!(!migrated);
        assert_eq!(
            ids(&service.get_groups("default").await.unwrap()),
            vec![1, 2, -1]
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migration_runs_again_after_partial_run() {
        // 幂等性: 迁移一遍后再跑一遍, 分组与书都不再变化。
        let (service, dir) = create_service().await;
        service
            .save_groups(
                "default",
                &vec![group(1, "a"), group(2, "b"), group(3, "c"), group(6, "d")],
            )
            .await
            .unwrap();
        let shelf = std::sync::Arc::new(tokio::sync::Mutex::new(vec![book_with_group("book:x", 1 | 2 | 6)]));
        for _ in 0..2 {
            let shelf_reader = shelf.clone();
            let shelf_writer = shelf.clone();
            service
                .migrate_non_power_of_two_group_ids(
                    "default",
                    move || {
                        let shelf = shelf_reader.clone();
                        async move { Ok(shelf.lock().await.clone()) }
                    },
                    move |next: Vec<Book>| {
                        let shelf = shelf_writer.clone();
                        async move {
                            *shelf.lock().await = next;
                            Ok(())
                        }
                    },
                )
                .await
                .unwrap();
        }
        // 3 → 4, 6(0b110) → 8; 位域叠加值 7 不精确匹配任何旧 id, 原样保留。
        assert_eq!(ids(&service.get_groups("default").await.unwrap()), vec![1, 2, 4, 8]);
        assert_eq!(shelf.lock().await[0].group, Some(7));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migration_leaves_groups_untouched_when_bookshelf_write_fails() {
        // 落盘顺序是「先书架、后分组列表」: 书架写失败时函数提前返回错误,
        // 分组列表必须还是旧 id —— 否则就落入了「分组干净、书架脏」的
        // 不可自愈状态(remap 为空, 下次启动永不修复)。
        let (service, dir) = create_service().await;
        service
            .save_groups("default", &vec![group(1, "a"), group(3, "脏数据")])
            .await
            .unwrap();

        let result = service
            .migrate_non_power_of_two_group_ids(
                "default",
                || async { Ok(vec![book_with_group("book:f", 3)]) },
                |_: Vec<Book>| async {
                    Err(AppError::Internal(anyhow::anyhow!("disk full")))
                },
            )
            .await;

        assert!(result.is_err(), "书架写失败必须让迁移整体失败");
        // 分组列表未动: 仍是 [1, 3], 下次启动迁移可重试。
        assert_eq!(ids(&service.get_groups("default").await.unwrap()), vec![1, 3]);
        std::fs::remove_dir_all(dir).ok();
    }
}
