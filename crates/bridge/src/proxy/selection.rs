//! Wiki: docs/wiki/model-selection.md

use axum::Json;
use axum::http::{Method, StatusCode, request::Parts};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;

#[derive(Debug, PartialEq, Eq)]
enum Admission {
    List,
    Inference,
}

#[derive(Deserialize)]
struct RequestModel {
    model: String,
}

fn check(
    method: &Method,
    path: &str,
    body: &[u8],
    selected: &str,
) -> Result<Admission, (StatusCode, &'static str)> {
    if method == Method::GET && path == "/v1/models" && body.is_empty() {
        return Ok(Admission::List);
    }
    if method != Method::POST
        || !matches!(
            path,
            "/v1/chat/completions" | "/v1/completions" | "/v1/embeddings" | "/v1/responses"
        )
    {
        return Err((
            StatusCode::FORBIDDEN,
            "endpoint is unavailable in selected-model mode",
        ));
    }
    let request: RequestModel = serde_json::from_slice(body).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "request must contain one string model field",
        )
    })?;
    if request.model != selected {
        return Err((
            StatusCode::FORBIDDEN,
            "requested model is not selected on this bridge",
        ));
    }
    Ok(Admission::Inference)
}

pub(super) async fn admit(st: &AppState, parts: &Parts, body: &[u8]) -> Option<Response> {
    let selected = st.selected_model.as_deref()?;
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", |value| value.as_str());
    match check(&parts.method, path, body, selected) {
        Ok(Admission::Inference) => None,
        Ok(Admission::List) => {
            let status = st.status_cache.read().await;
            let models: Vec<_> = status.models.iter().filter(|model| model.id == selected)
                .map(|model| json!({"id": model.id, "object": "model", "owned_by": "bridge"})).collect();
            drop(status);
            Some(Json(json!({"object": "list", "data": models})).into_response())
        }
        Err((code, message)) => Some((code, Json(json!({"error": {
            "message": message, "type": "invalid_request_error", "code": "model_selection_policy"
        }}))).into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_model_is_required_without_rewriting() {
        let path = "/v1/chat/completions";
        assert_eq!(
            check(&Method::POST, path, br#"{"model":"selected"}"#, "selected"),
            Ok(Admission::Inference)
        );
        assert_eq!(
            check(&Method::POST, path, br#"{"model":"other"}"#, "selected")
                .err()
                .map(|error| error.0),
            Some(StatusCode::FORBIDDEN)
        );
        for body in [
            b"{}".as_slice(),
            br#"{"model":null}"#,
            br#"{"model":1}"#,
            br#"{"model":"selected","model":"other"}"#,
            br#"{"model":"other","model":"selected"}"#,
            b"invalid",
        ] {
            assert_eq!(
                check(&Method::POST, path, body, "selected")
                    .err()
                    .map(|error| error.0),
                Some(StatusCode::BAD_REQUEST)
            );
        }
    }

    #[test]
    fn native_paths_queries_and_alternate_methods_are_blocked() {
        assert_eq!(
            check(&Method::GET, "/v1/models", b"", "selected"),
            Ok(Admission::List)
        );
        for path in [
            "/api/v1/models/load",
            "/api/v0/models",
            "/api/chat",
            "/v1/chat/completions?model=other",
            "/v1/chat/completions/",
            "/v1/models?all=true",
            "/v1/models/other",
        ] {
            assert!(check(&Method::POST, path, br#"{"model":"selected"}"#, "selected").is_err());
            assert!(check(&Method::GET, path, b"", "selected").is_err());
        }
        assert!(check(&Method::DELETE, "/v1/models", b"", "selected").is_err());
        assert!(check(&Method::GET, "/v1/models", b"{}", "selected").is_err());
    }
}
