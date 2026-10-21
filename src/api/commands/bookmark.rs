use crate::api::AppState;
use serde_json::Value;

use crate::error::error::{ApiResponse, AppError};
use crate::model::bookmark::Bookmark;

/// `get_bookmarks` 的共享实现 (Tauri command 与 Web HTTP handler 共用)。
pub async fn get_bookmarks_core(state: &AppState) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let list = read_list::<Bookmark>(state, &user_ns, "bookmark.json").await?;
    Ok(ApiResponse::ok(
        serde_json::to_value(list).unwrap_or_default(),
    ))
}

/// `get_bookmarks` 的 Tauri 入口: 只是 core 的薄壳。
#[tauri::command]
pub async fn get_bookmarks(
    state: tauri::State<'_, AppState>,
) -> Result<ApiResponse<Value>, AppError> {
    get_bookmarks_core(&state).await
}

/// `save_bookmark` 的共享实现。
///
/// 书签是阅读页的日常操作(划词/工具栏都要用), Web 端必须有对应路由 ——
/// 否则浏览器里点「加入书签」只会拿到一个 404。
pub async fn save_bookmark_core(
    state: &AppState,
    req: Bookmark,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    if req.book_name.is_empty() && req.book_author.is_empty() {
        return Err(AppError::BadRequest("书籍信息错误".to_string()));
    }
    let mut list = read_list::<Bookmark>(state, &user_ns, "bookmark.json").await?;
    upsert_by_key(&mut list, req, |b| {
        format!("{}_{}", b.book_name, b.book_author)
    });
    write_list(state, &user_ns, "bookmark.json", &list).await?;
    Ok(ApiResponse::ok(Value::String("".to_string())))
}

#[tauri::command]
pub async fn save_bookmark(
    state: tauri::State<'_, AppState>,
    req: Bookmark,
) -> Result<ApiResponse<Value>, AppError> {
    save_bookmark_core(&state, req).await
}

/// `save_bookmarks` 的共享实现 (批量 upsert)。
pub async fn save_bookmarks_core(
    state: &AppState,
    mut req: Vec<Bookmark>,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let mut list = read_list::<Bookmark>(state, &user_ns, "bookmark.json").await?;
    req.retain(|b| !(b.book_name.is_empty() && b.book_author.is_empty()));
    for b in req {
        upsert_by_key(&mut list, b, |v| {
            format!("{}_{}", v.book_name, v.book_author)
        });
    }
    write_list(state, &user_ns, "bookmark.json", &list).await?;
    Ok(ApiResponse::ok(Value::String("".to_string())))
}

#[tauri::command]
pub async fn save_bookmarks(
    state: tauri::State<'_, AppState>,
    req: Vec<Bookmark>,
) -> Result<ApiResponse<Value>, AppError> {
    save_bookmarks_core(&state, req).await
}

/// `delete_bookmark` 的共享实现。
pub async fn delete_bookmark_core(
    state: &AppState,
    req: Bookmark,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let mut list = read_list::<Bookmark>(state, &user_ns, "bookmark.json").await?;
    list.retain(|b| !(b.book_name == req.book_name && b.book_author == req.book_author));
    write_list(state, &user_ns, "bookmark.json", &list).await?;
    Ok(ApiResponse::ok(Value::String("".to_string())))
}

#[tauri::command]
pub async fn delete_bookmark(
    state: tauri::State<'_, AppState>,
    req: Bookmark,
) -> Result<ApiResponse<Value>, AppError> {
    delete_bookmark_core(&state, req).await
}

/// `delete_bookmarks` 的共享实现 (批量)。
pub async fn delete_bookmarks_core(
    state: &AppState,
    req: Vec<Bookmark>,
) -> Result<ApiResponse<Value>, AppError> {
    let user_ns = "default";
    let mut list = read_list::<Bookmark>(state, &user_ns, "bookmark.json").await?;
    for b in req {
        list.retain(|v| !(v.book_name == b.book_name && v.book_author == b.book_author));
    }
    write_list(state, &user_ns, "bookmark.json", &list).await?;
    Ok(ApiResponse::ok(Value::String("".to_string())))
}

#[tauri::command]
pub async fn delete_bookmarks(
    state: tauri::State<'_, AppState>,
    req: Vec<Bookmark>,
) -> Result<ApiResponse<Value>, AppError> {
    delete_bookmarks_core(&state, req).await
}

async fn read_list<T: for<'de> serde::Deserialize<'de>>(
    state: &AppState,
    user_ns: &str,
    name: &str,
) -> Result<Vec<T>, AppError> {
    state.json_document_service.read_list(user_ns, name).await
}

async fn write_list<T: serde::Serialize>(
    state: &AppState,
    user_ns: &str,
    name: &str,
    list: &Vec<T>,
) -> Result<(), AppError> {
    state
        .json_document_service
        .write_list(user_ns, name, list)
        .await
}

fn upsert_by_key<T, F>(list: &mut Vec<T>, item: T, key_fn: F)
where
    F: Fn(&T) -> String,
{
    let key = key_fn(&item);
    if let Some(pos) = list.iter().position(|v| key_fn(v) == key) {
        list[pos] = item;
    } else {
        list.push(item);
    }
}
