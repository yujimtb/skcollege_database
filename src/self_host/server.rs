use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::api::envelope::ErrorResponse;
use crate::api::pagination::PaginationParams;
use crate::self_host::app::{AppService, SelfHostError};

pub fn build_router(service: AppService) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/admin/sync", post(sync_now))
        .route("/api/persons", get(list_persons))
        .route("/api/persons/{person_id}", get(person_detail))
        .route("/api/persons/{person_id}/slides", get(person_slides))
        .route("/api/persons/{person_id}/messages", get(person_messages))
        .route("/api/persons/{person_id}/timeline", get(person_timeline))
        .with_state(service)
}

#[derive(Debug, Deserialize, Default)]
struct ReadQuery {
    mode: Option<String>,
    pin: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct PersonsQuery {
    mode: Option<String>,
    pin: Option<String>,
    #[serde(flatten)]
    pagination: PaginationParams,
}

async fn health(State(service): State<AppService>) -> Result<Json<crate::api::health::HealthResponse>, ApiError> {
    Ok(Json(service.health()?))
}

async fn sync_now(State(service): State<AppService>) -> Result<Json<crate::self_host::app::SyncReport>, ApiError> {
    let report = tokio::task::spawn_blocking(move || service.sync_all())
        .await
        .map_err(|err| ApiError::internal(err.to_string()))??;
    Ok(Json(report))
}

async fn list_persons(
    State(service): State<AppService>,
    Query(query): Query<PersonsQuery>,
) -> Result<Json<crate::api::envelope::ResponseEnvelope<serde_json::Value>>, ApiError> {
    Ok(Json(service.persons_response(
        query.mode.as_deref(),
        query.pin.as_deref(),
        &query.pagination,
    )?))
}

async fn person_detail(
    State(service): State<AppService>,
    Path(person_id): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<crate::api::envelope::ResponseEnvelope<serde_json::Value>>, ApiError> {
    Ok(Json(service.person_detail_response(
        &person_id,
        query.mode.as_deref(),
        query.pin.as_deref(),
    )?))
}

async fn person_slides(
    State(service): State<AppService>,
    Path(person_id): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<crate::api::envelope::ResponseEnvelope<serde_json::Value>>, ApiError> {
    Ok(Json(service.person_slides_response(
        &person_id,
        query.mode.as_deref(),
        query.pin.as_deref(),
    )?))
}

async fn person_messages(
    State(service): State<AppService>,
    Path(person_id): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<crate::api::envelope::ResponseEnvelope<serde_json::Value>>, ApiError> {
    Ok(Json(service.person_messages_response(
        &person_id,
        query.mode.as_deref(),
        query.pin.as_deref(),
    )?))
}

async fn person_timeline(
    State(service): State<AppService>,
    Path(person_id): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<crate::api::envelope::ResponseEnvelope<serde_json::Value>>, ApiError> {
    Ok(Json(service.person_timeline_response(
        &person_id,
        query.mode.as_deref(),
        query.pin.as_deref(),
    )?))
}

struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn internal(detail: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ErrorResponse::bad_request(&detail),
        }
    }
}

impl From<SelfHostError> for ApiError {
    fn from(value: SelfHostError) -> Self {
        match value {
            SelfHostError::NotFound(_) => Self {
                status: StatusCode::NOT_FOUND,
                body: ErrorResponse::not_found(),
            },
            SelfHostError::ReadMode(detail) => Self {
                status: StatusCode::BAD_REQUEST,
                body: ErrorResponse::bad_request(&detail),
            },
            SelfHostError::Policy(detail) => Self {
                status: StatusCode::FORBIDDEN,
                body: ErrorResponse::forbidden(&detail),
            },
            other => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: ErrorResponse::bad_request(&other.to_string()),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}