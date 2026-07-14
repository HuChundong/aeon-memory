use std::sync::Arc;

use crate::service::*;
use axum::{
    Json, Router,
    extract::{Request, State, rejection::JsonRejection},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use subtle::ConstantTimeEq;

/// The complete production HTTP surface. OPTIONS is CORS protocol handling,
/// not an application endpoint.
pub const ROUTES: [(&str, &str); 10] = [
    ("GET", "/health"),
    ("POST", "/recall"),
    ("POST", "/capture"),
    ("POST", "/search/memories"),
    ("POST", "/search/conversations"),
    ("POST", "/session/end"),
    ("POST", "/seed"),
    ("POST", "/offload/before-prompt"),
    ("POST", "/offload/after-tool"),
    ("POST", "/offload/llm-output"),
];

#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
}

#[derive(Clone)]
struct AppState {
    service: Arc<dyn AeonMemoryService>,
    api_key: Option<Arc<str>>,
    cors_origins: Arc<[String]>,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

fn error(status: StatusCode, message: impl Into<String>, code: Option<&'static str>) -> Response {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
            code,
        }),
    )
        .into_response()
}

fn service_error(err: ServiceError) -> Response {
    match err {
        ServiceError::InvalidInput(message) => {
            error(StatusCode::BAD_REQUEST, message, Some("INVALID_INPUT"))
        }
        ServiceError::NotFound(message) => error(StatusCode::NOT_FOUND, message, Some("NOT_FOUND")),
        ServiceError::Internal(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            message,
            Some("INTERNAL_ERROR"),
        ),
    }
}

fn json_error(err: JsonRejection) -> Response {
    let _ = err;
    error(StatusCode::INTERNAL_SERVER_ERROR, "Invalid JSON body", None)
}

fn missing_required(fields: &'static str) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        format!(
            "Missing required field{}: {fields}",
            if fields.contains(',') { "s" } else { "" }
        ),
        None,
    )
}

pub fn app(service: Arc<dyn AeonMemoryService>, config: AppConfig) -> Router {
    let state = AppState {
        service,
        api_key: config.api_key.filter(|key| !key.is_empty()).map(Arc::from),
        cors_origins: config.cors_origins.into(),
    };

    let protected = Router::new()
        .route("/recall", post(recall))
        .route("/capture", post(capture))
        .route("/search/memories", post(search_memories))
        .route("/search/conversations", post(search_conversations))
        .route("/session/end", post(end_session))
        .route("/seed", post(seed))
        .route("/offload/before-prompt", post(before_prompt))
        .route("/offload/after-tool", post(after_tool))
        .route("/offload/llm-output", post(llm_output))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), cors))
        .with_state(state)
}

async fn cors(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let request_origin = request.headers().get(header::ORIGIN).cloned();
    let is_preflight = request.method() == Method::OPTIONS;
    let mut response = if is_preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };

    if state.cors_origins.is_empty() {
        return response;
    }

    let headers = response.headers_mut();
    if state.cors_origins.iter().any(|origin| origin == "*") {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        insert_cors_protocol_headers(headers);
        return response;
    }

    // The TS gateway writes exactly `Vary: Origin` for a concrete allow-list,
    // including denied or missing Origin requests. Do not use tower-http's
    // default three-field Vary value here: it changes observable cache keys.
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    let allowed = request_origin.as_ref().is_some_and(|request_origin| {
        request_origin.to_str().is_ok_and(|request_origin| {
            state
                .cors_origins
                .iter()
                .any(|allowed| allowed == request_origin)
        })
    });
    if allowed {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            request_origin.expect("allowed origin is present"),
        );
        insert_cors_protocol_headers(headers);
    }
    response
}

fn insert_cors_protocol_headers(headers: &mut axum::http::HeaderMap) {
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, Authorization"),
    );
}

async fn authenticate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(expected) = state.api_key.as_deref() else {
        return next.run(request).await;
    };
    let Some(header) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return error(
            StatusCode::UNAUTHORIZED,
            "Unauthorized: missing Bearer token",
            None,
        );
    };
    let Some(provided) = header
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return error(
            StatusCode::UNAUTHORIZED,
            "Unauthorized: missing Bearer token",
            None,
        );
    };
    let valid = provided.len() == expected.len()
        && bool::from(provided.as_bytes().ct_eq(expected.as_bytes()));
    if !valid {
        return error(
            StatusCode::UNAUTHORIZED,
            "Unauthorized: invalid token",
            None,
        );
    }
    next.run(request).await
}

async fn health(State(state): State<AppState>) -> Response {
    match state.service.health().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => service_error(err),
    }
}

async fn recall(
    State(state): State<AppState>,
    body: Result<Json<RecallRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.query.is_empty() || request.session_key.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "Missing required fields: query, session_key",
            None,
        );
    }
    match state.service.recall(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn capture(
    State(state): State<AppState>,
    body: Result<Json<CaptureRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.user_content.is_empty()
        || request.assistant_content.is_empty()
        || request.session_key.is_empty()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Missing required fields: user_content, assistant_content, session_key",
            None,
        );
    }
    match state.service.capture(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn search_memories(
    State(state): State<AppState>,
    body: Result<Json<MemorySearchRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.query.is_empty() {
        return missing_required("query");
    }
    match state.service.search_memories(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn search_conversations(
    State(state): State<AppState>,
    body: Result<Json<ConversationSearchRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.query.is_empty() {
        return missing_required("query");
    }
    match state.service.search_conversations(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn end_session(
    State(state): State<AppState>,
    body: Result<Json<SessionEndRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.session_key.is_empty() {
        return missing_required("session_key");
    }
    match state.service.end_session(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn seed(
    State(state): State<AppState>,
    body: Result<Json<SeedRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.data.is_null() {
        return error(
            StatusCode::BAD_REQUEST,
            "Missing required field: data",
            None,
        );
    }
    match state.service.seed(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn before_prompt(
    State(state): State<AppState>,
    body: Result<Json<BeforePromptRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.agent_id.is_empty() || request.session_id.is_empty() {
        return missing_required("agent_id, session_id");
    }
    match state.service.before_prompt(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn after_tool(
    State(state): State<AppState>,
    body: Result<Json<AfterToolRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.agent_id.is_empty()
        || request.session_id.is_empty()
        || request.tool.tool_call_id.is_empty()
    {
        return missing_required("agent_id, session_id, tool.toolCallId");
    }
    match state.service.after_tool(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn llm_output(
    State(state): State<AppState>,
    body: Result<Json<LlmOutputRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(value)) => value,
        Err(err) => return json_error(err),
    };
    if request.agent_id.is_empty() || request.session_id.is_empty() {
        return missing_required("agent_id, session_id");
    }
    match state.service.llm_output(request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => service_error(err),
    }
}

async fn not_found(request: Request) -> Response {
    error(
        StatusCode::NOT_FOUND,
        format!("Not found: {} {}", request.method(), request.uri().path()),
        None,
    )
}
