# Chapter 3 — The first derive macro

> **Building an ORM on tokio-postgres** · Previous: [Chapter 2 — Declarative macros](02-declarative-macros.md) · Next: [Chapter 4 — Attributes, errors and generics](04-attributes.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part I, fourth chapter. Builds on Chapters 1 and 2. shelf: the proc-macro shell
> [orm/shelf-macros/src/lib.rs](../shelf-macros/src/lib.rs), the generator
> [orm/shelf-codegen/src/from_row.rs](../shelf-codegen/src/from_row.rs) and the playground's
> `pretty` helper (all built; the chapter's stage program is S0's simpler generator). Crates:
> syn 3.0.9, quote 1.0.47, proc-macro2 1.0.107, prettyplease 0.3.0. Decisions: PLAN.md §10
> (three crates; syn 3). Findings: §12 #2, #3, #5, #7, #12. Plan:
> [orm/PLAN.md](../PLAN.md) §4.1, §7, Chapter 3.

A procedural macro is a Rust function that runs inside the compiler. `#[derive(FromRow)]`
hands it the tokens of your struct; it returns more tokens, an `impl` block, which the
compiler compiles as if you had written it. There is no pattern language this time: the
function can parse the struct into a syntax tree, look at every field and attribute, and build
whatever code it needs.

This chapter writes the first version of shelf's derive: every named field read from the
column of its name, nothing more. Most of the work is not the generator, which is twenty lines
of syn and quote, but the setting around it: the crate it must live in, the crate it should
live in instead, the paths that keep the generated code working in any module, and the ways to
see what was generated.

**In this chapter you will learn:**

- what a proc-macro crate is, and what the compiler hands a derive
- why shelf splits its macros into a code generator and a thin proc-macro shell
- how syn 3 parses a struct, and how quote builds tokens with interpolation and repetition
- what hygiene means for a derive: absolute paths, `__private`, raw identifiers
- three ways to see an expansion: printing it, `cargo expand`, and rust-analyzer
- how a derive reports an error as a value, pointing at the right token

---

## 3.1 Proc-macro crates

Plan:
- `[lib] proc-macro = true`; the function `#[proc_macro_derive(FromRow)]`, from
  `proc_macro::TokenStream` to `proc_macro::TokenStream`.
- A **Macro lens** box: the derive receives the whole item, attributes included, and its output
  is added next to the item, not in its place.
- What a proc-macro crate cannot do: export anything but macros, or run outside the compiler.

## 3.2 Three crates

Draft prose:

Because a proc-macro crate can export nothing but macros, a test in another crate cannot call
the function that generates code, and neither can an example in this book. shelf therefore
keeps the generator in `shelf-codegen`, an ordinary library whose functions go from a parsed
`syn::DeriveInput` to a `proc_macro2::TokenStream`, and leaves `shelf-macros` a shell of three
lines per derive: parse, call the generator, and turn an error into a compile error.
`proc_macro2` is what makes the split possible: it has the same types as the compiler's
`proc_macro`, but works in any program.

## 3.3 Parsing with syn 3

Plan:
- `parse_macro_input!(input as DeriveInput)`; `Data::Struct`, `Fields::Named`, each `Field`
  with its `ident`, `ty` and `attrs`.
- syn's features (`derive`, `full`) and what shelf enables.
- What changed from syn 2, which most material online shows: nothing a derive uses (PLAN.md
  §12 #5); the changes are in types, expressions and patterns.

## 3.4 Generating with quote

Plan:
- `quote! { … }` with `#name`, and repetition `#(#reads,)*`; `format_ident!`.
- The stage program: S0's generator written inline in a single-file example, applied to a
  struct built with `parse_quote!`, printed with `pretty`, and asserted.
- `pretty`, the playground's helper, printed in full as a mirror of
  `orm/playground/src/lib.rs`.

## 3.5 Hygiene

Draft prose:

The generated `impl` lands in the user's module, where any name may mean something else. A
user may have their own `Result` alias, a constant called `Ok`, a trait called `Default`. So
every path the derive writes is absolute: `::core::result::Result`, `::shelf::FromRow`, and
tokio-postgres through `::shelf::__private::tokio_postgres`, which also spares the user a
direct dependency on the driver. shelf's hygiene test derives next to all three of those names
and must compile.

Plan:
- Raw identifiers: a field `r#type` reads the column `type` (`IdentExt::unraw`, §12 #3).
- `#[automatically_derived]`, and what it tells lints.
- The test, quoted from [orm/shelf/tests/derive.rs](../shelf/tests/derive.rs).

## 3.6 Seeing the expansion

Plan:
- Printing it: shelf-codegen called from an example, `pretty` on the result (the main way,
  stable Rust).
- `cargo expand`: a separate install that needs a nightly toolchain (§12 #7; check the
  install before recommending it).
- rust-analyzer's "Expand macro recursively".

## 3.7 Errors as values

Plan:
- `syn::Error::new_spanned(token, message)` and `into_compile_error`: an enum refused at its
  `enum` keyword, a tuple struct at its name.
- Spans: tokens the macro writes carry the call site's span; tokens copied from the input keep
  theirs, so an error about a field's type points at the field (§12 #12).

## The programs

- `run`: S0's generator written inline, applied with `parse_quote!`, printed and asserted.
- `run`: the real `#[derive(shelf::FromRow)]` reading a temporary table with `fetch_all`.
- `run`: `shelf_codegen::from_row::expand` and `pretty`, the expansion printed (captured).
- `run`: the error for an enum, printed from the `syn::Error`.
- Mirror: `<!-- mirror: orm/playground/src/lib.rs#pretty -->`.

## Side by side

Attaching generated behaviour to a type: by hand (Chapter 1's impl) · shelf (the derive) ·
sqlx (`#[derive(sqlx::FromRow)]`).

## In shelf

Excerpts of `shelf-macros/src/lib.rs` (the shell) and `shelf-codegen/src/from_row.rs` (with
the parts later chapters add elided).

## Claims

- Docs: syn 3.0.0's release notes (§12 #5); the Rust Reference on procedural macros.
- Opinion: the three-crate split pays for itself with the first test.

## When not to write a derive

- A handful of types: write the impls, as Chapter 1 did.
- When a blanket impl over a trait bound does the job.

## Exercises

Plan:
1. Generate a `const FIELDS: &[&str]` beside the impl. *Hint:* a second item in the same
   `quote!`.
2. Make the enum error point at the first variant instead of the keyword. *Hint:* the span of
   `variants.first()`.
3. Write an expansion test for your derive. *Hint:* compare `pretty` output with a file.

> **Open questions.** (1) Whether to recommend `cargo expand` at all, given the nightly
> toolchain it needs (§12 #7). (2) How much of the syn 2 → 3 difference to discuss: probably
> one paragraph and a link. (3) The stage program reproduces S0's generator, which no longer
> exists in the crate; check that the excerpts of the finished generator still read as its
> continuation.

---

Previous: [Chapter 2 — Declarative macros](02-declarative-macros.md) · Next: [Chapter 4 — Attributes, errors and generics](04-attributes.md)
