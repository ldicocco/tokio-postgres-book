# Chapter 4 — Attributes, errors and generics

> **Building an ORM on tokio-postgres** · Previous: [Chapter 3 — The first derive macro](03-first-derive.md) · Next: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part I, fifth chapter. Builds on Chapter 3, and on main book
> [Chapter 3](../../book/03-rows-and-types.md) (`Json<T>`) and
> [Chapter 5](../../book/05-custom-types.md) (`FromSql`). shelf:
> [attrs.rs](../shelf-codegen/src/attrs.rs), [case.rs](../shelf-codegen/src/case.rs),
> [generics.rs](../shelf-codegen/src/generics.rs), `shelf::FromColumn` and the UI tests in
> `orm/shelf/tests/ui/` (all built, S1a). Crates: syn 3.0.9, trybuild 1.0.122. Decisions:
> PLAN.md §10 (attributes parsed by hand, not with darling). Findings: §12 #9, #10, #11, #12.
> Plan: [orm/PLAN.md](../PLAN.md) §4.3, §7, Chapter 4.

A real schema does not match your structs one to one. A column is called `sku_code` but the
field is `sku`; a legacy table uses `camelCase`; a field holds a value computed in Rust and is
not in the table at all; a `jsonb` column holds a document you want as a struct; a `bigint`
should become a `Cents` that refuses negative values. Each of those is one attribute on a
field, and serde has taught every Rust developer to expect them.

This chapter teaches the derive to read `#[shelf(…)]`. Parsing is the easy part. The work is
in what happens when the attributes are wrong: a misspelt key, a value where none belongs, two
attributes that contradict each other. A good derive reports all of them in one build, each at
the token that caused it, in words the user understands. The same care goes into generic
structs, which need exactly the bounds their fields require, and no others.

**In this chapter you will learn:**

- how a derive declares a helper attribute, and what happens if it forgets
- how to parse `#[shelf(…)]` with `parse_nested_meta` into a typed struct
- `rename_all`, `rename`, `skip`, `flatten`, `json` and `try_from`: what each generates
- how to report every mistake in one build, each at the right token
- how to test a derive's errors with trybuild, and what reading them taught shelf
- how to give a generic struct the bounds its fields need, and why shelf refuses lifetimes

---

## 4.1 Helper attributes

Draft prose:

A derive must say which attributes belong to it: `#[proc_macro_derive(FromRow,
attributes(shelf))]`. Without that declaration the compiler does not pass `#[shelf(…)]` to
anyone; it rejects it, before the derive runs, with "cannot find attribute `shelf` in this
scope", and adds, unhelpfully, that `shelf` is a crate. shelf hit exactly that when its
hygiene test first used `#[shelf(skip)]` (PLAN.md §12 #9).

## 4.2 Parsing with `parse_nested_meta`

Plan:
- `Attribute::parse_nested_meta`, key by key: `meta.path.is_ident("rename")`, `meta.value()?`
  then a `LitStr`; flags with no value; a type after `=` (`try_from = i64`, or quoted as serde
  writes it).
- Into a typed `FieldAttrs` with each value's span kept for later errors; duplicates refused.
- Every derive parses every key and ignores the ones it does not use, so `Table`'s keys will
  not trouble `FromRow` (PLAN.md §4.3).

## 4.3 The six attributes

Plan:
- For each, the generated line, printed by running the generator: `rename` changes the column
  name; `rename_all` applies one of four conventions written by hand (`case.rs`); `skip` writes
  `Default::default()`; `flatten` calls the nested type's `FromRow`; `json` reads through
  `Json<T>`; `try_from` reads one type and converts.
- One program that uses all six against a table with a `jsonb` column.

## 4.4 Every error at once

Plan:
- `syn::Error::combine`: shelf collects errors across fields and attributes (`attrs::Errors`)
  and returns them together.
- Contradictions, each reported at the attribute that does not fit: `skip` with anything,
  `flatten` with `rename`, `json` with `try_from`.
- Where errors land (§12 #12): at a literal, at a key, at a field's type.

## 4.5 UI tests

Draft prose:

An error message is part of a derive's interface, so shelf tests it like one. trybuild compiles
each file in `tests/ui/`, expects it to fail, and compares the compiler's output with a
`.stderr` file beside it. Reading those files for the first time changed shelf: for a field
whose type tokio-postgres cannot read, the error named
`shelf::__private::tokio_postgres::types::FromSqlOwned`, a path no user should see. shelf now
bounds field types by a trait of its own, `FromColumn`, implemented for every type the driver
can read and carrying `#[diagnostic::on_unimplemented]`, so the same mistake reads "`Instant`
cannot be read from a PostgreSQL column" (§12 #11).

Plan:
- The `.stderr` excerpts: `conflicts.stderr` and `not_readable.stderr`.
- `TRYBUILD=overwrite`, and why the output is pinned to one toolchain (rustc 1.99.1).

## 4.6 Generics

Plan:
- `split_for_impl`, and the predicates shelf adds only for fields whose type mentions a type
  parameter (`T: FromColumn`, `M: FromRow`, `Json<Vec<T>>: FromColumn`), from `generics.rs`.
- Why not bound every parameter: a bound on a concrete type would be checked at the derive,
  far from the field.
- Lifetimes refused with an error: borrowing from a row would make `FromRow` generic over a
  lifetime and `fetch_all` impossible to write simply; owned values cost an allocation per
  string, which Chapter 5's numbers put in proportion.

## 4.7 What darling would save

Plan:
- The same `FieldAttrs` with darling 0.24 (built on syn 3): a struct and a derive; what it
  handles (unknown keys, duplicates) and what it hides.
- Why shelf parses by hand: this book is about the mechanism.

## The programs

- `run`: a stage program that parses attributes with `parse_nested_meta` from a
  `parse_quote!` struct and prints the result.
- `run`: three mistakes in one struct, the combined errors printed.
- `run`: all six attributes against a temporary table with a `jsonb` column.
- `run`: a generic `Counted<T>` with a flattened `T`.
- Excerpts of `.stderr` files from `orm/shelf/tests/ui/`, checked by `book.py excerpts`.

## Side by side

Attribute-driven mapping: by hand (Chapter 1, with the renames written out) · shelf · sqlx
(`#[sqlx(rename, flatten, json, try_from)]`, check the exact spelling).

## In shelf

`attrs.rs` (the parser and the combination checks), `case.rs`, `generics.rs`, and
`FromColumn` in [orm/shelf/src/row.rs](../shelf/src/row.rs).

## Claims

- Measured: none; the error texts are captured from rustc 1.99.1 (§12 #10, #11).
- Docs: syn's `parse_nested_meta`; the Rust Reference on the `diagnostic` attribute namespace
  (check the version it was stabilized in).
- Opinion: keep the attribute set small; each attribute is a promise to document and test.

## When not to use an attribute

- When the SQL can do it: `SELECT sku_code AS sku` needs no `rename`.
- When the conversion is business logic: read the raw value and convert in a constructor.

## Exercises

Plan:
1. Add `#[shelf(default)]`: read the column if present, else `Default`. *Hint:* `resolve`
   returns an `Option<usize>` for that field.
2. Refuse two fields that map to the same column. *Hint:* collect names in a `HashMap` with
   their spans.
3. Explain why `FromRow<'a>` would make `fetch_all` hard. *Hint:* who owns the rows?

> **Open questions.** (1) An unreadable field type is reported twice, once per generated use
> (§12 #11); one bound site would make it once. Decide before writing. (2) Duplicate column
> detection does not exist yet (Exercise 2, or shelf itself?). (3) The darling comparison needs
> code, written and compiled against darling 0.24.1 (to write).

---

Previous: [Chapter 3 — The first derive macro](03-first-derive.md) · Next: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md)
