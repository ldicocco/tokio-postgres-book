//! SQL text that the derives write at macro time.

/// `name` as a quoted SQL identifier: `"name"`, with any `"` inside it doubled. Quoting keeps
/// the name exactly as written (PostgreSQL folds unquoted names to lower case) and lets a table
/// be called `order` or a column `type`. The main book's `quote_ident` (its Chapter 2), run by
/// the compiler.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The columns, quoted and separated by commas: `"id", "sku"`.
pub fn column_list(columns: &[String]) -> String {
    let quoted: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
    quoted.join(", ")
}

/// `"a" = $1 AND "b" = $2`, numbering from `first`.
pub fn equal_params(columns: &[String], first: usize) -> String {
    let terms: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{} = ${}", quote_ident(c), first + i))
        .collect();
    terms.join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_and_inner_quotes_doubled() {
        assert_eq!(quote_ident("order"), r#""order""#);
        assert_eq!(quote_ident("unitPrice"), r#""unitPrice""#);
        assert_eq!(quote_ident(r#"say "hi""#), r#""say ""hi""""#);
    }

    #[test]
    fn a_composite_key_becomes_one_condition_per_column() {
        let key = ["store_id".to_owned(), "product_id".to_owned()];
        assert_eq!(
            equal_params(&key, 1),
            r#""store_id" = $1 AND "product_id" = $2"#
        );
    }
}
