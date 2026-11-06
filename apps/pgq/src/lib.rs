//! pgq: the parts of a tiny psql (Chapters 2, 4 and 9).
//!
//! * [`run`] sends SQL through the **simple query protocol** (`simple_query`): any number of
//!   statements in one string, no parameters, every value back as text. That is what psql
//!   itself does, and why `SimpleQueryMessage` has a `RowDescription`: without it, a `SELECT`
//!   that returns no rows would have no column names to print.
//! * [`stream`] does the same with `simple_query_raw`, printing rows as they arrive.
//! * [`describe`] uses the **extended protocol** instead: `prepare` asks the server to parse
//!   one statement and say what types its parameters and columns have, without running it.

use std::fmt::Write as _;
use std::pin::pin;

use futures_util::TryStreamExt;
use tokio_postgres::{Client, Error, SimpleQueryMessage};

/// Rows of text, to be printed the way psql prints them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl Table {
    /// ```text
    ///  id | name
    /// ----+-------
    ///  1  | chair
    /// ```
    /// NULL prints as nothing, as in psql.
    pub fn render(&self) -> String {
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.chars().count()).collect();
        for row in &self.rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.as_deref().unwrap_or("").chars().count());
            }
        }
        let line = |cells: Vec<&str>| -> String {
            let padded: Vec<String> = cells
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!(" {cell:<width$} "))
                .collect();
            padded.join("|").trim_end().to_owned() + "\n"
        };
        let mut out = line(self.headers.iter().map(String::as_str).collect());
        out += &widths
            .iter()
            .map(|w| "-".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("+");
        out.push('\n');
        for row in &self.rows {
            out += &line(
                row.iter()
                    .map(|cell| cell.as_deref().unwrap_or(""))
                    .collect(),
            );
        }
        out
    }
}

/// Runs `sql` (one or more statements) and returns what psql would print.
pub async fn run(client: &Client, sql: &str) -> Result<String, Error> {
    let mut out = String::new();
    let mut table: Option<Table> = None;
    for message in client.simple_query(sql).await? {
        match message {
            // Sent before the rows of each statement that returns rows (0.7.10 and later).
            SimpleQueryMessage::RowDescription(columns) => {
                let headers = columns.iter().map(|c| c.name().to_owned()).collect();
                table = Some(Table {
                    headers,
                    rows: Vec::new(),
                });
            }
            SimpleQueryMessage::Row(row) => {
                let cells = (0..row.len())
                    .map(|i| row.get(i).map(str::to_owned))
                    .collect();
                table.get_or_insert_with(Table::default).rows.push(cells);
            }
            // One per statement: rows returned (SELECT) or affected (INSERT, UPDATE, …).
            SimpleQueryMessage::CommandComplete(count) => match table.take() {
                Some(table) => {
                    let plural = if count == 1 { "" } else { "s" };
                    let _ = writeln!(out, "{}({count} row{plural})\n", table.render());
                }
                None => {
                    let _ = writeln!(out, "OK, {count} affected\n");
                }
            },
            _ => {} // `SimpleQueryMessage` is non-exhaustive
        }
    }
    Ok(out)
}

/// Runs `sql` and calls `print` with each line as soon as its row arrives: tab-separated, a
/// header line first. Memory use stays flat however many rows come back.
pub async fn stream(client: &Client, sql: &str, mut print: impl FnMut(&str)) -> Result<u64, Error> {
    let mut messages = pin!(client.simple_query_raw(sql).await?);
    let mut rows = 0;
    while let Some(message) = messages.try_next().await? {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                let names: Vec<&str> = columns.iter().map(|c| c.name()).collect();
                print(&names.join("\t"));
            }
            SimpleQueryMessage::Row(row) => {
                let cells: Vec<&str> = (0..row.len()).map(|i| row.get(i).unwrap_or("")).collect();
                print(&cells.join("\t"));
                rows += 1;
            }
            _ => {}
        }
    }
    Ok(rows)
}

/// What the server says about one statement without running it: the type of each parameter
/// and the name and type of each result column.
pub async fn describe(client: &Client, sql: &str) -> Result<String, Error> {
    let statement = client.prepare(sql).await?;
    let mut out = String::new();
    let params = Table {
        headers: vec!["parameter".into(), "type".into()],
        rows: statement
            .params()
            .iter()
            .enumerate()
            .map(|(i, ty)| vec![Some(format!("${}", i + 1)), Some(ty.name().to_owned())])
            .collect(),
    };
    let columns = Table {
        headers: vec!["column".into(), "type".into(), "from table".into()],
        rows: statement
            .columns()
            .iter()
            .map(|c| {
                vec![
                    Some(c.name().to_owned()),
                    Some(c.type_().name().to_owned()),
                    c.table_oid()
                        .map(|oid| format!("oid {oid}, column {}", c.column_id().unwrap_or(0))),
                ]
            })
            .collect(),
    };
    if !params.rows.is_empty() {
        out += &params.render();
        out.push('\n');
    }
    out += &columns.render();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_like_psql() {
        let table = Table {
            headers: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Some("1".into()), Some("chair".into())],
                vec![Some("10".into()), None],
            ],
        };
        assert_eq!(
            table.render(),
            " id | name\n----+-------\n 1  | chair\n 10 |\n"
        );
    }
}
