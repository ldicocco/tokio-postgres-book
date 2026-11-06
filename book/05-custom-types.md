# Chapter 5 — Custom types: ToSql and FromSql

> **Async PostgreSQL in Rust** · Previous: [Chapter 4 — Prepared statements and the extended protocol](04-prepared-statements.md) ·
> Next: [Chapter 6 — Errors](06-errors.md)

Chapter 3's table covers the types both languages have in common. But a well-designed schema
has types of its own: an `order_status` enum, an `address` composite, a `sku` domain that only
accepts valid codes. A well-designed Rust program has its own too: a `Cents` newtype that
cannot be confused with a quantity, an `Email` that is known to be valid. This chapter connects
the two.

There are two ways. You can implement the two conversion traits, `ToSql` and `FromSql`, by
hand. That takes a dozen lines, and it is the way to add validation or to support a type nobody
else supports. Or you can derive them for the common shapes: newtypes, enums, domains and
composites. You will meet both in stockroom, and you will also meet the two things that make
custom types tricky in practice: how domain types are described on the wire, and what happens
when an enum changes underneath a running program.

---

## 5.1 The two traits

`ToSql` turns a Rust value into the bytes of a PostgreSQL value, for a given PostgreSQL type;
`FromSql` does the reverse. Both live in the `postgres-types` crate, re-exported as
`tokio_postgres::types`. Stripped of details:

~~~rust
pub trait ToSql: fmt::Debug {
    /// Write `self` in the binary format of `ty` to `out`.
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn Error + Sync + Send>>;
    /// Can this Rust type be sent as a value of `ty`?
    fn accepts(ty: &Type) -> bool;
    /// `to_sql`, after checking `accepts`; written for you by the `to_sql_checked!()` macro.
    fn to_sql_checked(&self, ty: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn Error + Sync + Send>>;
}

pub trait FromSql<'a>: Sized {
    /// Read a value of `ty` from its binary format in `raw`.
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>>;
    /// Can a value of `ty` be read as this Rust type?
    fn accepts(ty: &Type) -> bool;
    /// What NULL means. The default is an error (`WasNull`); `Option<T>` overrides it.
    fn from_sql_null(ty: &Type) -> Result<Self, Box<dyn Error + Sync + Send>> { … }
}
~~~

Three things are worth noting before you implement them.

- **`accepts` is the type check** you met in Chapter 3. tokio-postgres calls it with the type
  the server described, before encoding a parameter or decoding a column, and turns a `false`
  into the "cannot convert between the Rust type … and the Postgres type …" error.
- **Everything is binary.** tokio-postgres asks for binary values, so `raw` is the type's
  binary representation (big-endian integers, UTF-8 text, and so on), not text. You rarely
  write it by hand: delegate to the implementation of a type that already knows the format.
- **Errors are boxed.** Any error type converts into `Box<dyn Error + Sync + Send>`, including a
  `String`. tokio-postgres wraps yours as the cause of its own error.

## 5.2 A newtype by hand

The most common custom type is a validated newtype: an `Email` is a `String` that has been
checked. It travels as text, so the implementation delegates encoding and decoding to `&str`
and adds the check on the way out of the database as well as on the way in:

```rust
use bytes::BytesMut;
use postgres_types::{FromSql, IsNull, ToSql, Type, to_sql_checked};
use std::error::Error;

/// An email address, known to be well-formed.
#[derive(Debug, Clone, PartialEq)]
struct Email(String);

impl Email {
    fn parse(text: &str) -> Result<Email, String> {
        match text.split_once('@') {
            Some((user, host)) if !user.is_empty() && host.contains('.') => {
                Ok(Email(text.to_owned()))
            }
            _ => Err(format!("{text:?} is not an email address")),
        }
    }
}

impl ToSql for Email {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        self.0.as_str().to_sql(ty, out) // text's binary format: the UTF-8 bytes
    }

    fn accepts(ty: &Type) -> bool {
        <&str as ToSql>::accepts(ty) // text, varchar, …
    }

    to_sql_checked!();
}

impl<'a> FromSql<'a> for Email {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        let text = <&str as FromSql>::from_sql(ty, raw)?;
        Ok(Email::parse(text)?) // a bad value in the database is an error, not an Email
    }

    fn accepts(ty: &Type) -> bool {
        <&str as FromSql>::accepts(ty)
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE customers (id int, email text)")
        .await?;

    let ada = Email::parse("ada@example.com").expect("valid");
    client
        .execute("INSERT INTO customers VALUES (1, $1)", &[&ada])
        .await?;
    let back: Email = client
        .query_one("SELECT email FROM customers WHERE id = 1", &[])
        .await?
        .get(0);
    assert_eq!(back, ada);

    // Something not written by this program: the check runs on the way out too.
    client
        .execute("INSERT INTO customers VALUES (2, 'not an address')", &[])
        .await?;
    let row = client
        .query_one("SELECT email FROM customers WHERE id = 2", &[])
        .await?;
    let error = row.try_get::<_, Email>("email").unwrap_err();
    let cause = Error::source(&error).expect("our message").to_string();
    assert_eq!(cause, "\"not an address\" is not an email address");

    // accepts() keeps it to text-like types.
    let refused = client
        .execute("SELECT $1::int4", &[&ada])
        .await
        .unwrap_err();
    assert_eq!(refused.to_string(), "error serializing parameter 0");
    Ok(())
}
```

`to_sql_checked!()` writes the third method of `ToSql` for you: it calls `accepts`, returns
the "cannot convert" error if it says no, and otherwise calls `to_sql`. Every hand-written
`ToSql` ends with it.

> **Rust refresher:** `<&str as ToSql>::accepts(ty)` is the fully qualified way to call a trait
> function on a specific type: "`accepts`, as `&str` implements it in `ToSql`". You need it
> because `&str` implements both `ToSql::accepts` and `FromSql::accepts`, and the compiler
> cannot guess which one you mean from `<&str>::accepts`.

## 5.3 Deriving: `transparent` newtypes

Most newtypes need no validation at all: they exist so that the compiler can tell a price from
a quantity. For those, derive the traits and say that the type is *transparent*, meaning that
it travels exactly like its single field:

```rust
use postgres_types::{FromSql, ToSql};

/// Money, as a whole number of cents.
#[derive(Debug, Clone, Copy, PartialEq, ToSql, FromSql)]
#[postgres(transparent)]
struct Cents(i64);

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE prices (sku text, price_cents bigint)")
        .await?;
    client
        .execute(
            "INSERT INTO prices VALUES ('CHAIR-01', $1)",
            &[&Cents(4999)],
        )
        .await?;

    let price: Cents = client
        .query_one("SELECT price_cents FROM prices", &[])
        .await?
        .get(0);
    assert_eq!(price, Cents(4999));

    // Transparent all the way down: arrays of Cents are bigint[].
    let doubled: Vec<Cents> = client
        .query_one(
            "SELECT array_agg(x * 2) FROM unnest($1::bigint[]) AS x",
            &[&vec![Cents(1), Cents(2)]],
        )
        .await?
        .get(0);
    assert_eq!(doubled, [Cents(2), Cents(4)]);
    Ok(())
}
```

`Cents` accepts exactly what `i64` accepts, `bigint`, and nothing else: sending it into an
`integer` column is still a type error, which is the point of the newtype.

## 5.4 Enums

A PostgreSQL enum is a type with a fixed list of labels. A Rust enum of unit variants maps onto
it with the derive. The type's name and the labels must match; `rename_all` saves you writing
each label when they follow a convention:

```rust
use postgres_types::{FromSql, ToSql};

#[derive(Debug, Clone, Copy, PartialEq, ToSql, FromSql)]
#[postgres(name = "order_status", rename_all = "lowercase")]
enum OrderStatus {
    Pending,
    Paid,
    Cancelled,
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    // A temporary type, like the temporary tables: it disappears with the session.
    client
        .batch_execute(
            "CREATE TYPE pg_temp.order_status AS ENUM ('pending', 'paid', 'cancelled');
             CREATE TEMP TABLE orders (id int, status order_status);
             INSERT INTO orders VALUES (1, 'pending'), (2, 'paid'), (3, 'cancelled');",
        )
        .await?;

    client
        .execute(
            "UPDATE orders SET status = $1 WHERE id = 1",
            &[&OrderStatus::Paid],
        )
        .await?;

    // An array of enum values for = ANY, and enum values back.
    let open = vec![OrderStatus::Pending, OrderStatus::Paid];
    let rows = client
        .query(
            "SELECT id, status FROM orders WHERE status = ANY($1) ORDER BY id",
            &[&open],
        )
        .await?;
    let found: Vec<(i32, OrderStatus)> = rows
        .iter()
        .map(|row| (row.get("id"), row.get("status")))
        .collect();
    assert_eq!(found, [(1, OrderStatus::Paid), (2, OrderStatus::Paid)]);
    Ok(())
}
```

The derive's `accepts` checks the type's *name* (`order_status`, in any schema) and its
*labels*, which tokio-postgres looked up from the server the first time it met the type. By
default the labels must match the variants exactly: same set, nothing extra on either side.
That strictness catches a typo in a `rename` on the first query. It also has a consequence
that bites in production:

```rust
use postgres_types::{FromSql, ToSql};

#[derive(Debug, Clone, Copy, PartialEq, ToSql, FromSql)]
#[postgres(name = "order_status", rename_all = "lowercase")]
enum Strict {
    Pending,
    Paid,
}

#[derive(Debug, Clone, Copy, PartialEq, ToSql, FromSql)]
#[postgres(name = "order_status", rename_all = "lowercase", allow_mismatch)]
enum Lenient {
    Pending,
    Paid,
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TYPE pg_temp.order_status AS ENUM ('pending', 'paid')")
        .await?;
    let paid = "SELECT 'paid'::order_status";
    assert_eq!(
        client.query_one(paid, &[]).await?.get::<_, Strict>(0),
        Strict::Paid
    );

    // A migration adds a label the program does not know yet.
    client
        .batch_execute("ALTER TYPE order_status ADD VALUE 'refunded'")
        .await?;
    // A connection that loads the type afresh (any new connection, or this one after
    // clearing its type cache) now sees three labels where the strict enum has two:
    client.clear_type_cache();
    let row = client.query_one(paid, &[]).await?;
    assert!(
        row.try_get::<_, Strict>(0).is_err(),
        "even 'paid' is refused"
    );

    // allow_mismatch only fails on a label the enum lacks, when a row actually holds it.
    assert_eq!(row.get::<_, Lenient>(0), Lenient::Paid);
    let refunded = client
        .query_one("SELECT 'refunded'::order_status", &[])
        .await?;
    let error = refunded.try_get::<_, Lenient>(0).unwrap_err();
    let cause = std::error::Error::source(&error).unwrap().to_string();
    assert_eq!(cause, "invalid variant `refunded`");
    Ok(())
}
```

> **Pitfall:** with a strict enum, `ALTER TYPE … ADD VALUE` in a migration breaks every running
> instance of your service, gradually: each new pooled connection loads the new label list
> and then refuses *every* value of the type, not just the new one. The code and the database
> would have to change at the same instant, which never happens. For enums that can grow, add
> `allow_mismatch` and deploy in either order: the database first (the old code fails only on
> rows with the new label), or the code first (the new variant is simply never sent).

## 5.5 Domains

A domain is a base type with a constraint: `CREATE DOMAIN sku AS text CHECK (…)`. You would
expect it to behave exactly like its base type, and on the way *out* it does: a column of
type `sku` is described as `text`. On the way *in*, it does not. A parameter that goes into a
`sku` column is described as the domain itself, and plain Rust types refuse it:

```rust
use postgres_types::{FromSql, ToSql};
use tokio_postgres::types::{Kind, Type};

/// Derived for a domain: a one-field tuple struct with the domain's name.
#[derive(Debug, Clone, PartialEq, ToSql, FromSql)]
#[postgres(name = "sku")]
struct Sku(String);

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE DOMAIN pg_temp.sku AS text CHECK (VALUE ~ '^[A-Z0-9]{3,12}(-[A-Z0-9]{1,6})?$');
             CREATE TEMP TABLE products (sku sku PRIMARY KEY, name text);",
        )
        .await?;

    let insert = client
        .prepare("INSERT INTO products VALUES ($1, $2)")
        .await?;
    // $1 targets the sku column: it is described as the domain, not as text.
    assert_eq!(insert.params()[0].name(), "sku");
    assert!(matches!(insert.params()[0].kind(), Kind::Domain(base) if *base == Type::TEXT));

    // So a plain &str is refused, by the client, before anything is sent...
    let refused = client
        .execute(&insert, &[&"CHAIR-01", &"Chair"])
        .await
        .unwrap_err();
    assert!(refused.as_db_error().is_none());

    // ...while the derived domain type is accepted, and the server checks the constraint.
    client
        .execute(&insert, &[&Sku("CHAIR-01".into()), &"Chair"])
        .await?;
    let bad = client
        .execute(&insert, &[&Sku("chair".into()), &"Chair"])
        .await
        .unwrap_err();
    assert_eq!(bad.code().map(|c| c.code()), Some("23514")); // check_violation

    // A cast makes the parameter plain text again, if you prefer &str.
    client
        .execute(
            "INSERT INTO products VALUES ($1::text, $2)",
            &[&"LAMP-01", &"Lamp"],
        )
        .await?;

    // On the way out, the column is described as text, and both types read it.
    let row = client
        .query_one("SELECT sku FROM products WHERE name = 'Lamp'", &[])
        .await?;
    assert_eq!(row.columns()[0].type_(), &Type::TEXT);
    assert_eq!(row.get::<_, Sku>(0), Sku("LAMP-01".into()));
    assert_eq!(row.get::<_, &str>(0), "LAMP-01");
    Ok(())
}
```

So for each domain you choose one of three approaches:

| Approach | Parameters | Results | When |
|---|---|---|---|
| A derived domain newtype, `#[postgres(name = "…")] struct T(Base)` | accepted as the domain | read from the base type too | the domain has a Rust type of its own |
| A hand-written `accepts` that takes the domain *and* the base type | accepted either way | accepted either way | you also want validation in Rust (stockroom's `Sku`) |
| A cast in the SQL, `$1::text` | the base type | the base type | occasional use, or SQL you do not control |

Comparisons are not affected: in `WHERE sku = $1` the server types `$1` as the base type, so a
`&str` works there.

> **SQL lens:** domains are a cheap way to give a constraint a name and reuse it across
> tables, and the server enforces it everywhere the domain is used. One limit is new in
> PostgreSQL 18: a *virtual* generated column (`GENERATED ALWAYS AS (…) VIRTUAL`) cannot use a
> user-defined type, and a domain counts, so a virtual column cannot be computed from
> domain-typed columns. stockroom's `stock.available` ran into exactly that.

## 5.6 Composite types

A composite type is a named record: `CREATE TYPE address AS (street text, city text, …)`.
Tables can have composite columns, and a query can produce a composite value with `ROW(…)`.
A Rust struct with named fields maps onto it, field by field, by name:

```rust
use postgres_types::{FromSql, ToSql};

#[derive(Debug, Clone, PartialEq, ToSql, FromSql)]
#[postgres(name = "address")]
struct Address {
    street: String,
    city: String,
    postcode: Option<String>, // a field that may be NULL
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TYPE pg_temp.address AS (street text, city text, postcode text);
             CREATE TEMP TABLE stores (code text, address address);",
        )
        .await?;

    let mitte = Address {
        street: "Main 1".into(),
        city: "Berlin".into(),
        postcode: Some("10115".into()),
    };
    client
        .execute("INSERT INTO stores VALUES ('mitte', $1)", &[&mitte])
        .await?;

    // The whole value, or one field of it, in SQL.
    let row = client
        .query_one(
            "SELECT address, (address).city AS city FROM stores WHERE code = 'mitte'",
            &[],
        )
        .await?;
    assert_eq!(row.get::<_, Address>("address"), mitte);
    assert_eq!(row.get::<_, &str>("city"), "Berlin");

    // Arrays of composites work like any array.
    let more = vec![
        Address {
            street: "Ring 2".into(),
            city: "Potsdam".into(),
            postcode: None,
        },
        Address {
            street: "Allee 3".into(),
            city: "Leipzig".into(),
            postcode: Some("04109".into()),
        },
    ];
    let codes = vec!["ring", "allee"];
    // unnest in the select list keeps each address whole...
    client
        .execute(
            "INSERT INTO stores SELECT unnest($1::text[]), unnest($2::address[])",
            &[&codes, &more],
        )
        .await?;
    // ...while in FROM, a function returning a composite is expanded into its fields.
    let error = client
        .execute(
            "INSERT INTO stores SELECT * FROM unnest($1::text[], $2::address[])",
            &[&codes, &more],
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.as_db_error().unwrap().message(),
        "INSERT has more expressions than target columns"
    );

    let count: i64 = client
        .query_one("SELECT count(*) FROM stores", &[])
        .await?
        .get(0);
    assert_eq!(count, 3);
    Ok(())
}
```

The derive matches fields by name, so their order in the struct does not matter, and it checks
that the type has exactly these fields. A field that can be NULL needs an `Option`, just like a
column. `#[postgres(name = "…")]` on a field renames it, and `rename_all` works here too.

## 5.7 Behind the scenes: `Type`, `Kind` and the type cache

Built-in types are constants: `Type::INT8`, `Type::TEXT`. A type the server defines at run
time (an enum, a domain, a composite, or an extension's type) has an OID that tokio-postgres
cannot know in advance. The first time it meets one in a statement's description, it looks the
type up in the server's catalog, with a few extra queries, and builds a `Type` whose `kind()`
describes it:

| `Kind` | What it is |
|---|---|
| `Simple` | A base type, such as `int8` or `text` |
| `Enum(labels)` | An enum, with its labels |
| `Domain(base)` | A domain over `base` |
| `Composite(fields)` | A composite type, with each field's name and type |
| `Array(member)` | An array of `member` |
| `Range(member)`, `Multirange(member)` | A range type |
| `Pseudo` | A pseudo-type such as `void` or `record` |

The derives' `accepts` read exactly this: the enum's labels, the domain's base type, the
composite's fields.

These lookups are cached *per connection*, for the connection's lifetime. That is why the
strict enum in §5.4 kept working until the cache was cleared: the connection still held the
old label list. Two consequences follow:

- After `ALTER TYPE` (new enum labels, changed composite fields), connections that existed
  before keep the old description until they are closed or call `Client::clear_type_cache()`.
  stockroom's migration runner calls it after applying migrations, for its own connection.
- The first query on a fresh connection that involves a custom type pays for the lookup: a
  few extra round trips, once. A pool hides this cost behind connection reuse (Chapter 13).

### Side by side — a PostgreSQL enum

The task: define an `order_status` enum, store a value, and read it back as a typed value.

```sql
-- psql
tpbook=# CREATE TYPE order_status AS ENUM ('pending', 'paid', 'cancelled');
tpbook=# \dT+ order_status
                                          List of data types
 Schema |     Name     | Internal name | Size | Elements  |  Owner   | Access privileges | Description
--------+--------------+---------------+------+-----------+----------+-------------------+-------------
 public | order_status | order_status  | 4    | pending  +| postgres |                   |
        |              |               |      | paid     +|          |                   |
        |              |               |      | cancelled |          |                   |
(1 row)

tpbook=# SELECT 'paid'::order_status < 'cancelled';   -- labels are ordered as declared
 ?column?
----------
 t
(1 row)
```

~~~rust
// sqlx (illustration)
#[derive(sqlx::Type)]
#[sqlx(type_name = "order_status", rename_all = "lowercase")]
enum OrderStatus { Pending, Paid, Cancelled }

let status: OrderStatus = sqlx::query_scalar("SELECT 'paid'::order_status").fetch_one(&pool).await?;
~~~

```rust
use postgres_types::{FromSql, ToSql};

#[derive(Debug, Clone, Copy, PartialEq, ToSql, FromSql)]
#[postgres(name = "order_status", rename_all = "lowercase")]
enum OrderStatus {
    Pending,
    Paid,
    Cancelled,
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TYPE pg_temp.order_status AS ENUM ('pending', 'paid', 'cancelled')")
        .await?;
    let status: OrderStatus = client
        .query_one("SELECT $1::order_status", &[&OrderStatus::Paid])
        .await?
        .get(0);
    assert_eq!(status, OrderStatus::Paid);
    let pending = OrderStatus::Pending;
    assert_ne!(status, pending);
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | An enum is a type with ordered labels | A derive maps the type name and the labels | A derive maps the type name and the labels |
| **Different** | Labels are text you type | Matches labels by name when encoding and decoding | By default, the label *set* must match exactly, checked against the labels looked up from the server (per connection); `allow_mismatch` relaxes that to per-value |

---

## In stockroom

stockroom's [domain/](../stockroom/src/domain/mod.rs) module has one of each kind. `Cents` is a
transparent newtype:

~~~rust
// stockroom/src/domain/cents.rs (excerpt)
#[serde(transparent)]
#[postgres(transparent)]
pub struct Cents(pub i64);
~~~

`OrderStatus` is the enum, with `allow_mismatch` and the reason written next to it:

~~~rust
// stockroom/src/domain/order_status.rs (excerpt)
/// `allow_mismatch` matters in production. Without it, the derive accepts the database type
/// only if its labels are *exactly* these five: a migration that adds a sixth status would
/// make every new connection refuse every `order_status` value, `'paid'` included, until
/// this enum gets the variant too. With it, only an unknown label fails, and only when a row
/// actually holds it, so the database can be migrated ahead of the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSql, FromSql)]
#[serde(rename_all = "lowercase")]
#[postgres(name = "order_status", rename_all = "lowercase", allow_mismatch)]
pub enum OrderStatus {
    Pending,
    Paid,
    Ready,
    Collected,
    Cancelled,
}
~~~

A test in [tests/types.rs](../stockroom/tests/types.rs) pins that behaviour down: it adds a
label to the database and checks that fresh connections can still read the others.

`Quantity` is a derived domain newtype for `positive_quantity` (which order lines use), with
its own validation on the Rust side through serde. `Address` is the composite. `Sku` is
written by hand, because it wants both a domain-aware `accepts` and validation when reading:

~~~rust
// stockroom/src/domain/sku.rs (excerpt)
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
~~~

Instead of delegating, `to_sql` writes the bytes itself, which shows how simple text's binary
format is. The seed command inserts stores as three arrays zipped by `unnest`, one of them an
array of the composite, and its comment records §5.6's lesson:

~~~rust
// stockroom/src/cli/seed.rs (excerpt)
let store_ids: Vec<Uuid> = tx
    .query(
        "INSERT INTO stores (code, name, address)
         SELECT unnest($1::text[]), unnest($2::text[]), unnest($3::address[])
         RETURNING id",
        &[&codes, &names, &addresses],
    )
    .await?
~~~

## Summary

- `ToSql` and `FromSql` encode and decode the binary format of a PostgreSQL type; `accepts`
  is the type check behind "cannot convert". End a hand-written `ToSql` with `to_sql_checked!()`.
- Hand-written implementations usually delegate to an existing type and add validation. A
  `FromSql` that validates turns bad data into a conversion error.
- Derives cover newtypes (`transparent`), enums (`name`, `rename_all`, `allow_mismatch`),
  domains (a one-field tuple struct with the domain's `name`) and composites (named fields).
- A derived enum checks the whole label set by default; for enums that grow, use
  `allow_mismatch`, or every new connection refuses every value after a migration.
- A domain-typed parameter is described as the domain, so plain base types are refused; result
  columns are described as the base type.
- In `FROM`, `unnest` of a composite array expands the fields; in the select list it does not.
- Custom types are looked up once per connection and cached; `clear_type_cache` forgets them.

## Exercises

1. **A percentage.** Write `struct Percent(u8)` that maps to `smallint`, accepts 0 to 100 on
   both sides, and refuses anything else with a clear message.
   *Hint:* delegate to `i16`'s implementations, and convert with `i16::from` and
   `u8::try_from`.
2. **Evolve an enum.** With a strict enum and a real (not temporary) type, open two
   connections, add a label with one of them, and check what each connection can read.
   Explain the difference.
3. **A composite with a nested composite.** Make a `geo` composite `(lat float8, lon float8)`
   and an `address` composite that has a `location geo` field. Derive both in Rust, and round
   trip a value.
4. **`interval`, by hand.** tokio-postgres has no mapping for `interval`. Its binary format is
   16 bytes: a big-endian `i64` of microseconds, then an `i32` of days, then an `i32` of
   months. Write `struct PgInterval { months: i32, days: i32, microseconds: i64 }` with a
   `FromSql`, and read `interval '1 year 2 months 3 days 04:05:06.5'`.
   *Hint:* `postgres_types::accepts!(INTERVAL)` writes `accepts` for you.

### Solutions

**2.** The connection that ran `ALTER TYPE` still holds the type's old description in its
cache, so its strict enum keeps working (until it calls `clear_type_cache`). The second
connection behaves the same *if* it had already used the type. A third connection opened
afterwards looks the type up fresh, sees one label too many, and refuses every value. That is
how a migration breaks a service gradually, as its pool replaces connections.

**4.**

```rust
use postgres_types::{FromSql, Type, accepts};
use std::error::Error;

#[derive(Debug, PartialEq)]
struct PgInterval {
    months: i32,
    days: i32,
    microseconds: i64,
}

impl<'a> FromSql<'a> for PgInterval {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        let raw: &[u8; 16] = raw.try_into()?; // anything else is an error, not a panic
        Ok(PgInterval {
            microseconds: i64::from_be_bytes(raw[0..8].try_into()?),
            days: i32::from_be_bytes(raw[8..12].try_into()?),
            months: i32::from_be_bytes(raw[12..16].try_into()?),
        })
    }

    accepts!(INTERVAL);
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let interval: PgInterval = client
        .query_one("SELECT interval '1 year 2 months 3 days 04:05:06.5'", &[])
        .await?
        .get(0);
    assert_eq!(
        interval,
        PgInterval {
            months: 14,
            days: 3,
            microseconds: ((4 * 60 + 5) * 60 + 6) * 1_000_000 + 500_000,
        }
    );
    Ok(())
}
```

The three parts stay separate because they are not interchangeable. A month is not a fixed
number of days, and a day is not always 24 hours across a daylight-saving change, so PostgreSQL
keeps them apart and so should you. Converting to a single `Duration` is only correct when
`months` and `days` are zero.

---

Previous: [Chapter 4 — Prepared statements and the extended protocol](04-prepared-statements.md) ·
Next: [Chapter 6 — Errors](06-errors.md)
