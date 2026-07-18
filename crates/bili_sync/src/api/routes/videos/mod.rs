use std::collections::HashSet;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{Extension, Path, Query};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bili_sync_entity::*;
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    QueryOrder, TransactionTrait, TryIntoModel,
};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use crate::api::error::InnerApiError;
use crate::api::helper::{update_page_download_status, update_video_download_status};
use crate::api::request::{
    ResetFilteredVideoStatusRequest, ResetVideoStatusRequest, UpdateFilteredVideoStatusRequest,
    UpdateVideoStatusRequest, VideosRequest,
};
use crate::api::response::{
    ClearAndResetVideoStatusResponse, PageInfo, ResetFilteredVideosResponse, ResetVideoResponse, SimplePageInfo,
    SimpleVideoInfo, UpdateFilteredVideoStatusResponse, UpdateVideoStatusResponse, VideoInfo, VideoResponse,
    VideosResponse,
};
use crate::api::wrapper::{ApiError, ApiResponse, ValidatedJson};
use crate::utils::status::{PageStatus, STATUS_OK, VideoStatus};

pub(super) fn router() -> Router {
    Router::new()
        .route("/videos", get(get_videos))
        .route("/videos/{id}", get(get_video))
        .route(
            "/videos/{id}/clear-and-reset-status",
            post(clear_and_reset_video_status),
        )
        .route("/videos/{id}/reset-status", post(reset_video_status))
        .route("/videos/{id}/update-status", post(update_video_status))
        .route("/videos/reset-status", post(reset_filtered_video_status))
        .route("/videos/update-status", post(update_filtered_video_status))
        .route("/videos/{video_id}/pages/{page_id}/stream", get(stream_page_video))
}

/// 列出视频的基本信息，支持根据视频来源筛选、名称查找和分页
pub async fn get_videos(
    Extension(db): Extension<DatabaseConnection>,
    Query(params): Query<VideosRequest>,
) -> Result<ApiResponse<VideosResponse>, ApiError> {
    let mut query = video::Entity::find();
    for (field, column) in [
        (params.collection, video::Column::CollectionId),
        (params.favorite, video::Column::FavoriteId),
        (params.submission, video::Column::SubmissionId),
        (params.watch_later, video::Column::WatchLaterId),
    ] {
        if let Some(id) = field {
            query = query.filter(column.eq(id));
        }
    }
    if let Some(query_word) = params.query {
        query = query.filter(
            video::Column::Name
                .contains(&query_word)
                .or(video::Column::Bvid.contains(query_word)),
        );
    }
    if let Some(status_filter) = params.status_filter {
        query = query.filter(status_filter.to_video_query());
    }
    if let Some(validation_filter) = params.validation_filter {
        query = query.filter(validation_filter.to_video_query());
    }
    let total_count = query.clone().count(&db).await?;
    let (page, page_size) = if let (Some(page), Some(page_size)) = (params.page, params.page_size) {
        (page, page_size)
    } else {
        (0, 10)
    };
    Ok(ApiResponse::ok(VideosResponse {
        videos: query
            .order_by_desc(video::Column::Id)
            .into_partial_model::<VideoInfo>()
            .paginate(&db, page_size)
            .fetch_page(page)
            .await?,
        total_count,
    }))
}

pub async fn get_video(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<VideoResponse>, ApiError> {
    let (video_info, pages_info) = tokio::try_join!(
        video::Entity::find_by_id(id).into_partial_model::<VideoInfo>().one(&db),
        page::Entity::find()
            .filter(page::Column::VideoId.eq(id))
            .order_by_asc(page::Column::Cid)
            .into_partial_model::<PageInfo>()
            .all(&db)
    )?;
    let Some(video_info) = video_info else {
        return Err(InnerApiError::NotFound(id).into());
    };
    Ok(ApiResponse::ok(VideoResponse {
        video: video_info,
        pages: pages_info,
    }))
}

pub async fn reset_video_status(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
    Json(request): Json<ResetVideoStatusRequest>,
) -> Result<ApiResponse<ResetVideoResponse>, ApiError> {
    let (video_info, pages_info) = tokio::try_join!(
        video::Entity::find_by_id(id).into_partial_model::<VideoInfo>().one(&db),
        page::Entity::find()
            .filter(page::Column::VideoId.eq(id))
            .order_by_asc(page::Column::Cid)
            .into_partial_model::<PageInfo>()
            .all(&db)
    )?;
    let Some(mut video_info) = video_info else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let resetted_pages_info = pages_info
        .into_iter()
        .filter_map(|mut page_info| {
            let mut page_status = PageStatus::from(page_info.download_status);
            if (request.force && page_status.force_reset_failed()) || page_status.reset_failed() {
                page_info.download_status = page_status.into();
                Some(page_info)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut video_status = VideoStatus::from(video_info.download_status);
    let mut video_resetted = (request.force && video_status.force_reset_failed()) || video_status.reset_failed();
    if !resetted_pages_info.is_empty() {
        video_status.set(4, 0); //  将“分页下载”重置为 0
        video_resetted = true;
    }
    let resetted_videos_info = if video_resetted {
        video_info.download_status = video_status.into();
        vec![&video_info]
    } else {
        vec![]
    };
    let resetted = !resetted_videos_info.is_empty() || !resetted_pages_info.is_empty();
    if resetted {
        let txn = db.begin().await?;
        if !resetted_videos_info.is_empty() {
            // 只可能有 1 个元素，所以不用 batch
            update_video_download_status::<VideoInfo>(&txn, &resetted_videos_info, None).await?;
        }
        if !resetted_pages_info.is_empty() {
            update_page_download_status(&txn, &resetted_pages_info, Some(500)).await?;
        }
        txn.commit().await?;
    }
    Ok(ApiResponse::ok(ResetVideoResponse {
        resetted,
        video: video_info,
        pages: resetted_pages_info,
    }))
}

pub async fn clear_and_reset_video_status(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
) -> Result<ApiResponse<ClearAndResetVideoStatusResponse>, ApiError> {
    let video_info = video::Entity::find_by_id(id).one(&db).await?;
    let Some(video_info) = video_info else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let txn = db.begin().await?;
    let mut video_info = video_info.into_active_model();
    video_info.single_page = Set(None);
    video_info.download_status = Set(0);
    video_info.valid = Set(true);
    let video_info = video_info.update(&txn).await?;
    page::Entity::delete_many()
        .filter(page::Column::VideoId.eq(id))
        .exec(&txn)
        .await?;
    txn.commit().await?;
    let video_info = video_info.try_into_model()?;
    let warning = if video_info.path.is_empty() {
        None
    } else {
        tokio::fs::remove_dir_all(&video_info.path)
            .await
            .context(format!("删除本地路径「{}」失败", video_info.path))
            .err()
            .map(|e| format!("{:#}", e))
    };
    Ok(ApiResponse::ok(ClearAndResetVideoStatusResponse {
        warning,
        video: VideoInfo {
            id: video_info.id,
            bvid: video_info.bvid,
            name: video_info.name,
            upper_name: video_info.upper_name,
            valid: video_info.valid,
            should_download: video_info.should_download,
            download_status: video_info.download_status,
            collection_id: video_info.collection_id,
            favorite_id: video_info.favorite_id,
            submission_id: video_info.submission_id,
            watch_later_id: video_info.watch_later_id,
        },
    }))
}

pub async fn reset_filtered_video_status(
    Extension(db): Extension<DatabaseConnection>,
    Json(request): Json<ResetFilteredVideoStatusRequest>,
) -> Result<ApiResponse<ResetFilteredVideosResponse>, ApiError> {
    let mut query = video::Entity::find();
    for (field, column) in [
        (request.collection, video::Column::CollectionId),
        (request.favorite, video::Column::FavoriteId),
        (request.submission, video::Column::SubmissionId),
        (request.watch_later, video::Column::WatchLaterId),
    ] {
        if let Some(id) = field {
            query = query.filter(column.eq(id));
        }
    }
    if let Some(query_word) = request.query {
        query = query.filter(
            video::Column::Name
                .contains(&query_word)
                .or(video::Column::Bvid.contains(query_word)),
        );
    }
    if let Some(status_filter) = request.status_filter {
        query = query.filter(status_filter.to_video_query());
    }
    if let Some(validation_filter) = request.validation_filter {
        query = query.filter(validation_filter.to_video_query());
    }
    let all_videos = query.into_partial_model::<SimpleVideoInfo>().all(&db).await?;
    let all_pages = page::Entity::find()
        .filter(page::Column::VideoId.is_in(all_videos.iter().map(|v| v.id)))
        .into_partial_model::<SimplePageInfo>()
        .all(&db)
        .await?;
    let resetted_pages_info = all_pages
        .into_iter()
        .filter_map(|mut page_info| {
            let mut page_status = PageStatus::from(page_info.download_status);
            if (request.force && page_status.force_reset_failed()) || page_status.reset_failed() {
                page_info.download_status = page_status.into();
                Some(page_info)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let video_ids_with_resetted_pages: HashSet<i32> = resetted_pages_info.iter().map(|page| page.video_id).collect();
    let resetted_videos_info = all_videos
        .into_iter()
        .filter_map(|mut video_info| {
            let mut video_status = VideoStatus::from(video_info.download_status);
            let mut video_resetted =
                (request.force && video_status.force_reset_failed()) || video_status.reset_failed();
            if video_ids_with_resetted_pages.contains(&video_info.id) {
                video_status.set(4, 0); // 将"分页下载"重置为 0
                video_resetted = true;
            }
            if video_resetted {
                video_info.download_status = video_status.into();
                Some(video_info)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let has_video_updates = !resetted_videos_info.is_empty();
    let has_page_updates = !resetted_pages_info.is_empty();
    if has_video_updates || has_page_updates {
        let txn = db.begin().await?;
        if has_video_updates {
            update_video_download_status(&txn, &resetted_videos_info, Some(500)).await?;
        }
        if has_page_updates {
            update_page_download_status(&txn, &resetted_pages_info, Some(500)).await?;
        }
        txn.commit().await?;
    }
    Ok(ApiResponse::ok(ResetFilteredVideosResponse {
        resetted: has_video_updates || has_page_updates,
        resetted_videos_count: resetted_videos_info.len(),
        resetted_pages_count: resetted_pages_info.len(),
    }))
}

pub async fn update_video_status(
    Path(id): Path<i32>,
    Extension(db): Extension<DatabaseConnection>,
    ValidatedJson(request): ValidatedJson<UpdateVideoStatusRequest>,
) -> Result<ApiResponse<UpdateVideoStatusResponse>, ApiError> {
    let (video_info, mut pages_info) = tokio::try_join!(
        video::Entity::find_by_id(id).into_partial_model::<VideoInfo>().one(&db),
        page::Entity::find()
            .filter(page::Column::VideoId.eq(id))
            .order_by_asc(page::Column::Cid)
            .into_partial_model::<PageInfo>()
            .all(&db)
    )?;
    let Some(mut video_info) = video_info else {
        return Err(InnerApiError::NotFound(id).into());
    };
    let mut video_status = VideoStatus::from(video_info.download_status);
    for update in &request.video_updates {
        video_status.set(update.status_index, update.status_value);
    }
    video_info.download_status = video_status.into();
    let mut updated_pages_info = Vec::new();
    let mut page_id_map = pages_info
        .iter_mut()
        .map(|page| (page.id, page))
        .collect::<std::collections::HashMap<_, _>>();
    for page_update in &request.page_updates {
        if let Some(page_info) = page_id_map.remove(&page_update.page_id) {
            let mut page_status = PageStatus::from(page_info.download_status);
            for update in &page_update.updates {
                page_status.set(update.status_index, update.status_value);
            }
            page_info.download_status = page_status.into();
            updated_pages_info.push(page_info);
        }
    }
    let has_video_updates = !request.video_updates.is_empty();
    let has_page_updates = !updated_pages_info.is_empty();
    if has_video_updates || has_page_updates {
        let txn = db.begin().await?;
        if has_video_updates {
            update_video_download_status::<VideoInfo>(&txn, &[&video_info], None).await?;
        }
        if has_page_updates {
            update_page_download_status::<PageInfo>(&txn, &updated_pages_info, None).await?;
        }
        txn.commit().await?;
    }
    Ok(ApiResponse::ok(UpdateVideoStatusResponse {
        success: has_video_updates || has_page_updates,
        video: video_info,
        pages: pages_info,
    }))
}

pub async fn update_filtered_video_status(
    Extension(db): Extension<DatabaseConnection>,
    ValidatedJson(request): ValidatedJson<UpdateFilteredVideoStatusRequest>,
) -> Result<ApiResponse<UpdateFilteredVideoStatusResponse>, ApiError> {
    let mut query = video::Entity::find();
    for (field, column) in [
        (request.collection, video::Column::CollectionId),
        (request.favorite, video::Column::FavoriteId),
        (request.submission, video::Column::SubmissionId),
        (request.watch_later, video::Column::WatchLaterId),
    ] {
        if let Some(id) = field {
            query = query.filter(column.eq(id));
        }
    }
    if let Some(query_word) = request.query {
        query = query.filter(
            video::Column::Name
                .contains(&query_word)
                .or(video::Column::Bvid.contains(query_word)),
        );
    }
    if let Some(status_filter) = request.status_filter {
        query = query.filter(status_filter.to_video_query());
    }
    if let Some(validation_filter) = request.validation_filter {
        query = query.filter(validation_filter.to_video_query());
    }
    let mut all_videos = query.into_partial_model::<SimpleVideoInfo>().all(&db).await?;
    let mut all_pages = page::Entity::find()
        .filter(page::Column::VideoId.is_in(all_videos.iter().map(|v| v.id)))
        .into_partial_model::<SimplePageInfo>()
        .all(&db)
        .await?;
    for video_info in all_videos.iter_mut() {
        let mut video_status = VideoStatus::from(video_info.download_status);
        for update in &request.video_updates {
            video_status.set(update.status_index, update.status_value);
        }
        video_info.download_status = video_status.into();
    }
    for page_info in all_pages.iter_mut() {
        let mut page_status = PageStatus::from(page_info.download_status);
        for update in &request.page_updates {
            page_status.set(update.status_index, update.status_value);
        }
        page_info.download_status = page_status.into();
    }
    let has_video_updates = !all_videos.is_empty();
    let has_page_updates = !all_pages.is_empty();
    if has_video_updates || has_page_updates {
        let txn = db.begin().await?;
        if has_video_updates {
            update_video_download_status(&txn, &all_videos, Some(500)).await?;
        }
        if has_page_updates {
            update_page_download_status(&txn, &all_pages, Some(500)).await?;
        }
        txn.commit().await?;
    }
    Ok(ApiResponse::ok(UpdateFilteredVideoStatusResponse {
        success: has_video_updates || has_page_updates,
        updated_videos_count: all_videos.len(),
        updated_pages_count: all_pages.len(),
    }))
}

/// 流式播放某个分页的视频文件，支持 HTTP Range 请求以便浏览器内播放器进行拖动进度条
pub async fn stream_page_video(
    Path((video_id, page_id)): Path<(i32, i32)>,
    Extension(db): Extension<DatabaseConnection>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let page_model = page::Entity::find()
        .filter(page::Column::Id.eq(page_id))
        .filter(page::Column::VideoId.eq(video_id))
        .one(&db)
        .await?
        .ok_or_else(|| InnerApiError::NotFound(page_id))?;
    // 校验视频内容子任务已完成
    let page_status = PageStatus::from(page_model.download_status);
    let status_array: [u32; 5] = page_status.into();
    if status_array[1] != STATUS_OK {
        return Err(InnerApiError::BadRequest("视频内容尚未下载完成".to_string()).into());
    }
    let video_path = page_model
        .path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| InnerApiError::NotFound(page_id))?;
    let file_metadata = tokio::fs::metadata(&video_path)
        .await
        .with_context(|| format!("获取视频文件信息失败：{}", video_path))?;
    let file_size = file_metadata.len();
    let file = File::open(&video_path)
        .await
        .with_context(|| format!("打开视频文件失败：{}", video_path))?;
    // 处理 Range 请求
    if let Some(range_header) = headers.get(header::RANGE).and_then(|v| v.to_str().ok())
        && let Some((start, end)) = parse_range(range_header, file_size)
    {
        let content_length = end - start + 1;
        let mut file = file;
        file.seek(SeekFrom::Start(start))
            .await
            .context("seek video file failed")?;
        let stream = ReaderStream::new(file.take(content_length));
        let body = Body::from_stream(stream);
        return Ok((
            StatusCode::PARTIAL_CONTENT,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("video/mp4")),
                (
                    header::CONTENT_LENGTH,
                    HeaderValue::from_str(&content_length.to_string()).unwrap(),
                ),
                (
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end, file_size)).unwrap(),
                ),
                (header::ACCEPT_RANGES, HeaderValue::from_static("bytes")),
            ],
            body,
        )
            .into_response());
    }
    // 无 Range 头：返回整个文件
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("video/mp4")),
            (
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&file_size.to_string()).unwrap(),
            ),
            (header::ACCEPT_RANGES, HeaderValue::from_static("bytes")),
        ],
        body,
    )
        .into_response())
}

/// 解析 HTTP Range 头，仅支持 bytes=start-end 或 bytes=start- 形式
fn parse_range(range_header: &str, file_size: u64) -> Option<(u64, u64)> {
    let range = range_header.strip_prefix("bytes=")?;
    let (start_str, end_str) = range.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = if end_str.is_empty() {
        file_size - 1
    } else {
        end_str.parse().ok()?
    };
    if start > end || start >= file_size {
        return None;
    }
    Some((start, end.min(file_size - 1)))
}
