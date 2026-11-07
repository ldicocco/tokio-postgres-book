# Appendix — attribute reference, generated code and phrasebook

> **Building an ORM on tokio-postgres** · Previous: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)

> **Sketch, 7 November 2026.** A plan of this appendix, not its text; it will be replaced by
> the appendix. It collects reference material from every chapter: the `#[shelf(…)]`
> attributes (Chapters 4, 6, 8–11), what each derive generates (from the expansion tests in
> `orm/shelf-codegen/tests/expand/`), a phrasebook from shelf to diesel, SeaORM and sqlx
> (Chapter 0's comparison and each chapter's side by side), a glossary, and further reading.
> Entries already known are drafted; the rest is planned. Plan: [orm/PLAN.md](../PLAN.md)
> §4.3, §7, Appendix.

Look things up here once the chapters have taught them: which attribute does what and where it
may go, what a derive writes for a given struct, and how to say a shelf call in the crate you
use at work.

---

## A. Attribute reference

Drafted for the attributes that exist (phase S1a); the rest are added as their phases build
them.

On the struct, read by `FromRow`:

| Attribute | Meaning | Chapter |
|---|---|---|
| `rename_all = "…"` | Column names follow a convention: `"snake_case"` (the default), `"camelCase"`, `"PascalCase"` or `"SCREAMING_SNAKE_CASE"` | 4 |

On a field, read by `FromRow`:

| Attribute | Meaning | Chapter |
|---|---|---|
| `rename = "col"` | Read the column `col`; overrides `rename_all` | 4 |
| `skip` | Do not read; use `Default::default()` | 4 |
| `flatten` | A nested `FromRow` struct, read from the same row | 4 |
| `json` | Read a `json` or `jsonb` column through `Json<T>` (serde) | 4 |
| `try_from = Type` | Read a `Type`, then convert with `TryFrom`; the type may be quoted | 4 |

Rules, as the derive enforces them: keys are refused if unknown or repeated; `skip` goes with
nothing else; `flatten` goes with nothing but itself; `json` and `try_from` exclude each other;
every error is reported at its token, all in one build.

Plan for the attributes still to come: `table`, `schema`, `primary_key`, `generated`
(Chapter 6); `model` (Chapter 8); `version`, `updated_at` (Chapter 9); `belongs_to`, `has_many`
(Chapter 11).

## B. What each derive generates

For this struct:

~~~rust
// orm/shelf-codegen/tests/expand/basic.rs (excerpt)
#[derive(FromRow)]
struct Product {
    id: i64,
    sku: String,
    r#type: String,
    tags: Option<Vec<String>>,
}
~~~

`#[derive(FromRow)]` generates, as the expansion test pins it down:

~~~rust
// orm/shelf-codegen/tests/expand/basic.expanded.rs (excerpt)
#[automatically_derived]
impl ::shelf::FromRow for Product {
    type Columns = (usize, usize, usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Product", "id", "id")?,
            ::shelf::__private::column::<String>(columns, "Product", "sku", "sku")?,
            ::shelf::__private::column::<String>(columns, "Product", "type", "type")?,
            ::shelf::__private::column::<
                Option<Vec<String>>,
            >(columns, "Product", "tags", "tags")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "Product", "id")?,
            sku: ::shelf::__private::get::<String>(row, columns.1, "Product", "sku")?,
            r#type: ::shelf::__private::get::<
                String,
            >(row, columns.2, "Product", "type")?,
            tags: ::shelf::__private::get::<
                Option<Vec<String>>,
            >(row, columns.3, "Product", "tags")?,
        })
    }
}
~~~

Plan:
- The other expansion tests (`rename`, `skip_flatten`, `json_try_from`, `generics`) in the
  same form, each one line per attribute in the text and the full expansion as an excerpt.
- `Table`, `Insert` and `Patch` as their phases add them.

## C. Phrasebook

Plan: one table per task (map a row, find by key, insert, partial update, filter a page, load
children, bulk insert), with the shelf call and the diesel, SeaORM and sqlx equivalents, each
checked against the crate's version at the time of writing (check). A first row, to show the
shape:

| Task | shelf | sqlx | SeaORM |
|---|---|---|---|
| Read rows into a struct | `#[derive(FromRow)]`, `fetch_all` | `#[derive(FromRow)]`, `query_as` | `#[derive(DeriveEntityModel)]` (check) |

## D. Glossary

Draft entries:

- **Derive macro**: a procedural macro attached with `#[derive(…)]`; it receives the item and
  adds code next to it (Chapter 3).
- **Expansion**: the code a macro produces in place of its invocation, or beside it for a
  derive; shelf prints it with `pretty` (Chapter 3).
- **Helper attribute**: an attribute a derive declares (`attributes(shelf)`) so the compiler
  passes it through to the derive (Chapter 4).
- **Hygiene**: the rules that keep a macro's names from clashing with the caller's; paths are
  not covered, hence the absolute paths in generated code (Chapters 2 and 3).
- **Resolve**: shelf's step that finds and checks each field's column once per statement
  (Chapter 5).
- **Span**: where a token came from; errors are reported at spans (Chapter 4).
- **TokenStream**: the sequence of tokens a procedural macro receives and returns;
  `proc_macro2::TokenStream` is the same thing usable outside the compiler (Chapter 3).
- **N+1**: one query for a list, then one per item (Chapter 11).

Plan: the terms of Part II and III as their chapters are written.

## E. Further reading

Plan:
- *The Little Book of Rust Macros*; the Rust Reference on macros by example and procedural
  macros.
- syn's and quote's documentation; dtolnay's proc-macro workshop.
- diesel's, SeaORM's and toasty's guides; the main book's Appendix F.

> **Open questions.** (1) Whether the phrasebook belongs here or in Chapter 0, with a short
> version in the other. (2) Whether B should include every expansion test or one per derive.

---

Previous: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)
