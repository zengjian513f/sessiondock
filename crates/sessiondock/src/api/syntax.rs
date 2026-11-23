use axum::{Json, http::StatusCode};

use crate::{
    error::ApiError,
    syntax::{self, Block, Highlight},
};

pub(super) async fn highlight(
    Json(blocks): Json<Vec<Block>>,
) -> Result<Json<Vec<Option<Highlight>>>, ApiError> {
    syntax::highlight(blocks).await.map(Json).map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "syntax_unavailable",
            error,
        )
    })
}
