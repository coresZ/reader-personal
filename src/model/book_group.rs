use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookGroup {
    #[serde(default)]
    pub group_id: i64,
    pub group_name: String,
    #[serde(default)]
    pub order_no: i32,
    /// 该分组使用的书架排序方式; 缺省(= None)表示跟随全局设置。
    ///
    /// 与 Legado 的 `BookGroup.bookSort` 同义(-1/缺省 = 跟随全局):
    /// 0 最近阅读 / 1 更新时间 / 2 书名 / 3 手动排序 / 4 综合 / 5 作者。
    /// 备份往返必须保留, 否则从 Legado 恢复后分组级排序偏好会丢失。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub book_sort: Option<i32>,
}
