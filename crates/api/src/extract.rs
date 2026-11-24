use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::request::Parts;
use nullradix_app::AppError;

use crate::ApiError;

/// JSON request body whose rejections are rendered as problem details.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(AppError::invalid("input-invalid", rejection.body_text()).into()),
        }
    }
}

/// Query string whose rejections are rendered as problem details.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    Query<T>: FromRequestParts<S, Rejection = QueryRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(AppError::invalid("query-invalid", rejection.body_text()).into()),
        }
    }
}

/// Path parameters whose rejections are rendered as problem details.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    Path<T>: FromRequestParts<S, Rejection = PathRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(AppError::invalid("path-invalid", rejection.body_text()).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{StatusCode, header};
    use axum::routing::post;
    use serde::Deserialize;
    use tower::ServiceExt;

    use super::*;

    #[derive(Deserialize)]
    struct Input {
        name: String,
    }

    async fn echo(ApiJson(input): ApiJson<Input>) -> String {
        input.name
    }

    async fn send(body: &str) -> (StatusCode, String) {
        let request = axum::http::Request::post("/")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap();
        let response = Router::new()
            .route("/", post(echo))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn passes_valid_bodies_through() {
        assert_eq!(
            send(r#"{"name":"board"}"#).await,
            (StatusCode::OK, String::from("board"))
        );
    }

    #[tokio::test]
    async fn rejects_invalid_bodies_with_problem_details() {
        let (status, body) = send(r#"{"nom":"board"}"#).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let problem: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(problem["code"], "input-invalid");
    }
}
