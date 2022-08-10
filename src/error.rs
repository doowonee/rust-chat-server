use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use thiserror::Error;

/// 에러 enum
#[derive(Error, Debug)]
pub enum Error {
    #[error("Redis error: {0}")]
    Redis(String),
    #[error("Unknown error: {0}")]
    Unknown(String),
}

/// axum error 핸들러 구현
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let body = "something else went wrong";
        // its often easiest to implement `IntoResponse` by calling other implementations
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}
