//! Rendering a query: SQL text with `$n` placeholders numbered in order, and the parameter list
//! beside it, kept in step by one function that writes both.

use std::fmt::Write;
use std::ops::Range;

use tokio_postgres::types::ToSql;

use super::expr::{Node, Param};

/// A statement being written: its text, and the parameters its `$n` refer to, borrowed from the
/// query that owns them.
pub(crate) struct Sql<'q> {
    pub text: String,
    pub params: Vec<&'q (dyn ToSql + Sync)>,
}

impl<'q> Sql<'q> {
    pub fn new(text: impl Into<String>) -> Self {
        Sql {
            text: text.into(),
            params: Vec::new(),
        }
    }

    /// Appends the next placeholder, `$n`, and its value.
    pub fn param(&mut self, value: &'q (dyn ToSql + Sync)) {
        self.params.push(value);
        write!(self.text, "${}", self.params.len()).expect("writing to a String");
    }

    /// Appends `node` as SQL. Within an `AND` or an `OR`, a child of the other kind is put in
    /// parentheses, and so is raw SQL, whose precedence the builder cannot know.
    pub fn node(&mut self, node: &'q Node) {
        match node {
            Node::Compare { column, op, value } => {
                write!(self.text, "{column} {op} ").expect("writing to a String");
                self.param(boxed(value));
            }
            Node::Between { column, low, high } => {
                write!(self.text, "{column} BETWEEN ").expect("writing to a String");
                self.param(boxed(low));
                self.text.push_str(" AND ");
                self.param(boxed(high));
            }
            Node::Any { column, values } => {
                write!(self.text, "{column} = ANY(").expect("writing to a String");
                self.param(boxed(values));
                self.text.push(')');
            }
            Node::Null { column, negated } => {
                let not = if *negated { " NOT" } else { "" };
                write!(self.text, "{column} IS{not} NULL").expect("writing to a String");
            }
            Node::And(nodes) => self.join(nodes, " AND ", |n| matches!(n, Node::Or(_))),
            Node::Or(nodes) => self.join(nodes, " OR ", |n| matches!(n, Node::And(_))),
            Node::Not(node) => {
                self.text.push_str("NOT (");
                self.node(node);
                self.text.push(')');
            }
            Node::Raw { sql, params } => {
                // The fragment's `$1` is the next parameter of the statement.
                let offset = self.params.len();
                let mut at = 0;
                for (range, n) in placeholders(sql) {
                    self.text.push_str(&sql[at..range.start]);
                    write!(self.text, "${}", offset + n).expect("writing to a String");
                    at = range.end;
                }
                self.text.push_str(&sql[at..]);
                self.params.extend(params.iter().map(boxed));
            }
        }
    }

    fn join(&mut self, nodes: &'q [Node], separator: &str, nested: fn(&Node) -> bool) {
        for (i, node) in nodes.iter().enumerate() {
            if i > 0 {
                self.text.push_str(separator);
            }
            if nested(node) || matches!(node, Node::Raw { .. }) {
                self.text.push('(');
                self.node(node);
                self.text.push(')');
            } else {
                self.node(node);
            }
        }
    }
}

fn boxed(param: &Param) -> &(dyn ToSql + Sync) {
    &**param
}

/// The `$n` placeholders in `sql`, with where each one is and its number. Text that only looks
/// like one is skipped: string literals (`'…'`, `E'…'`, `$tag$…$tag$`), quoted identifiers,
/// comments, and identifiers that contain a `$` (`a$1`).
pub(crate) fn placeholders(sql: &str) -> impl Iterator<Item = (Range<usize>, usize)> + '_ {
    let bytes = sql.as_bytes();
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < bytes.len() {
            let next = bytes.get(i + 1).copied();
            match bytes[i] {
                b'\'' => {
                    // E'…' takes backslash escapes; '' is a quote in either kind.
                    let escapes = i > 0
                        && matches!(bytes[i - 1], b'E' | b'e')
                        && (i < 2 || !is_ident(bytes[i - 2]));
                    i += 1;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'\\' if escapes => i += 2,
                            b'\'' if bytes.get(i + 1) == Some(&b'\'') => i += 2,
                            b'\'' => {
                                i += 1;
                                break;
                            }
                            _ => i += 1,
                        }
                    }
                }
                b'"' => {
                    i += 1;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'"' if bytes.get(i + 1) == Some(&b'"') => i += 2,
                            b'"' => {
                                i += 1;
                                break;
                            }
                            _ => i += 1,
                        }
                    }
                }
                b'-' if next == Some(b'-') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                b'/' if next == Some(b'*') => {
                    // PostgreSQL's block comments nest.
                    let mut depth = 0;
                    while i < bytes.len() {
                        if bytes[i..].starts_with(b"/*") {
                            depth += 1;
                            i += 2;
                        } else if bytes[i..].starts_with(b"*/") {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        } else {
                            i += 1;
                        }
                    }
                }
                b'$' if i > 0 && is_ident(bytes[i - 1]) => i += 1,
                b'$' if next.is_some_and(|b| b.is_ascii_digit()) => {
                    let start = i;
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let n = sql[start + 1..i].parse().unwrap_or(usize::MAX);
                    return Some((start..i, n));
                }
                b'$' => {
                    // A dollar quote, `$$…$$` or `$tag$…$tag$`, or a lone `$`.
                    let tag_end = bytes[i + 1..]
                        .iter()
                        .position(|b| !is_ident(*b) || *b == b'$')
                        .map(|p| i + 1 + p);
                    match tag_end {
                        Some(end) if bytes[end] == b'$' => {
                            let tag = &bytes[i..=end];
                            let body = end + 1;
                            i = bytes[body..]
                                .windows(tag.len())
                                .position(|w| w == tag)
                                .map_or(bytes.len(), |p| body + p + tag.len());
                        }
                        _ => i += 1,
                    }
                }
                _ => i += 1,
            }
        }
        None
    })
}

/// A byte that can continue an SQL identifier (non-ASCII bytes are letters to PostgreSQL).
fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

#[cfg(test)]
mod tests {
    use super::placeholders;

    fn numbers(sql: &str) -> Vec<usize> {
        placeholders(sql).map(|(_, n)| n).collect()
    }

    #[test]
    fn placeholders_are_found_in_order() {
        assert_eq!(
            numbers(r#"lower("sku") = lower($1) OR $2 > $10"#),
            [1, 2, 10]
        );
        let (range, n) = placeholders("x = $12").next().unwrap();
        assert_eq!((range, n), (4..7, 12));
    }

    #[test]
    fn quoted_text_and_comments_are_skipped() {
        assert_eq!(numbers("'$1' = $1"), [1]);
        assert_eq!(numbers("'it''s $2' = $1"), [1]);
        assert_eq!(numbers(r"E'\'$2' = $1"), [1]);
        assert_eq!(numbers(r#""$2" = $1"#), [1]);
        assert_eq!(numbers("$$ $2 $$ = $1"), [1]);
        assert_eq!(numbers("$tag$ $2 $x$ $tag$ = $1"), [1]);
        assert_eq!(numbers("$1 -- $2\n = $3"), [1, 3]);
        assert_eq!(numbers("$1 /* $2 /* $3 */ $4 */ = $5"), [1, 5]);
        assert_eq!(numbers("a$1 = $1"), [1]);
    }
}
