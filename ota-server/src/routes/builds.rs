use axum::{ Json, extract::{ Path, State }, http::StatusCode };
use serde::Serialize;
use uuid::Uuid;

use crate::{
    errors::{ ApiError, ApiResult },
    models::{ BuildResponse, BuildStatus },
    state::AppState,
};

#[derive(Serialize)]
pub struct BuildStartedResponse {
    build_id: String,
    status: &'static str,
}

pub async fn start_build(
    State(state): State<AppState>,
    Path(project_id): Path<String> ) -> ApiResult<(StatusCode, Json<BuildStartedResponse>)> {
    let project = state.storage.get_project(&project_id).await?;
    let active: i64 = sqlx
        ::query_scalar(
            "SELECT COUNT(*) FROM builds WHERE project_id=? AND status IN ('queued','building')"
        )
        .bind(&project_id)
        .fetch_one(state.storage.pool()).await?;
    if active > 0 {
        return Err(
            ApiError::conflict(
                "BUILD_ALREADY_RUNNING",
                "This project already has a queued or active build"
            )
        );
    }

    let build_id = Uuid::new_v4().to_string();
    state.storage.insert_build(&build_id, &project_id, BuildStatus::Queued.as_str()).await?;
    state.storage.append_build_log(&build_id, "Build queued").await?;
    state.coordinator.start(project, build_id.clone());

    Ok((
        StatusCode::ACCEPTED,
        Json(BuildStartedResponse {
            build_id,
            status: "building",
        }),
    ))
}

pub async fn get_build(
    State(state): State<AppState>,
    Path(build_id): Path<String>
) -> ApiResult<Json<BuildResponse>> {
    Ok(Json(state.storage.get_build(&build_id).await?.into()))
}

pub async fn list_builds(State(state): State<AppState>) -> ApiResult<Json<Vec<BuildResponse>>> {
    Ok(Json(state.storage.list_builds().await?.into_iter().map(Into::into).collect()))
}
