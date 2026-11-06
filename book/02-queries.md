# Chapter 2 — Running SQL: execute, query and parameters

> **Async PostgreSQL in Rust** · Previous: [Chapter 1 — Connecting](01-connecting.md) ·
> Next: [Chapter 3 — Rows and the type mapping](03-rows-and-types.md)

A `Client` has about twenty methods that run SQL. They look like a long list to learn, but they
answer only three questions: do you want rows back, how many, and how should your values
travel? This chapter covers the everyday ones: `batch_execute` for scripts, `execute` for
changes, `query` and its *one*, *opt* and *scalar* variants for reading, and `simple_query` for
when you need to see everything as text.

The most important idea in the chapter is not a method but a rule. **Values never go into the
SQL text.** They travel separately, as parameters. The rule makes SQL injection impossible,
and tokio-postgres's API is built around it.

---

## 2.1 `batch_execute`: scripts

`batch_execute` takes a string with any number of statements, runs them all, and returns
nothing but success or failure. It is for schema scripts, migrations and setting things up,
and the examples in this chapter use it to create the tables they need:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;

    // Several statements in one string: a table, and some data to start with.
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 sku         text   PRIMARY KEY,
                 name        text   NOT NULL,
                 price_cents bigint NOT NULL CHECK (price_cents > 0)
             );
             INSERT INTO products VALUES ('CHAIR-01', 'Garden chair', 4999);
             INSERT INTO products VALUES ('LAMP-01', 'Desk lamp', 2999);",
        )
        .await?;

    // The statements of one batch run in one implicit transaction: when one fails, the
    // ones before it are undone too.
    let failed = client
        .batch_execute(
            "INSERT INTO products VALUES ('VASE-01', 'Vase', 1999);
             INSERT INTO products VALUES ('RUG-01', 'Rug', 0);", // breaks the CHECK
        )
        .await;
    assert!(failed.is_err());
    let count: i64 = client
        .query_one("SELECT count(*) FROM products", &[])
        .await?
        .get(0);
    assert_eq!(count, 2, "the vase was rolled back with the rug");
    Ok(())
}
```

`CREATE TEMP TABLE` makes a table that exists only for this session and disappears when the
connection closes. That is why the book's examples can run in any order without cleaning up
after themselves.

> **SQL lens:** when one message carries several statements, PostgreSQL runs them as a single
> transaction unless they contain transaction commands of their own (`BEGIN`, `COMMIT`). That is
> the behaviour above, and it is often what you want from a script: all of it or none of it.
> One exception: statements that cannot run inside a transaction, such as
> `CREATE INDEX CONCURRENTLY` and `VACUUM`, cannot run in a multi-statement batch either.

`batch_execute` takes no parameters. That is the price of running several statements at
once, and it makes `batch_execute` the wrong tool for anything that includes a value from
outside your program. §2.3 explains why, and §2.7 why it is built that way.

## 2.2 `execute`: changes, counted

`execute` runs one statement and returns how many rows it affected, as a `u64`:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let created = client
        .execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, name text, price_cents bigint)",
            &[],
        )
        .await?;
    assert_eq!(created, 0, "DDL affects no rows");

    let inserted = client
        .execute(
            "INSERT INTO products VALUES
                 ('CHAIR-01', 'Garden chair', 4999), ('LAMP-01', 'Desk lamp', 2999),
                 ('RUG-01', 'Rug', 8999)",
            &[],
        )
        .await?;
    assert_eq!(inserted, 3);

    // 10% off everything over 30.00: two rows.
    let discounted = client
        .execute(
            "UPDATE products SET price_cents = price_cents * 9 / 10 WHERE price_cents > $1",
            &[&3000_i64],
        )
        .await?;
    assert_eq!(discounted, 2);

    // Deleting something that is not there is not an error: zero rows.
    let deleted = client
        .execute("DELETE FROM products WHERE sku = $1", &[&"VASE-01"])
        .await?;
    assert_eq!(deleted, 0);
    Ok(())
}
```

The count is what the server reports when the statement completes: rows inserted, updated or
deleted, rows returned by a `SELECT`, zero for everything else. The count is often the whole
answer. "Was the order cancelled?" is `execute(…) == 1`, and Chapter 8 uses exactly that for
optimistic locking.

`&[&3000_i64]` is the first parameter, `$1`. It is time to look at those properly.

## 2.3 Parameters

A statement refers to its parameters by position, `$1`, `$2` and so on, and the values follow
in a slice, in the same order:

~~~rust
client.execute(
    "UPDATE products SET price_cents = $2 WHERE sku = $1",
    &[&sku, &new_price],
).await?;
~~~

The SQL text and the values go to the server **in different protocol messages**. The server
parses the text first, with `$1` and `$2` as placeholders, and only then receives the values,
as data. A value can contain quotes, semicolons or `DROP TABLE` and remain a value. Here is the
difference it makes, with a value a user typed into a form:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE reviews (product text, body text, author text);
             INSERT INTO reviews VALUES ('CHAIR-01', 'Sturdy.', 'Ada'),
                                        ('CHAIR-01', 'Comfy.', 'Bob');",
        )
        .await?;

    // What someone typed into the "your name" field.
    let author = "Eve'); DELETE FROM reviews; --";

    // As a parameter, the text is a value, whatever it contains.
    client
        .execute(
            "INSERT INTO reviews (product, body, author) VALUES ($1, $2, $3)",
            &[&"CHAIR-01", &"Nice.", &author],
        )
        .await?;
    let count: i64 = client
        .query_one("SELECT count(*) FROM reviews", &[])
        .await?
        .get(0);
    assert_eq!(count, 3);

    // Pasted into the SQL text, the text *is* SQL. Never do this.
    let sql = format!(
        "INSERT INTO reviews (product, body, author) VALUES ('CHAIR-01', 'Nice.', '{author}')"
    );
    client.batch_execute(&sql).await?;
    let count: i64 = client
        .query_one("SELECT count(*) FROM reviews", &[])
        .await?
        .get(0);
    assert_eq!(count, 0, "every review is gone");
    Ok(())
}
```

The formatted string became three statements: an `INSERT`, a `DELETE FROM reviews`, and a
comment that swallowed the rest. Escaping quotes yourself is not the fix; there are more ways
to break out of a string literal than quotes. Parameters are the fix.

> **Rust refresher:** the parameter slice has the type `&[&(dyn ToSql + Sync)]`: a slice of
> references to *trait objects*. Each element can be a different type (`&String`, `&i64`,
> `&Option<bool>`), as long as it implements `ToSql`, the trait that encodes a Rust value in
> PostgreSQL's format (Chapters 3 and 5). They are references because the values stay where
> they are; the slice only points at them. `Sync` is there because the query's future holds
> these references across `.await`, and for a future to be `Send` (spawnable), everything it
> borrows must be `Sync`. You rarely write the type out; `&[&a, &b]` coerces to it.

You do write it out when the parameters are not known in advance, for instance a search with
optional filters. The SQL text grows one placeholder per filter, and the values go into a
`Vec`:

```rust
use tokio_postgres::Client;
use tokio_postgres::types::ToSql;

/// SKUs of the products matching the filters that are set.
async fn search(
    client: &Client,
    name: Option<&str>,
    max_price: Option<i64>,
) -> Result<Vec<String>, tokio_postgres::Error> {
    let mut sql = String::from("SELECT sku FROM products WHERE true");
    let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
    let pattern; // declared out here: `params` borrows it after the `if` ends
    if let Some(name) = name {
        pattern = format!("%{name}%");
        params.push(&pattern);
        sql += &format!(" AND name ILIKE ${}", params.len());
    }
    if let Some(max) = &max_price {
        params.push(max);
        sql += &format!(" AND price_cents <= ${}", params.len());
    }
    sql += " ORDER BY sku";
    let rows = client.query(&sql, &params).await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, name text, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 'Garden chair', 4999),
                 ('CHAIR-02', 'Office chair', 12999), ('LAMP-01', 'Desk lamp', 2999);",
        )
        .await?;
    assert_eq!(search(&client, None, None).await?.len(), 3);
    assert_eq!(
        search(&client, Some("chair"), None).await?,
        ["CHAIR-01", "CHAIR-02"]
    );
    assert_eq!(
        search(&client, Some("chair"), Some(5000)).await?,
        ["CHAIR-01"]
    );
    assert_eq!(search(&client, None, Some(3000)).await?, ["LAMP-01"]);
    Ok(())
}
```

`format!` appears there too, but notice what it inserts: placeholders (`$1`, `$2`), never
values. Building SQL *structure* in Rust is fine; putting *data* into it is not.

The number of values must match the number of placeholders. tokio-postgres checks before
sending anything, and fails with "expected 2 parameters but got 1".

### Identifiers are not values

Parameters stand for values, not for parts of the statement's structure. A table name, a
column name or a sort direction cannot be a parameter: `SELECT * FROM $1` is a syntax error
(SQLSTATE 42601), because the server must know which table it is planning for before any value
arrives. When structure must vary, choose it from a fixed list in Rust:

~~~rust
/// The columns a client may sort products by.
fn sort_column(requested: &str) -> Option<&'static str> {
    match requested {
        "sku" => Some("sku"),
        "price" => Some("price_cents"),
        "name" => Some("name"),
        _ => None,
    }
}
~~~

Only when the name genuinely comes from outside, as with the channel names a user gives
[pglisten](../apps/pglisten/src/lib.rs), quote it as an identifier: in double quotes, with any
double quote doubled. That is `quote_ident`, ten characters of code and the one place in this
repository where outside text enters SQL.

> **Pitfall:** `LIMIT`, `OFFSET`, and values in `WHERE`, `SET` or `VALUES` *can* be parameters;
> table and column names, `ASC`/`DESC`, and SQL keywords cannot. If you find yourself writing
> `format!` with a variable that is not a placeholder number, stop and ask which kind it is.

## 2.4 `query`, `query_one` and `query_opt`: reading

`query` runs a statement and collects every row into a `Vec<Row>`. Two variants say how many
rows you expect, and turn a surprise into an error:

| Method | Returns | Fails when |
|---|---|---|
| `query` | `Vec<Row>` | never, because of the row count |
| `query_one` | `Row` | there is not exactly one row |
| `query_opt` | `Option<Row>` | there is more than one row |

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, name text, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 'Garden chair', 4999),
                                         ('LAMP-01', 'Desk lamp', 2999);",
        )
        .await?;

    let all = client
        .query("SELECT sku, price_cents FROM products ORDER BY sku", &[])
        .await?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].get::<_, &str>("sku"), "LAMP-01");

    let chair = client
        .query_one("SELECT name FROM products WHERE sku = $1", &[&"CHAIR-01"])
        .await?;
    assert_eq!(chair.get::<_, &str>("name"), "Garden chair");

    let vase = client
        .query_opt("SELECT name FROM products WHERE sku = $1", &[&"VASE-01"])
        .await?;
    assert!(vase.is_none());

    // Two rows where one was promised: an error, not a silently ignored second row.
    let error = client
        .query_one("SELECT sku FROM products", &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "query returned an unexpected number of rows"
    );
    assert!(
        error.as_db_error().is_none(),
        "the server did nothing wrong"
    );
    Ok(())
}
```

Use `query_one` when a missing row is a bug (a count, a row you just inserted) and `query_opt`
when it is an answer ("is there a product with this SKU?"). Mapping "not found" to a 404 starts
with choosing `query_opt`. Note that the row-count error is the client's own: there is no
SQLSTATE to match on, and Chapter 6 shows how to tell it apart from other errors.

> **Pitfall:** `query_one` does not add `LIMIT 1` to your statement. It reads every row the
> server sends and then complains if there was more than one. A query that can match many
> rows when you want the first needs `ORDER BY … LIMIT 1` in the SQL.

## 2.5 Scalars: `query_scalar`, `query_one_scalar`, `query_opt_scalar`

Many queries return a single column, and taking it out of each row is boilerplate. Since 0.7.16,
`Client` has shortcuts that return the first column directly:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999), ('LAMP-01', 2999);",
        )
        .await?;

    let count: i64 = client
        .query_one_scalar("SELECT count(*) FROM products", &[])
        .await?;
    let skus: Vec<String> = client
        .query_scalar("SELECT sku FROM products ORDER BY sku", &[])
        .await?;
    let price: Option<i64> = client
        .query_opt_scalar(
            "SELECT price_cents FROM products WHERE sku = $1",
            &[&"VASE-01"],
        )
        .await?;

    assert_eq!(count, 2);
    assert_eq!(skus, ["CHAIR-01", "LAMP-01"]);
    assert_eq!(price, None);
    Ok(())
}
```

The value type must own its data (`String`, not `&str`), because the row it came from is
dropped before the method returns. In 0.7.18 the scalar methods exist only on `Client`, not on
`Transaction` or `GenericClient` (Chapter 8), so code that must also run inside a transaction
uses `query_one(…)?.get(0)`.

## 2.6 `RETURNING`: changing and reading in one statement

An `INSERT`, `UPDATE` or `DELETE` can return the rows it touched, with `RETURNING`. Run such a
statement with `query` (or `query_one`) instead of `execute`, and you get the generated id, the
default timestamp or the new price without a second query:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id          bigint GENERATED ALWAYS AS IDENTITY,
                 sku         text   PRIMARY KEY,
                 price_cents bigint NOT NULL
             )",
        )
        .await?;

    // The id is made by the server; RETURNING hands it back.
    let id: i64 = client
        .query_one(
            "INSERT INTO products (sku, price_cents) VALUES ($1, $2) RETURNING id",
            &[&"CHAIR-01", &4999_i64],
        )
        .await?
        .get("id");
    assert_eq!(id, 1);

    // An upsert: insert, or update the price if the SKU exists. PG 18's RETURNING old and
    // new say which one happened, and what the price was before.
    let upsert = "INSERT INTO products (sku, price_cents) VALUES ($1, $2)
                  ON CONFLICT (sku) DO UPDATE SET price_cents = EXCLUDED.price_cents
                  RETURNING old.sku IS NULL AS inserted, old.price_cents AS was";
    let row = client.query_one(upsert, &[&"LAMP-01", &2999_i64]).await?;
    assert!(row.get::<_, bool>("inserted"));
    let row = client.query_one(upsert, &[&"CHAIR-01", &3999_i64]).await?;
    assert!(!row.get::<_, bool>("inserted"));
    assert_eq!(row.get::<_, Option<i64>>("was"), Some(4999));
    Ok(())
}
```

`RETURNING old.…` and `new.…` are new in PostgreSQL 18. Before 18, the usual trick to tell an
inserted row from an updated one in an upsert is `RETURNING (xmax = 0) AS inserted`, which
relies on an implementation detail of how PostgreSQL marks row versions. It works, and it is
worth replacing when you can.

## 2.7 The simple query protocol

Every method so far uses PostgreSQL's *extended* query protocol: the statement and its values
travel in separate messages, and results come back in binary, typed. PostgreSQL has a second,
older protocol, the *simple* one, in which a single message carries one or more statements as
text, with no parameters, and every value comes back as text. `batch_execute` uses it, which is
why it can run several statements and cannot take parameters.

`simple_query` uses it too, and returns everything the server sent back:

```rust
use tokio_postgres::SimpleQueryMessage;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999), ('LAMP-01', NULL);",
        )
        .await?;

    let messages = client
        .simple_query(
            "SELECT sku, price_cents FROM products ORDER BY sku;
             UPDATE products SET price_cents = 1 WHERE price_cents IS NULL",
        )
        .await?;

    let mut rows = Vec::new();
    let mut completed = Vec::new();
    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                assert_eq!(columns[1].name(), "price_cents");
            }
            // Every value is `Option<&str>`: text, or None for NULL. No types.
            SimpleQueryMessage::Row(row) => {
                rows.push((row.get(0).map(str::to_owned), row.get(1).map(str::to_owned)));
            }
            // One per statement: rows returned or affected.
            SimpleQueryMessage::CommandComplete(count) => completed.push(count),
            _ => {} // the enum is non-exhaustive
        }
    }
    assert_eq!(rows[0], (Some("CHAIR-01".into()), Some("4999".into())));
    assert_eq!(rows[1], (Some("LAMP-01".into()), None));
    assert_eq!(completed, [2, 1]);
    Ok(())
}
```

`SimpleQueryMessage` has three variants. `RowDescription` (since 0.7.11) arrives before the rows
of each statement that returns rows, with the column names. Without it, a `SELECT` that
matched nothing would leave you no way to print its column headers. `Row` holds one row of
text. `CommandComplete` carries a statement's count. It carries only the count: the command
tag the server sends ("UPDATE 1", "CREATE TABLE") is not exposed, which is why the tiny psql in
this repository prints "OK, 0 affected" where psql prints "CREATE TABLE".

When would you use it?

- **Tools that run SQL they did not write**: a migration runner, an admin console, a psql of
  your own. They need several statements per string and do not know the result types in
  advance; text suits them. [pgq](../apps/pgq/src/lib.rs) is built on it.
- **Commands that do not take parameters anyway**: `SET`, `LISTEN`, `VACUUM`, DDL.
- **Some connection poolers.** PgBouncer in its older modes could not carry the extended
  protocol's named statements across transactions, and the simple protocol always works
  (Chapters 4 and 13).

For everything with values from outside, use the extended protocol, that is, every method in
§2.2 to §2.6.

> **Protocol lens:** the simple protocol is one round trip: a *Query* message with the text,
> answered by a *RowDescription*, *DataRow*s and a *CommandComplete* for each statement, then
> *ReadyForQuery*. The extended protocol splits the work into *Parse* (the text, with
> placeholders), *Bind* (the values), *Describe* and *Execute*, which is what makes parameters
> and binary values possible, and it needs one round trip more unless the statement was
> prepared before. Chapter 4 counts them.

### Side by side — inserting a row and reading it back

The task: insert a product given as three values, and get back what was stored.

```sql
-- psql: plain SQL with literals...
tpbook=# INSERT INTO products (sku, name, price_cents) VALUES ('VASE-01', 'Vase', 1999)
tpbook-#   RETURNING sku, price_cents;
   sku   | price_cents
---------+-------------
 VASE-01 |        1999
(1 row)

INSERT 0 1
-- ...or, since psql 16, real parameters with \bind (the extended protocol):
tpbook=# INSERT INTO products (sku, name, price_cents) VALUES ($1, $2, $3)
tpbook-#   RETURNING sku, price_cents \bind 'RUG-01' 'Rug' 8999 \g
```

~~~rust
// sqlx (illustration)
let (sku, price): (String, i64) = sqlx::query_as(
    "INSERT INTO products (sku, name, price_cents) VALUES ($1, $2, $3) RETURNING sku, price_cents",
)
.bind("VASE-01")
.bind("Vase")
.bind(1999_i64)
.fetch_one(&pool)
.await?;
~~~

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, name text, price_cents bigint)",
        )
        .await?;
    let row = client
        .query_one(
            "INSERT INTO products (sku, name, price_cents) VALUES ($1, $2, $3)
             RETURNING sku, price_cents",
            &[&"VASE-01", &"Vase", &1999_i64],
        )
        .await?;
    let (sku, price): (&str, i64) = (row.get("sku"), row.get("price_cents"));
    assert_eq!((sku, price), ("VASE-01", 1999));
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | `$n` placeholders and values sent separately (with `\bind`) | `$n` placeholders, values sent separately | `$n` placeholders, values sent separately |
| **Different** | Without `\bind`, values are literals in the text; psql variables (`:'name'`) are quoted and pasted in by psql itself | Values attached one by one with `.bind()`. The `query!` macros check the SQL and types at compile time, against a live database | Values in a slice of references, checked at run time against the types the server reports for the statement. Rows are read by name or position with typed `get` (Chapter 3) |

---

## In stockroom

Every repository function in stockroom follows the same pattern: the SQL written out in full,
the values as parameters, `RETURNING` instead of a second query. Creating a store:

~~~rust
// stockroom/src/repo/stores.rs (excerpt)
pub async fn create(db: &impl GenericClient, new: &NewStore) -> Result<Store, AppError> {
    let stmt = db
        .prepare_cached(
            "INSERT INTO stores (code, name, address) VALUES ($1, $2, $3)
             RETURNING id, code, name, address, created_at",
        )
        .await?;
    let row = db
        .query_one(&stmt, &[&new.code, &new.name, &new.address])
        .await?;
    Ok(Store::try_from(&row)?)
}
~~~

Three things in it come later. `prepare_cached` is Chapter 4 and 13 (preparing a statement once
per connection). `&new.address` is a Rust struct passed as a PostgreSQL composite value
(Chapter 5). `GenericClient` lets the same function run on a pooled connection or inside a
transaction (Chapter 8).

The one place stockroom uses the simple protocol is a diagnostic: `GET /admin/sessions` lists
the database's sessions straight from `pg_stat_activity`, and only displays them, so text is
exactly what it wants:

~~~rust
// stockroom/src/repo/reports.rs (excerpt)
let messages = client
    .simple_query(
        "SELECT pid, application_name, backend_type, state,
                wait_event_type || ':' || wait_event AS waiting_on,
                date_trunc('second', now() - query_start) AS running_for,
                left(query, 120) AS query
         FROM pg_stat_activity
         WHERE datname = current_database()
         ORDER BY pid",
    )
    .await?;
let mut sessions = Vec::new();
for message in messages {
    if let SimpleQueryMessage::Row(row) = message {
        let mut session = serde_json::Map::new();
        for (i, column) in row.columns().iter().enumerate() {
            // `Option<&str>`: NULL is `None`; everything else is text, whatever its SQL type.
            session.insert(column.name().to_owned(), row.get(i).into());
        }
        sessions.push(session);
    }
}
~~~

The JSON this produces has `"pid": "60027"`, a string, where a typed query would give a number:
that is the simple protocol's trade.

## Summary

- `batch_execute` runs a script of several statements in one implicit transaction. No
  parameters, no results.
- `execute` returns the number of rows affected (or returned); 0 for DDL.
- `query` returns all rows; `query_one` exactly one; `query_opt` zero or one. A wrong row count
  is an error from the client, with no SQLSTATE.
- `query_scalar`, `query_one_scalar` and `query_opt_scalar` (0.7.16+, `Client` only) return
  the first column of each row.
- Values go in parameters, `$1…$n`, never into the SQL text. Building SQL *structure* with
  `format!` is fine; table names, column names and keywords come from allowlists, or
  `quote_ident` as a last resort.
- A dynamic parameter list is a `Vec<&(dyn ToSql + Sync)>`, with the values it points at
  living at least as long.
- `RETURNING` reads what a change produced. PG 18's `RETURNING old/new` shows before and after.
- `simple_query` uses the simple protocol: several statements, no parameters, all text, with
  `RowDescription`, `Row` and `CommandComplete` messages.

## Exercises

1. **A price change.** Write `async fn reprice(client: &Client, percent: i64, min_price: i64)
   -> Result<u64, Error>` that raises every price at or above `min_price` by `percent` percent
   and returns how many products changed.
   *Hint:* integer arithmetic in SQL: `price_cents * (100 + $1) / 100`.
2. **One more filter.** Extend `search` from §2.3 with `min_price: Option<i64>`. Make sure the
   placeholder numbers stay right in every combination of filters.
   *Hint:* the number of each placeholder is `params.len()` right after the push.
3. **Spot the injection.** A colleague wrote this for a "sort by" dropdown. What can a
   client do with it, and how do you fix it?

   ~~~rust
   let direction = query_params.get("dir").unwrap_or("asc");
   let sql = format!("SELECT sku FROM products ORDER BY price_cents {direction}");
   client.query(&sql, &[]).await?
   ~~~

4. **Found, or not.** Write `async fn price_of(client: &Client, sku: &str) ->
   Result<Option<i64>, Error>`, which returns `Ok(None)` for an unknown SKU and an error only
   when something actually went wrong.
   *Hint:* one of the scalar methods does exactly this.
5. **A tiny psql.** Use `simple_query` to run a script typed by a user and print, for each
   statement, either its rows or "OK, n affected". Compare your version with `run` in
   [apps/pgq/src/lib.rs](../apps/pgq/src/lib.rs).

### Solutions

**3.** `direction` is pasted into the SQL, so a client can send anything:
`?dir=asc; DELETE FROM products` does not run here only because `query` uses the extended
protocol, which allows one statement per message (the server refuses with "cannot insert
multiple commands into a prepared statement"). But `?dir=, (SELECT …)` can still add
subqueries to the sort, and a later refactoring to `batch_execute` would make the first attack
work too. A sort direction is structure, not a value, so it cannot be a parameter. Map it from
an allowlist:

~~~rust
let direction = match query_params.get("dir") {
    Some("desc") => "DESC",
    _ => "ASC",
};
let sql = format!("SELECT sku FROM products ORDER BY price_cents {direction}");
~~~

Now `direction` is always one of two string literals in your binary.

**4.**

```rust
use tokio_postgres::{Client, Error};

async fn price_of(client: &Client, sku: &str) -> Result<Option<i64>, Error> {
    client
        .query_opt_scalar("SELECT price_cents FROM products WHERE sku = $1", &[&sku])
        .await
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999);",
        )
        .await?;
    assert_eq!(price_of(&client, "CHAIR-01").await?, Some(4999));
    assert_eq!(price_of(&client, "NOPE-01").await?, None);
    Ok(())
}
```

`query_opt_scalar` returns `Ok(None)` for no row and an error for anything else, including
two rows (impossible here, because `sku` is the primary key).

---

Previous: [Chapter 1 — Connecting](01-connecting.md) ·
Next: [Chapter 3 — Rows and the type mapping](03-rows-and-types.md)
