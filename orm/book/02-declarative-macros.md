# Chapter 2 — Declarative macros

> **Building an ORM on tokio-postgres** · Previous: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md) · Next: [Chapter 3 — The first derive macro](03-first-derive.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part I, third chapter. Builds on Chapter 1 (the hand-written `FromRow`) and on
> `macro_rules!` as the Rust Reference describes it ("Macros By Example"). shelf: the tuple
> impls in [orm/shelf/src/tuples.rs](../shelf/src/tuples.rs) (built, S1a), from one
> `macro_rules!` invocation with a line per arity. Crates: tokio-postgres 0.7.18 only. Plan:
> [orm/PLAN.md](../PLAN.md) §7, Chapter 2.

Chapter 1's `FromRow` impl is mechanical: the same line for every field, with the field's
name in two places. Mechanical code is what macros are for, and Rust has two kinds. This
chapter uses the simpler one, `macro_rules!`, which matches the tokens it is given against
patterns and pastes them into a template.

It goes surprisingly far. A `model!` macro can declare a struct and write its `FromRow` impl
in one go, and one invocation can write `FromRow` for tuples of every size from one to twelve,
which shelf keeps to this day. Then it stops: as soon as a field needs an attribute (read it
from another column, skip it, decode it as JSON), the patterns turn into a puzzle, and the
errors point at the macro instead of at your code. That is the case for the next chapter's
procedural macros.

**In this chapter you will learn:**

- how `macro_rules!` matches tokens: matchers, fragment specifiers and repetitions
- how to write a `model!` macro that declares a struct and its `FromRow` impl together
- how one macro invocation writes `FromRow` for tuples of one to twelve elements
- what `$crate` is for, and how hygiene keeps a macro's local variables apart from yours
- where declarative macros stop, and why shelf's derives are procedural

---

## 2.1 Matching tokens

Plan:
- A macro is a list of rules, each a matcher and a transcriber; the first rule that matches
  wins.
- Fragment specifiers used in the chapter: `ident`, `ty`, `vis`, `tt`, `expr`, `meta`; what
  each accepts. Repetitions `$( … ),*` and `$( … )+`.
- A **Rust refresher** box: macros expand before type checking, so a macro sees tokens, not
  types.

## 2.2 A `model!` macro

Plan:
- `model! { pub struct Product { id: i64, sku: String } }` expanding to the struct and its
  `FromRow` impl, with `stringify!($field)` as the column name.
- Used against a temporary table, the same assertions as Chapter 1.
- What it costs the user: the struct is now inside a macro call, rust-analyzer sees less, and
  other derives must be passed through (`$(#[$meta:meta])*`).

## 2.3 Tuples, one line per arity

Draft prose:

A query that returns two columns does not always deserve a struct. shelf reads it into a
tuple, `(String, i64)`, by position: the first element from the first column, and so on. That
needs one impl per tuple size, each with a type parameter and a position per element, and a
single macro writes all twelve. Each line of the invocation is one arity, with each element's
type parameter next to its position: `(A 0, B 1, C 2)`. The position is matched as a `tt`, a
single token tree, so that it can be pasted both as a number and, with `stringify!`, as the
name of the field in an error.

Plan:
- The excerpt from `orm/shelf/src/tuples.rs`: the macro and the first three lines of its
  invocation.
- Why not a recursive macro that peels one element at a time: it works, and is Exercise 1,
  but the flat list is easier to read in an excerpt.

## 2.4 `$crate` and hygiene

Plan:
- `$crate` in a macro exported from shelf names shelf wherever the macro is used, even if the
  user renamed the dependency.
- Hygiene: a `let row = …` inside the macro does not capture the caller's `row`; shown with a
  program where both exist.
- What hygiene does not cover: paths (`Result` means whatever is in scope at the call site),
  which is why expansions use `::core::result::Result`. Chapter 3 returns to this.

## 2.5 Where declarative macros stop

Plan:
- Field attributes: `#[col = "sku_code"] sku: String` needs a rule per combination, or a
  tt-muncher; shown briefly, not built.
- Errors: a mistake inside the invocation is reported against the macro's rules, not as a
  problem in your struct (the message captured from a real build).
- Generics and where clauses in the matched struct: possible, painful.
- The conclusion: a derive that reads attributes needs to parse Rust, which is what syn does.

## The programs

- `run`: the `model!` macro, reading a temporary table.
- `run`: a local trait with tuple impls for one to four elements from one invocation, then
  shelf's own tuples through `fetch_all::<(String, i64)>` (built).
- `run`: hygiene, a macro's `row` beside the caller's.
- A ` ~~~rust ` fragment with a broken invocation and its captured error (to capture).

## Side by side

Generating impls for many types: by hand (four impls written out) · shelf (the
`tuple_impls!` macro, built) · sqlx, which also implements `FromRow` for tuples (check how).

## The data

A temporary `products` table, as in Chapter 1.

## Claims

- Docs: the Rust Reference, "Macros By Example" (fragment specifiers, hygiene, `$crate`).
- Opinion: use `macro_rules!` for regular repetition; reach for a procedural macro as soon as
  you need to read attributes or report errors at the user's code.

## When not to use `macro_rules!`

- When a generic impl does the job (`impl<T: FromSql> … for T`), with no macro at all.
- When the input needs attributes or good error messages: Chapter 3.

## Exercises

Plan:
1. Rewrite the tuple macro recursively, peeling one element per step. *Hint:* the base case
   is a single element; the recursive rule matches a head and a tail.
2. Make `model!` accept other derives on the struct. *Hint:* `$(#[$meta:meta])*`.
3. Count a struct's fields with a macro. *Hint:* a repetition that expands to `1 +` per field.

> **Open questions.** (1) Whether tt-munching attributes deserves a full example or a
> paragraph; the chapter's point is where to stop. (2) How sqlx writes its tuple impls (check
> its source at the version the appendix names). (3) Macro metavariable expressions such as
> `${index()}` would remove the explicit positions; check whether they are stable in Rust 1.99
> (check).

---

Previous: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md) · Next: [Chapter 3 — The first derive macro](03-first-derive.md)
