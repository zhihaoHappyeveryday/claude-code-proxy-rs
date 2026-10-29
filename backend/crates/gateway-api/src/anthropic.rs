//! Claude Code 原生 Messages HTTP 入口，复用 Core 的鉴权、准入、租约与结算。
use crate::ApiState;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Extension, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
};
use gateway_core::{
    engine::{
        EngineError,
        authentication::ClientAuthenticationRequest,
        execution::{
            ClientAuthenticationError, ClientTransport, ExecutionRequestMetadata, ExecutionSession,
            PreparedExecutionRequest, gateway_error_from_engine,
        },
    },
    error::{GatewayError, GatewayErrorKind},
    event::ProviderResponseHeader,
    lifecycle::Deadline,
    operation::{GenerateRequest, Operation, ProtocolPayload},
    routing::PublicModelId,
};
use serde_json::{Value, json};

/// HTTP 外层只限制入站读取/鉴权；开始执行后由 Core 统一收敛 timeout 与用量。
#[derive(Clone)]
pub(crate) struct ExecutionDeadline {
    pub deadline: Deadline,
    pub started: std::sync::Arc<tokio::sync::Notify>,
    pub handed_off: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub(crate) fn router() -> Router<ApiState> {
    Router::new()
        .route("/v1/messages", post(messages))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
}

async fn messages(
    State(state): State<ApiState>,
    uri: Uri,
    headers: HeaderMap,
    deadline: Option<Extension<ExecutionDeadline>>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(authorization) = client_authorization(&headers) else {
        return local_error(401, "authentication_error", "Invalid client API key");
    };
    let Ok(auth) = ClientAuthenticationRequest::new(authorization) else {
        return local_error(401, "authentication_error", "Invalid client API key");
    };
    let auth = auth.with_request_context(
        crate::middleware::current().and_then(|c| c.extensions.shared_request()),
    );
    let auth = match crate::middleware::current_settings() {
        Some(settings) => auth.with_settings(settings),
        None => auth,
    };
    let client = match state.execution.authenticate_request(auth).await {
        Ok(client) => client,
        Err(ClientAuthenticationError::InvalidKey) => {
            return local_error(401, "authentication_error", "Invalid client API key");
        }
        Err(_) => return local_error(503, "api_error", "Gateway is unavailable"),
    };
    let Ok(Json(Value::Object(body))) = body else {
        return local_error(400, "invalid_request_error", "Expected a JSON object");
    };
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return local_error(400, "invalid_request_error", "model is required");
    };
    let Ok(public_model) = PublicModelId::new(model) else {
        return local_error(400, "invalid_request_error", "Invalid model");
    };
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let Ok(payload) = ProtocolPayload::json_object("anthropic", body) else {
        return local_error(400, "invalid_request_error", "Invalid request");
    };
    let hop_headers = headers
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let wire_headers = headers
        .iter()
        .filter(|(name, _)| !hop_headers.iter().any(|hop| hop == name.as_str()))
        .filter(|(name, _)| {
            name.as_str().starts_with("anthropic-")
                || name.as_str().starts_with("x-stainless-")
                || matches!(
                    name.as_str(),
                    "user-agent" | "accept" | "x-app" | "x-client-request-id"
                )
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| json!([name.as_str(), value]))
        })
        .collect::<Vec<_>>();
    let payload = payload.with_context(serde_json::Map::from_iter([
        ("headers".into(), wire_headers.into()),
        (
            "query".into(),
            uri.query().map(Value::from).unwrap_or(Value::Null),
        ),
    ]));
    let mut prepared = match state.execution.prepare_execution(client).await {
        Ok(prepared) => prepared,
        Err(error) => return gateway_error(&error),
    };
    if let Some(Extension(deadline)) = deadline {
        prepared.constrain_deadline(deadline.deadline);
        deadline
            .handed_off
            .store(true, std::sync::atomic::Ordering::Release);
        deadline.started.notify_one();
    }
    let started = state
        .execution
        .start_prepared(
            prepared,
            PreparedExecutionRequest {
                public_model,
                operation: Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
                metadata: ExecutionRequestMetadata {
                    protocol: "anthropic".into(),
                    endpoint: "/v1/messages".into(),
                    transport: if streaming {
                        ClientTransport::HttpSse
                    } else {
                        ClientTransport::HttpJson
                    },
                    stream: streaming,
                    client_ip: None,
                    user_agent: headers
                        .get("user-agent")
                        .and_then(|h| h.to_str().ok())
                        .map(str::to_owned),
                    previous_response_id: None,
                },
            },
        )
        .await;
    let started = match started {
        Ok(started) => started,
        Err(error) => return gateway_error(&error),
    };
    if streaming {
        stream(SessionGuard(Some(started.session))).await
    } else {
        collect(SessionGuard(Some(started.session))).await
    }
}

fn client_authorization(headers: &HeaderMap) -> Option<String> {
    if headers.get_all("x-api-key").iter().count() > 1
        || headers.get_all("authorization").iter().count() > 1
    {
        return None;
    }
    let key = headers.get("x-api-key").and_then(|v| v.to_str().ok());
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok());
    match (key, auth) {
        (Some(key), None) if !key.is_empty() => Some(format!("Bearer {key}")),
        (None, Some(auth)) => Some(auth.to_owned()),
        (Some(key), Some(auth)) if auth.strip_prefix("Bearer ") == Some(key) => {
            Some(auth.to_owned())
        }
        _ => None,
    }
}

async fn collect(mut guard: SessionGuard) -> Response {
    let session = guard.0.as_mut().expect("owned execution");
    let events = match session.collect_uncommitted().await {
        Ok(events) => events,
        Err(error) => {
            let response = engine_error(&error);
            let _ = session
                .record_client_status(response.status().as_u16())
                .await;
            return response;
        }
    };
    let body = events
        .into_iter()
        .filter_map(|event| event.into_parts().1)
        .find_map(|wire| wire.into_raw_json_body());
    let Some(body) = body else {
        return local_error(502, "api_error", "Invalid upstream response");
    };
    let mut response = Response::new(Body::from(body));
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    apply_headers(&mut response, session.response_headers());
    if let Err(error) = session.commit_downstream(Some(200)).await {
        return engine_error(&error);
    }
    if session.is_finalized() {
        guard.0 = None;
    }
    response
}

async fn stream(mut guard: SessionGuard) -> Response {
    // 先拿到上游响应，保留首段前的 HTTP 错误语义。
    let session = guard.0.as_mut().expect("owned execution");
    let first = match session.next_event().await {
        Ok(Some(event)) => event,
        Ok(None) => return local_error(502, "api_error", "Empty upstream response"),
        Err(error) => {
            let response = engine_error(&error);
            let _ = session
                .record_client_status(response.status().as_u16())
                .await;
            return response;
        }
    };
    let headers = session.response_headers().to_vec();
    let output = async_stream::stream! {
        let mut pending = Some(first);
        let mut upstream_error_delivered = false;
        loop {
            let session = guard.0.as_mut().expect("owned execution");
            let batch = match pending.take() {
                Some(batch) => Some(batch),
                None => match session.next_event().await {
                    Ok(batch) => batch,
                    Err(_) => {
                        if !upstream_error_delivered {
                        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"Upstream stream interrupted\"}}\n\n"));
                        }
                        break;
                    }
                }
            };
            let Some(batch) = batch else { break; };
            if batch.commit_requirement() == gateway_core::engine::CommitRequirement::CommitBeforeDelivery
                && session.commit_downstream(Some(200)).await.is_err() {
                yield Err(std::io::Error::other("Gateway stream commit failed"));
                break;
            }
            for event in batch.into_provider_events() {
                if let Some(wire) = event.into_parts().1 && let Some(chunk) = wire.raw_sse_frame() {
                    upstream_error_delivered |= wire.event_type() == Some("error");
                    yield Ok(chunk.clone());
                }
            }
        }
    };
    let mut response = Response::new(Body::from_stream(output));
    apply_headers(&mut response, &headers);
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

struct SessionGuard(Option<Box<dyn ExecutionSession>>);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(session) = self.0.take()
            && !session.is_finalized()
        {
            session.cancel();
            let finalize = session.detach_finalize();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                drop(runtime.spawn(finalize));
            }
        }
    }
}

fn apply_headers(response: &mut Response, headers: &[ProviderResponseHeader]) {
    for header in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name().as_bytes()),
            HeaderValue::from_bytes(header.value()),
        ) {
            response.headers_mut().append(name, value);
        }
    }
}
fn engine_error(error: &EngineError) -> Response {
    if let EngineError::Provider(error) = error
        && let Some(upstream) = error.client_visible_upstream_response()
    {
        let mut response = Response::new(Body::from(upstream.body().clone()));
        *response.status_mut() =
            StatusCode::from_u16(upstream.status()).unwrap_or(StatusCode::BAD_GATEWAY);
        apply_headers(&mut response, upstream.headers());
        return response;
    }
    gateway_error(&gateway_error_from_engine(error))
}
fn gateway_error(error: &GatewayError) -> Response {
    let (status, kind) = match error.kind() {
        GatewayErrorKind::Unauthorized => (401, "authentication_error"),
        GatewayErrorKind::InvalidRequest | GatewayErrorKind::Unsupported => {
            (400, "invalid_request_error")
        }
        GatewayErrorKind::ModelNotFound => (404, "not_found_error"),
        GatewayErrorKind::PolicyDenied => (403, "permission_error"),
        GatewayErrorKind::RateLimited => (429, "rate_limit_error"),
        GatewayErrorKind::Timeout => (504, "api_error"),
        GatewayErrorKind::UpstreamUnavailable => (502, "api_error"),
        _ => (503, "api_error"),
    };
    local_error(status, kind, error.client_message())
}
fn local_error(status: u16, kind: &str, message: &str) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({"type":"error", "error":{"type":kind,"message":message}})),
    )
        .into_response()
}

pub(crate) fn timeout_response() -> Response {
    local_error(504, "api_error", "Request timed out")
}
