//! `Sku`: a stock-keeping unit, validated when it is made and when it is read.
//!
//! The conversions to and from PostgreSQL are written by hand rather than derived
//! (Chapter 5): the database column is the domain `sku`, and tokio-postgres describes a
//! parameter that targets that column as the domain itself, not as `text`, so a plain `&str`
//! would be refused with "cannot convert between the Rust type `&str` and the Postgres type
//! `sku`". `accepts` says yes to both.

use std::error::Error;
use std::fmt;

use bytes::BytesMut;
use postgres_types::{FromSql, IsNull, Kind, ToSql, Type, to_sql_checked};
use serde::{Deserialize, Serialize};

/// 3–12 upper-case letters or digits, optionally followed by a dash and 1–6 more: `CHAIR-01`.
/// The same rule as the `sku_format` constraint on the domain.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sku(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{0:?} is not a SKU: expected 3–12 upper-case letters or digits, optionally followed by a dash and 1–6 more"
)]
pub struct InvalidSku(pub String);

impl Sku {
    pub fn parse(text: &str) -> Result<Sku, InvalidSku> {
        fn part(s: &str, min: usize, max: usize) -> bool {
            (min..=max).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        }
        let valid = match text.split_once('-') {
            Some((base, variant)) => part(base, 3, 12) && part(variant, 1, 6),
            None => part(text, 3, 12),
        };
        if valid {
            Ok(Sku(text.to_owned()))
        } else {
            Err(InvalidSku(text.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sku {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Sku {
    type Error = InvalidSku;
    fn try_from(text: String) -> Result<Self, Self::Error> {
        Sku::parse(&text)
    }
}

impl From<Sku> for String {
    fn from(sku: Sku) -> String {
        sku.0
    }
}

/// `text`, `varchar` and friends, or the `sku` domain over one of them.
fn accepts_sku(ty: &Type) -> bool {
    match ty.kind() {
        Kind::Domain(base) => ty.name() == "sku" && <&str as ToSql>::accepts(base),
        _ => <&str as ToSql>::accepts(ty),
    }
}

impl ToSql for Sku {
    fn to_sql(
        &self,
        _ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        // The binary format of `text` (and so of a domain over it) is just the UTF-8 bytes.
        out.extend_from_slice(self.0.as_bytes());
        Ok(IsNull::No)
    }

    fn accepts(ty: &Type) -> bool {
        accepts_sku(ty)
    }

    to_sql_checked!();
}

impl<'a> FromSql<'a> for Sku {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        let text = <&str as FromSql>::from_sql(ty, raw)?;
        // The domain makes a bad SKU in the table impossible; a query can still produce one
        // (`SELECT 'nope' AS sku`). Refusing it here keeps the type's promise.
        Ok(Sku::parse(text)?)
    }

    fn accepts(ty: &Type) -> bool {
        accepts_sku(ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_base_and_variant_forms() {
        for ok in ["ABC", "CHAIR01", "CHAIR-01", "A1B2C3D4E5F6-XYZ123"] {
            assert!(Sku::parse(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "AB",
            "chair",
            "CHAIR-",
            "-01",
            "CHAIR-01-02",
            "ABCDEFGHIJKLM",
            "CHAIR-1234567",
            "CHAIR 01",
        ] {
            assert!(Sku::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn serde_validates() {
        assert!(serde_json::from_str::<Sku>("\"CHAIR-01\"").is_ok());
        assert!(serde_json::from_str::<Sku>("\"chair\"").is_err());
    }
}
