//! Naming conventions for `#[shelf(rename_all = "…")]`, applied to snake_case field names.

/// A convention that turns a snake_case field name into a column name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Case {
    /// The field name as it is: the default, and PostgreSQL's own habit.
    #[default]
    Snake,
    /// `unit_price` → `unitPrice`
    Camel,
    /// `unit_price` → `UnitPrice`
    Pascal,
    /// `unit_price` → `UNIT_PRICE`
    ScreamingSnake,
}

impl Case {
    /// The names `rename_all` accepts, for error messages.
    pub const NAMES: &'static str =
        "\"snake_case\", \"camelCase\", \"PascalCase\" or \"SCREAMING_SNAKE_CASE\"";

    pub fn from_name(name: &str) -> Option<Case> {
        match name {
            "snake_case" => Some(Case::Snake),
            "camelCase" => Some(Case::Camel),
            "PascalCase" => Some(Case::Pascal),
            "SCREAMING_SNAKE_CASE" => Some(Case::ScreamingSnake),
            _ => None,
        }
    }

    pub fn apply(self, field: &str) -> String {
        match self {
            Case::Snake => field.to_owned(),
            Case::ScreamingSnake => field.to_ascii_uppercase(),
            Case::Camel | Case::Pascal => {
                let mut out = String::with_capacity(field.len());
                let mut upper = self == Case::Pascal;
                for c in field.chars() {
                    if c == '_' {
                        upper = !out.is_empty() || self == Case::Pascal;
                    } else if upper {
                        out.extend(c.to_uppercase());
                        upper = false;
                    } else {
                        out.push(c);
                    }
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Case;

    #[test]
    fn each_case_converts_a_snake_case_name() {
        assert_eq!(Case::Snake.apply("unit_price"), "unit_price");
        assert_eq!(Case::Camel.apply("unit_price_cents"), "unitPriceCents");
        assert_eq!(Case::Pascal.apply("unit_price"), "UnitPrice");
        assert_eq!(Case::ScreamingSnake.apply("unit_price"), "UNIT_PRICE");
        assert_eq!(Case::Camel.apply("sku"), "sku");
        assert_eq!(Case::Camel.apply("_private"), "private");
    }

    #[test]
    fn names_round_trip() {
        for name in [
            "snake_case",
            "camelCase",
            "PascalCase",
            "SCREAMING_SNAKE_CASE",
        ] {
            assert!(Case::from_name(name).is_some(), "{name}");
        }
        assert_eq!(Case::from_name("kebab-case"), None);
    }
}
