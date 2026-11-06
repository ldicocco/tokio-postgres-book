//! `AppError`: every way a request can fail, and what each one looks like to an API client
//! (Chapter 6, Chapter 15).
//!
//! Database errors are classified once, in `From<tokio_postgres::Error>`, by SQLSTATE and by
//! constraint name. Constraint names are part of the schema's contract with this file:
//! `migrations/0001_init.sql` names every constraint a client can trip over, and
//! [`constraint_message`] says what each one means. A `DbError`'s own text never reaches a
//! client, because it can contain table names, values and other internals; it goes to the log.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::PoolError;
use serde_json::{Value, json};
use tokio_postgres::error::{DbError, SqlState};

use crate::domain::{OrderStatus, Sku};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0} not found")]
    NotFound(&'static str),

    /// The request is well-formed but asks for something invalid (422).
    #[error("{message}")]
    Invalid {
        field: Option<&'static str>,
        message: String,
    },

    /// The request clashes with existing data, such as a duplicate SKU (409).
    #[error("{message}")]
    Conflict {
        field: Option<&'static str>,
        message: String,
    },

    #[error("not enough stock of {sku}: {requested} requested, {available} available")]
    InsufficientStock {
        sku: Sku,
        requested: i32,
        available: i32,
    },

    #[error("an order cannot go from {from} to {to}")]
    Transition { from: OrderStatus, to: OrderStatus },

    /// `If-Match` named a version that is no longer current (412).
    #[error("the resource has changed: its current version is {current}")]
    StaleVersion { current: i32 },

    /// Serialization failure, deadlock or lock timeout: retried by `db::in_transaction`, and
    /// reported (503, `Retry-After`) only when the retries run out.
    #[error("the database is busy, try again")]
    Busy(#[source] tokio_postgres::Error),

    /// `statement_timeout` or a cancel request ended the query (SQLSTATE 57014).
    #[error("the query took too long")]
    Timeout(#[source] tokio_postgres::Error),

    /// No connection: the pool timed out, or the connection is gone.
    #[error("the database is unavailable: {0}")]
    Unavailable(String),

    /// Anything else from the database: a bug, or something an operator must look at.
    #[error("database error")]
    Db(#[source] tokio_postgres::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        AppError::Invalid {
            field: Some(field),
            message: message.into(),
        }
    }

    /// Worth running the whole transaction again: the failure was caused by concurrency,
    /// not by the request.
    pub fn is_retryable(&self) -> bool {
        matches!(self, AppError::Busy(_))
    }

    /// The client's fault (4xx): safe to report per item, as `POST /products/batch` does.
    pub fn is_client_error(&self) -> bool {
        self.status().is_client_error()
    }

    pub fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Invalid { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Conflict { .. }
            | AppError::InsufficientStock { .. }
            | AppError::Transition { .. } => StatusCode::CONFLICT,
            AppError::StaleVersion { .. } => StatusCode::PRECONDITION_FAILED,
            AppError::Busy(_) | AppError::Timeout(_) | AppError::Unavailable(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            AppError::Db(_) | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The JSON body a client sees: a stable `error` code, a human message, and the fields
    /// a program needs to react.
    pub fn body(&self) -> Value {
        let code = match self {
            AppError::NotFound(_) => "not_found",
            AppError::Invalid { .. } => "invalid",
            AppError::Conflict { .. } => "conflict",
            AppError::InsufficientStock { .. } => "insufficient_stock",
            AppError::Transition { .. } => "invalid_transition",
            AppError::StaleVersion { .. } => "stale_version",
            AppError::Busy(_) => "busy",
            AppError::Timeout(_) => "timeout",
            AppError::Unavailable(_) => "unavailable",
            AppError::Db(_) | AppError::Internal(_) => "internal",
        };
        // Internal details stay in the log.
        let message = match self {
            AppError::Db(_) | AppError::Internal(_) => {
                "something went wrong on our side".to_owned()
            }
            other => other.to_string(),
        };
        let mut body = json!({ "error": code, "message": message });
        match self {
            AppError::Invalid {
                field: Some(field), ..
            }
            | AppError::Conflict {
                field: Some(field), ..
            } => {
                body["field"] = json!(field);
            }
            AppError::InsufficientStock {
                sku,
                requested,
                available,
            } => {
                body["sku"] = json!(sku);
                body["requested"] = json!(requested);
                body["available"] = json!(available);
            }
            AppError::Transition { from, to } => {
                body["from"] = json!(from);
                body["to"] = json!(to);
            }
            AppError::StaleVersion { current } => body["currentVersion"] = json!(current),
            _ => {}
        }
        body
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            log_server_error(&self);
        }
        let mut response = (status, Json(self.body())).into_response();
        if matches!(self, AppError::Busy(_)) {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

fn log_server_error(error: &AppError) {
    let db = match error {
        AppError::Busy(e) | AppError::Timeout(e) | AppError::Db(e) => e.as_db_error(),
        _ => None,
    };
    match db {
        Some(db) => tracing::error!(
            code = db.code().code(),
            message = db.message(),
            detail = db.detail(),
            hint = db.hint(),
            table = db.table(),
            constraint = db.constraint(),
            "{error}"
        ),
        None => tracing::error!(error = ?error, "{error}"),
    }
}

impl From<tokio_postgres::Error> for AppError {
    fn from(error: tokio_postgres::Error) -> Self {
        let Some(db) = error.as_db_error() else {
            // Not from the server: IO, a closed connection, or a conversion on our side.
            return if error.is_closed() {
                AppError::Unavailable("the connection to the database was lost".into())
            } else {
                AppError::Db(error)
            };
        };
        match *db.code() {
            SqlState::UNIQUE_VIOLATION => {
                let (field, message) =
                    constraint_message(db).unwrap_or((None, "this already exists"));
                AppError::Conflict {
                    field,
                    message: message.into(),
                }
            }
            SqlState::CHECK_VIOLATION
            | SqlState::NOT_NULL_VIOLATION
            | SqlState::FOREIGN_KEY_VIOLATION => {
                let (field, message) = constraint_message(db)
                    .unwrap_or((None, "the request breaks a rule of the data"));
                AppError::Invalid {
                    field,
                    message: message.into(),
                }
            }
            SqlState::T_R_SERIALIZATION_FAILURE
            | SqlState::T_R_DEADLOCK_DETECTED
            | SqlState::LOCK_NOT_AVAILABLE => AppError::Busy(error),
            SqlState::QUERY_CANCELED => AppError::Timeout(error),
            SqlState::ADMIN_SHUTDOWN
            | SqlState::CANNOT_CONNECT_NOW
            | SqlState::TOO_MANY_CONNECTIONS => AppError::Unavailable(db.message().to_owned()),
            _ => AppError::Db(error),
        }
    }
}

/// What a violated constraint means to a client: the request field it is about, and a message.
pub fn constraint_message(db: &DbError) -> Option<(Option<&'static str>, &'static str)> {
    let (field, message) = match db.constraint()? {
        "products_sku_key" => ("sku", "a product with this SKU already exists"),
        "stores_code_key" => ("code", "a store with this code already exists"),
        "sku_format" => (
            "sku",
            "a SKU is 3–12 upper-case letters or digits, optionally followed by a dash and 1–6 more",
        ),
        "stores_code_format" => (
            "code",
            "a store code is 2–32 lower-case letters, digits or dashes",
        ),
        "stores_name_length" | "products_name_length" => {
            ("name", "the name must be 1–200 characters")
        }
        "products_price_positive" => ("priceCents", "the price must be positive"),
        "positive_quantity_check" => ("quantity", "a quantity must be positive"),
        "stock_on_hand_not_negative" => ("delta", "stock on hand cannot go below zero"),
        "stock_reserved_within_on_hand" => {
            ("delta", "stock on hand cannot go below what is reserved")
        }
        _ => return None,
    };
    Some((Some(field), message))
}

impl From<PoolError> for AppError {
    fn from(error: PoolError) -> Self {
        match error {
            PoolError::Backend(e) => e.into(),
            PoolError::Timeout(_) => {
                AppError::Unavailable("no database connection became free in time".into())
            }
            other => AppError::Unavailable(other.to_string()),
        }
    }
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        AppError::Invalid {
            field: None,
            message: rejection.body_text(),
        }
    }
}

impl From<QueryRejection> for AppError {
    fn from(rejection: QueryRejection) -> Self {
        AppError::Invalid {
            field: None,
            message: rejection.body_text(),
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        AppError::Internal(error.to_string())
    }
}
