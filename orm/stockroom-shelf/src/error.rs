//! shelf's errors in the API's terms: stockroom's `AppError`, unchanged.
//!
//! An `impl From<shelf::Error> for AppError` would let `?` convert them, but neither type
//! belongs to this crate, and the orphan rule refuses it (E0117). So every repository function
//! converts with `map_err(app_error)`.

use stockroom::AppError;

/// What a client sees when shelf fails: a missing row is 404, a stale version 412, a database
/// error whatever stockroom makes of it (its SQLSTATE and constraint names), and a mapping or
/// schema error, which is a bug, 500.
pub fn app_error(error: shelf::Error) -> AppError {
    match error {
        shelf::Error::NotFound { model } => AppError::NotFound(noun(model)),
        shelf::Error::Stale {
            current_version, ..
        } => match i32::try_from(current_version) {
            Ok(current) => AppError::StaleVersion { current },
            Err(_) => AppError::Internal(format!("version {current_version} is not an i32")),
        },
        shelf::Error::Db(error) => error.into(),
        other => AppError::Internal(other.to_string()),
    }
}

/// The word stockroom's messages use for a model's rows: "product not found".
fn noun(model: &'static str) -> &'static str {
    match model {
        "Store" => "store",
        "Product" => "product",
        "OrderRow" => "order",
        "LineRow" => "order line",
        "Stock" => "stock",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_the_api_s_terms() {
        let not_found = app_error(shelf::Error::NotFound { model: "Product" });
        assert_eq!(not_found.to_string(), "product not found");
        let stale = app_error(shelf::Error::Stale {
            model: "Product",
            current_version: 3,
        });
        assert_eq!(stale.body()["currentVersion"], 3);
        assert_eq!(stale.status().as_u16(), 412);
    }
}
