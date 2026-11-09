# Building an ORM on tokio-postgres

**A build-along course, and the child project of
[Async PostgreSQL in Rust](../README.md): you build shelf, a small ORM on
[tokio-postgres](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/), one chapter at a time.
The derive macros that map rows to structs come first. A model layer, partial updates with
optimistic locking, a typed query builder, relations without N+1 queries, bulk writes, streams
and a schema check follow.**

> **Status: phases S1a, S1b, S1c and S2 done, on the branch `shelf`.** The plan is in
> [PLAN.md](PLAN.md). shelf is built and tested: `#[derive(FromRow)]`, the model layer (`Table`,
> `Insert`, `Patch`, the executor), the typed query builder, relations, bulk writes, streams
> and `verify`; and so is the capstone, `stockroom-shelf`, which serves the main book's
> stockroom API with stores, products and orders on shelf. **Chapters 0–4 are written**; every other chapter is
> **sketched**: a plan of the chapter in its place, with draft prose where the content is
> settled ([SKETCHES.md](SKETCHES.md)). [The book PDF](book/shelf-book.pdf) shows them all.

## Who it is for

Readers of the main book, or developers who already know tokio-postgres at that level: `Client`,
`query` and parameters, `Row`, `ToSql`/`FromSql`, transactions and deadpool. You do not need to
know procedural macros; the book teaches them from scratch.

## What is here

```text
orm/
├── PLAN.md            the plan: scope, target design, chapter outline, phases, decisions
├── CLAUDE.md          working rules for this project (on top of the root CLAUDE.md)
├── book/              the chapters (NN-slug.md) and their registry, book.toml
├── shelf/             the runtime: traits, executor, errors, query builder, … (re-exports the derives)
├── shelf-macros/      the proc-macro crate: a thin shell
├── shelf-codegen/     what the derives generate, as a plain library that tests and the book call
├── playground/        the book's runnable examples (generated from the chapters) and helpers
└── stockroom-shelf/   the capstone: stockroom's API, with part of its data layer on shelf
```

## Running it

Set up as in the main book's [Chapter 0](../book/00-introduction.md) (PostgreSQL 18 and a
`.env` with `ADMIN_DATABASE_URL`), then:

```bash
cargo test -p shelf -p shelf-codegen         # shelf's own tests
cargo test -p stockroom-shelf                # the capstone, against stockroom's own router
python3 tools/book.py --book orm check       # compile, lint and run every example in the book
python3 tools/book.py --book orm list        # chapter status
```

## Contents

<!-- book:contents:begin -->
| Chapter | You will learn | Status |
|---|---|---|
| [Chapter 0 — Introduction and setup](book/00-introduction.md) | What an ORM does and what shelf will be, the three crates and why, the finished API in ten lines, and the setup | current |
| [Chapter 1 — Mapping rows by hand](book/01-mapping-by-hand.md) | The FromRow trait, reading by name or by index, NULL and nested structs, and the boilerplate a macro should write | current |
| [Chapter 2 — Declarative macros](book/02-declarative-macros.md) | A macro_rules! that writes FromRow from a struct definition, tuple impls, $crate, and where declarative macros stop | current |
| [Chapter 3 — The first derive macro](book/03-first-derive.md) | Proc-macro crates, TokenStream, syn and quote, the three-crate split, hygiene, and printing the expansion | current |
| [Chapter 4 — Attributes, errors and generics](book/04-attributes.md) | #[shelf(…)] with parse_nested_meta, rename, skip, flatten, json and try_from, errors with spans, UI tests, generics | current |
| [Chapter 5 — Checking a statement once, not every row](book/05-column-indices.md) | Resolving columns and checking their types once per statement, measured against lookups by name | current |
| [Chapter 6 — Tables: the model derive](book/06-tables.md) | #[derive(Table)], primary keys, column metadata and SQL written at compile time, find and all | sketch |
| [Chapter 7 — The executor and shelf's errors](book/07-executor.md) | One trait over Client, Transaction and deadpool, statement strategies measured through lagproxy, and shelf::Error | sketch |
| [Chapter 8 — Writing rows: insert, update, delete, upsert](book/08-writing.md) | #[derive(Insert)], database defaults, RETURNING the model, full updates, deletes and ON CONFLICT | sketch |
| [Chapter 9 — Partial updates and optimistic locking](book/09-patch-and-versions.md) | #[derive(Patch)] three ways, Option<Option<T>>, version columns and Stale, updated_at | sketch |
| [Chapter 10 — A typed query builder](book/10-query-builder.md) | Typed columns from the derive, expressions, $n numbering, ordering, keyset pages, and what fails to compile | sketch |
| [Chapter 11 — Relations without N+1](book/11-relations.md) | belongs_to and has_many, the N+1 problem measured, batched loading with = ANY($1), pipelined loads, joins | sketch |
| [Chapter 12 — Bulk writes and streams](book/12-bulk-and-streams.md) | insert_many through unnest and binary COPY generated by the derive, and models streamed from a RowStream | sketch |
| [Chapter 13 — Trusting the mapping: schema checks and tests](book/13-trusting-the-mapping.md) | Verifying models against the live schema with Statement::columns, expansion and UI tests, and what compile-time checks add | sketch |
| [Chapter 14 — Capstone: stockroom on shelf](book/14-capstone.md) | Part of stockroom's data layer rewritten with shelf behind the same routes, compared line for line, and the escape hatches | sketch |
| [Appendix — attribute reference, generated code and phrasebook](book/15-appendix.md) | Every #[shelf(…)] attribute, what each derive generates, a shelf ↔ diesel ↔ SeaORM ↔ sqlx phrasebook, and a glossary | sketch |
<!-- book:contents:end -->

## License

As for the whole repository: code under MIT or Apache-2.0, text under CC BY 4.0
([../README.md](../README.md#license)).
