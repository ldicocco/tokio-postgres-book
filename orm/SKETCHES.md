# Chapter sketches

**No sketch remains in `orm/book/`.** Every chapter of the shelf book replaced its sketch by the
end of 9 November 2026 (phase S3c), so the sketches this file describes are in the git history,
not in the tree. The file stays as the method, for a chapter added later.

Before a chapter of the shelf book is written, it has a sketch: a short plan of the chapter,
with draft prose where the content can already be defined. The format follows the author's
other book series (*Decisions in the Loop*, `tools/SKETCHES.md` there), adapted to this book's
style contract (the root [CLAUDE.md](../CLAUDE.md) and [orm/CLAUDE.md](CLAUDE.md)) and its
tooling.

Sketches follow the plan, [orm/PLAN.md](PLAN.md): its chapter outline (§7), its target design
(§4), its decisions (§10) and its findings (§12). When a sketch and the plan disagree, the plan
wins, or is changed first. A sketch is not binding: it records the general idea of a chapter as
the project stands on its date, and the chapter may depart from it when writing shows a better
way.

## Where sketches live

Every chapter that is not written yet has its sketch in `orm/book/`, under the chapter's file
name, as a placeholder chapter with the status `sketch` in `orm/book/book.toml`. The book PDF
contains the sketches (the registry's `pdf_statuses`), so a reader sees the whole book's shape
before it is written. All sixteen were written on 7 November 2026, after phase S1a; Chapters 0–4
replaced theirs on 9 November 2026, and the sketches of Chapters 6–13 were revised the same day
to the design as built in phases S1b and S1c, and Chapter 14's to the capstone as built in S2.

When a chapter is written, it replaces its sketch: the same file, the card gone, the open
questions settled (in PLAN.md §10 or §12), the status `draft` and then `current`. A sketch
can also be revised in place before then, when the plan changes; its card then carries the new
date.

`tools/book.py` checks a sketch like any begun chapter: the heading against the registry, the
banner, links and anchors, excerpts, and any runnable block, which is compiled and run. The
registry check also requires that a chapter has a card if and only if its status is `sketch`.

## How to read a sketch

- **The card** at the top says where the chapter stands: its part, what it builds on (the
  main book's chapters, earlier shelf chapters), what it adds to shelf and whether that exists
  yet, the crates it uses, the decisions and findings it depends on, and the plan sections it
  follows.
- **Draft prose** is meant to survive into the chapter. **A plan** is a bullet list of what a
  section will say, the code it shows, the measurement it makes.
- **The programs** are described, not written: what each runnable example shows and asserts,
  with its check directive (`run`, `needs=lagproxy`, `should-panic`, `no-run`). A sketch may
  keep a runnable block only if it already works (Chapter 0's do); `book.py check` runs it.
- **Claims** are labelled: *measured* (this repository's runs, with the PLAN.md §12 entry that
  records them, or the main book's §12), *docs* (docs.rs, the PostgreSQL documentation, a
  crate's own documentation, attributed), or *opinion*. A number not yet measured is marked
  "(to measure)"; an API not yet checked in the pinned sources, "(check)"; a shelf item that
  is not built yet, "(proposed)".
- **Open questions**, in a box at the end, are planning, not book text: what to decide or
  verify before the chapter is written.

## What a chapter's sketch contains

1. **The heading and the banner**, as in any chapter: `# Chapter N — Title`, then
   `> **Building an ORM on tokio-postgres** · Previous: … · Next: …`.
2. **A card**: a block quote whose first words are **Sketch, <date>.**, within the first
   twelve lines. It names the part; the main-book chapters it builds on ("main book Chapter 3")
   and the shelf chapters; the shelf files it adds or grows, with "(built, S1a)" or
   "(proposed, S1b)"; the crates with their versions; the decisions (PLAN.md §10) and findings
   (§12 #n) it uses; and the plan sections it follows.
3. **An opening** of two to four paragraphs of draft prose, in the book's voice: starting from
   a job the reader needs done and from what they would try first.
4. **"In this chapter you will learn:"** followed by a list.
5. **Sections** numbered as the chapter will number them (`## N.M Title`, from PLAN.md §7),
   each either *draft prose* or *a plan*. Only what can already be defined is drafted.
6. **The programs**: each runnable example, with its directive and what it asserts; the shelf
   files the chapter will quote in "In shelf" excerpts; the tests it points to.
7. **Side by side**: the one task the section solves three ways, and the crate in the third
   column (sqlx, SeaORM or diesel; PLAN.md §2).
8. **The data**: the tables the examples create (temporary, or in a schema of their own), and
   any part of stockroom's schema the chapter uses.
9. **Claims**, labelled as above.
10. **When not to use shelf for this**, and what to use instead.
11. **Exercises**: a plan of two to four, each with the idea of its hint.
12. **Open questions**: a block quote that starts with **Open questions.**, then the footer
    links.

## What the appendix's sketch contains

The appendix collects reference material from the chapters, so its sketch says where each entry
will come from: a card, an opening of a paragraph, the sections with their entries (drafted
where the content is already known: an attribute that exists, an expansion that the tests pin
down, a term the written chapters use), and the open questions.

## Conventions

- Voice as in the root CLAUDE.md: second person, plain English, *why* before *how*.
- "main book Chapter N" (with a relative link, `../../book/NN-slug.md`) for *Async PostgreSQL
  in Rust*; "Chapter N" for this book.
- No code that claims to compile unless it is a runnable block that `book.py check` runs.
  Short shapes of a proposed API go in a ` ~~~rust ` block whose first line says so
  (`// shelf (proposed, illustrative)`), and never name a file there, which would make the
  block an excerpt.
- Crate API names only when they were checked in the pinned sources or exist in shelf;
  anything else is described in words or marked "(check)" or "(proposed)".
- Pipe tables with at most five columns; callouts sparingly; no images.
- Length: about 100 to 200 lines of Markdown for a chapter, two to four PDF pages.
