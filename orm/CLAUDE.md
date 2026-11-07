# orm/CLAUDE.md — the shelf child project

Guidance for Claude Code working under `orm/`: **shelf**, a small ORM on tokio-postgres, and
its book, *Building an ORM on tokio-postgres*.

**[orm/PLAN.md](PLAN.md) is the source of truth** for this project: its scope, the target
design, the chapter outline, the phases and the decisions taken. Read it before starting any
phase or chapter, and update it in the same change when a decision changes.

**Everything in the root [CLAUDE.md](../CLAUDE.md) applies here**: the style contract, the
code conventions, the database rules, the verified tokio-postgres facts, and the rule against
writing output from memory. This file lists only what differs or is added.

## Status

- Work happens on the branch **`shelf`**. It is merged into `main` in phase S4, not before.
- **Phase S0 (scaffold) is done** (2026-11-07): the crates, `tools/book.py --book`, and a
  Chapter 0 stub (`draft`) with two runnable examples.
- **Phase S1a (the mapping) is done** (2026-11-07): `#[derive(FromRow)]` complete, with
  attributes, errors, generics, tuples, one check per statement and `fetch_*`. PLAN.md §4.0 says
  what exists and where, and §12 #4–#13 what building it taught.
- **Every chapter is sketched** (2026-11-07): `orm/book/NN-*.md` holds each chapter's sketch
  (status `sketch`), in the format of [SKETCHES.md](SKETCHES.md), and the book PDF
  (`orm/book/shelf-book.pdf`) contains them.
- **Next: phase S1b, the model.** PLAN.md §13 is its checklist; a new session starts there.

## Commands

```bash
python3 tools/book.py --book orm list                  # status of the shelf book
python3 tools/book.py --book orm check                 # everything, against a fresh tpbook_shelf_check
python3 tools/book.py --book orm check --chapter 3     # one chapter's examples
python3 tools/book.py --book orm fmt --chapter 3       # rustfmt one chapter's ```rust blocks
python3 tools/book.py --book orm index                 # regenerate orm/README.md's contents table
python3 tools/book.py --book orm pdf 3                 # a local preview, in target/book/shelf/
python3 tools/book.py --book orm book                  # orm/book/shelf-book.pdf (once chapters are current)
cargo test -p shelf -p shelf-codegen                   # shelf's own tests (needs ADMIN_DATABASE_URL)
cargo test -p shelf --test from_row                    # one test file
SHELF_BLESS=1 cargo test -p shelf-codegen --test expand   # accept changed expansions (read the diff)
TRYBUILD=overwrite cargo test -p shelf --test ui       # accept changed compiler errors (read the diff)
cargo run -p shelf --release --example bench_mapping   # the mapping benchmark (DATABASE_URL)
```

`--book` also goes after the command (`book.py check --book orm`). The root gates still apply:
the workspace's cargo test, clippy and fmt cover the orm crates, and the main book's
`python3 tools/book.py check` must stay green on this branch.

## The crates

- `orm/shelf` is the runtime, and re-exports the derives. `orm/shelf-macros` is the proc-macro
  crate. `orm/shelf-codegen` holds what the derives generate. `orm/playground`
  (`shelf-playground`) holds the book's examples.
- **All code generation lives in shelf-codegen**, as `pub fn expand(&DeriveInput) ->
  syn::Result<TokenStream>` per derive. shelf-macros stays a shell: `parse_macro_input!`, then
  `expand`, then `into_compile_error`. Never add logic to it.
- **Generated code uses absolute paths only**: `::shelf::…`, `::core::…`, `::std::…`, and
  tokio-postgres through `::shelf::__private::tokio_postgres`. Never write a bare `Result`,
  `Option`, `Vec` or `Ok`. `orm/shelf/tests/derive.rs` defines `type Result = ();` to catch
  that.
- **Codegen never panics on user input.** Report a `syn::Error` spanned on the offending tokens
  (`Error::new_spanned`), and combine several with `Error::combine`.
- **Every attribute has an expansion test** (`shelf-codegen/tests/expand/`) and an integration
  test (in shelf, against PostgreSQL, through `tests/common::TestDb`). **Every error a derive
  reports has a trybuild UI test** (`shelf/tests/ui/`).
- **A new derive** gets: a module in shelf-codegen with `pub fn expand`, a
  `#[proc_macro_derive(Name, attributes(shelf))]` shell in shelf-macros, a re-export with its
  attribute table in `shelf/src/lib.rs`, an arm in `shelf-codegen/tests/expand.rs`'s `expand`,
  and its keys in `attrs.rs` (which every derive parses, so each must ignore the others' keys).
- **Generated code reads columns through `__private::{column, get}`**, which are bounded by
  `shelf::FromColumn` (not `FromSqlOwned`) so that a type error reads well (PLAN.md §12 #11).
- **SQL written by a derive** quotes identifiers at macro time, passes every value as a `$n`
  parameter, and never selects `*`.
- **Shape the code to grow** (PLAN.md §3): one module per derive and per concern, so that later
  chapters add code instead of rewriting what earlier chapters quote.

## Writing the shelf book

These add to the root style contract:

- **Writing a chapter replaces its sketch**: same file; the card goes, the open questions are
  settled (in PLAN.md §10 or §12), and the status becomes `draft`, then `current`. Read the
  sketch first: its claims say what is measured, what is "(to measure)" and what is
  "(check)". Revise a sketch in place (with a new date on its card) when the plan changes.
- After changing any chapter or sketch, rebuild the PDF: `python3 tools/book.py --book orm
  book` (it contains every chapter, sketches included).

- Chapter files are `orm/book/NN-slug.md`, registered in `orm/book/book.toml`. Its `[book]`
  table holds the settings `book.py` reads.
- **The banner** names this book, then links the neighbouring chapters, as in the main book:
  `> **Building an ORM on tokio-postgres** · Previous: [Chapter N-1 — …](…) · Next: […](…)`.
- **Side-by-side sections** have three columns: **by hand** (a runnable block), **shelf** (a
  runnable block, possibly the same program), and an established crate as a ` ~~~rust `
  illustration whose first line names it (`// SeaORM (illustration)`). The closing table is
  `| | by hand | shelf | SeaORM |`, with **Same** and **Different** rows.
- **Callouts:** the four from the main book, plus `> **Macro lens:**` for what the compiler
  hands a macro and takes back (tokens, spans, hygiene, expansion order).
- **"In shelf" excerpts** start with `// orm/shelf-codegen/src/from_row.rs (excerpt)` (the
  path from the repository root) and are followed by a link to the file.
- **The build-along rules** (PLAN.md §3): each stage first as a runnable single-file program;
  the finished crate in excerpts; behaviour through runnable examples that use the real
  derives.
- **Expansions and compiler errors are captured, never typed:** run shelf-codegen through
  `shelf_playground::pretty`, and take errors from trybuild's `.stderr` files or a real build.
- **Links to the main book** are relative: `../../book/03-rows-and-types.md#anchor`.
- Examples become `orm/playground/examples/shelf_chNN_KK.rs`, which is generated and
  gitignored. The playground's own helpers sit between `// region: NAME` markers in
  `orm/playground/src/lib.rs`, and the chapter that introduces one mirrors it
  (`<!-- mirror: orm/playground/src/lib.rs#pretty -->`).

## Databases

The root list applies, plus:

- `tpbook_shelf_check` is **dropped and recreated** by every `book.py --book orm check`.
- `shelf_test` holds shelf's integration tests: `tests/common/mod.rs` creates it on first use
  and gives each test a schema `t_<uuidv7>` (on the connection's `search_path`), dropped
  afterwards.

## Verified facts (S0, S1a)

- syn 3.0.9's derive types have the same shape as syn 2's (`DeriveInput`, `Data`, `Fields`,
  `Field`). `syn::ext::IdentExt::unraw` strips the `r#` from a raw identifier.
- prettyplease 0.3.0 (on syn 3) formats a derive's output as rustfmt would. Its exact output
  for `FromRow` is pinned by `from_row::tests::each_field_reads_the_column_of_its_name`.
- A trait and a derive macro can share a name (`shelf::FromRow`), because traits and macros
  live in different namespaces.
- The two playgrounds build their examples into the same `target/debug/examples/`, hence the
  `shelf_` prefix (`example_prefix` in `orm/book/book.toml`).
- In tokio-postgres 0.7.18, `Row::try_get("name")` searches the columns by exact name, then
  again ignoring ASCII case; every `try_get`, by name or index, checks `accepts` per row. shelf
  matches names exactly (PostgreSQL folds unquoted names itself) and resolves them once.
- `Column::table_oid()` and `column_id()` exist; tokio-postgres reports 0 as `None`.
- syn 3.0.0 changed nothing a derive uses (`DeriveInput`, `Fields`, `Attribute`,
  `parse_nested_meta`); its breaking changes are in types, expressions, patterns and literals.
- A derive must declare its helper attribute (`attributes(shelf)`), or the compiler rejects
  `#[shelf(…)]` before the derive runs.
- `#[diagnostic::on_unimplemented]` works through a blanket impl (`FromColumn`).
- `cargo expand` is not installed on this machine (a nightly toolchain is).
