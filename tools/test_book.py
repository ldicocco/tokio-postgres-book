"""Unit tests for tools/book.py's parsing: run with `python3 -m unittest discover tools`."""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import book  # noqa: E402


DOC = """# Chapter 3 — Rows

Prose with `inline code` and a [link](02-queries.md#21-batch_execute).

<!-- check: no-run, timeout=90 -->
```rust
fn main() {}
```

~~~rust
// stockroom/src/repo.rs (excerpt)
let x = 1; // a teaching comment
// …
~~~

````markdown
```rust
not a real block: it is inside a four-backtick fence
```
````

## 3.1 `Row::get` — and friends
## 3.1 `Row::get` — and friends
"""


class ParseBlocks(unittest.TestCase):
    def setUp(self):
        self.blocks = book.parse_blocks(DOC)

    def test_finds_the_outer_blocks_only(self):
        self.assertEqual([b.fence for b in self.blocks], ["```", "~~~", "````"])

    def test_runnable_is_backtick_rust_only(self):
        self.assertEqual([b.runnable for b in self.blocks], [True, False, False])

    def test_directives_attach_to_the_next_block(self):
        self.assertEqual(self.blocks[0].check, {"no-run": True, "timeout": "90"})
        self.assertEqual(self.blocks[1].check, {})

    def test_line_numbers_are_one_based_fence_lines(self):
        self.assertEqual(self.blocks[0].line, 6)
        self.assertEqual(self.blocks[0].last_line, 8)

    def test_prose_excludes_fenced_lines(self):
        prose = [line for _, line in book.prose_lines(DOC)]
        self.assertNotIn("fn main() {}", prose)
        self.assertNotIn("not a real block: it is inside a four-backtick fence", prose)
        self.assertIn("## 3.1 `Row::get` — and friends", prose)

    def test_mirror_directive(self):
        blocks = book.parse_blocks("<!-- mirror: playground/src/lib.rs#connect -->\n~~~rust\nx\n~~~\n")
        self.assertEqual(blocks[0].mirror, "playground/src/lib.rs#connect")


class Anchors(unittest.TestCase):
    def test_github_slug(self):
        self.assertEqual(book.github_slug("2.1 `batch_execute`"), "21-batch_execute")
        self.assertEqual(book.github_slug("Side by side — *connecting*"), "side-by-side--connecting")
        self.assertEqual(book.github_slug("A [linked](x.md) heading"), "a-linked-heading")

    def test_duplicates_are_numbered(self):
        self.assertEqual(
            book.anchors(DOC),
            {"chapter-3--rows", "31-rowget--and-friends", "31-rowget--and-friends-1"},
        )


class Excerpts(unittest.TestCase):
    def test_source_is_read_from_the_first_line(self):
        self.assertEqual(book.excerpt_source(book.parse_blocks(DOC)[1]), "stockroom/src/repo.rs")

    def test_illustrations_are_not_excerpts(self):
        block = book.parse_blocks("~~~rust\n// sqlx (illustration)\nlet x = 1;\n~~~\n")[0]
        self.assertIsNone(book.excerpt_source(block))

    def test_comments_are_stripped_but_not_inside_strings(self):
        self.assertEqual(book.normalize('let x = 1;  // note', "//"), "let x = 1;")
        self.assertEqual(book.normalize('let u = "http://x"; // n', "//"), 'let u = "http://x";')
        self.assertEqual(book.normalize("SELECT '--' AS a -- why", "--"), "SELECT '--' AS a")
        self.assertEqual(book.normalize("    // only a comment", "//"), "")


class Mirrors(unittest.TestCase):
    def test_region_is_the_lines_between_markers(self):
        lines = book.mirror_lines("playground/src/lib.rs#connect")
        self.assertTrue(lines[0].startswith("use tokio_postgres"))
        self.assertFalse(any("region:" in line for line in lines))

    def test_missing_region_is_an_error(self):
        with self.assertRaises(ValueError):
            book.mirror_lines("playground/src/lib.rs#no-such-region")


class Urls(unittest.TestCase):
    URL = "postgres://postgres:s3cret@localhost:5432/postgres"

    def test_with_dbname(self):
        self.assertEqual(
            book.with_dbname(self.URL, "tpbook_check"),
            "postgres://postgres:s3cret@localhost:5432/tpbook_check",
        )

    def test_redact(self):
        self.assertEqual(book.redact(f"cannot reach {self.URL}"),
                         "cannot reach postgres://postgres:***@localhost:5432/postgres")

    def test_without_password(self):
        self.assertEqual(book.without_password(self.URL),
                         ("postgres://postgres@localhost:5432/postgres", "s3cret"))


class CompilerOutput(unittest.TestCase):
    def test_locations_are_mapped_back_to_the_chapter(self):
        chapter = book.Chapter("rows", "03-rows.md", "t", "s", "draft")
        block = book.Block("```", "rust", 40, ["fn main() {}"])
        ex = book.Example("ch03_01", chapter, block)
        out = book.map_lines("playground/examples/ch03_01.rs:2:5: error: oops", [ex])
        self.assertIn("[book/03-rows.md:41]", out)

    def test_prefixed_example_names_are_mapped_too(self):
        chapter = book.Chapter("introduction", "00-introduction.md", "t", "s", "draft")
        block = book.Block("```", "rust", 10, ["fn main() {}"])
        ex = book.Example("shelf_ch00_01", chapter, block)
        out = book.map_lines("orm/playground/examples/shelf_ch00_01.rs:3:1: error", [ex])
        self.assertIn("00-introduction.md:12]", out)


class Sketches(unittest.TestCase):
    """A sketch opens with its card (orm/SKETCHES.md), and the cover counts sketches apart."""

    def test_the_card_is_a_block_quote_starting_with_sketch_and_a_date(self):
        self.assertTrue(book.SKETCH_CARD.search("> **Sketch, 7 November 2026.** A plan of this"))
        self.assertFalse(book.SKETCH_CARD.search("> **Protocol lens:** what travels"))
        self.assertFalse(book.SKETCH_CARD.search("A sketch, 7 November 2026, in prose"))

    def test_the_cover_note_counts_written_and_sketched_chapters(self):
        def chapter(status):
            return book.Chapter("x", "00-x.md", "t", "s", status)

        self.assertEqual(book.book_contents_note([chapter("current")] * 3), "3 chapters")
        self.assertEqual(book.book_contents_note([chapter("sketch")] * 2), "2 chapters, sketched")
        self.assertEqual(
            book.book_contents_note([chapter("current"), chapter("sketch"), chapter("sketch")]),
            "1 of 3 chapters written, the others sketched",
        )


class Books(unittest.TestCase):
    """`--book` selects a registry, and its [book] table rebinds the module's settings."""

    def tearDown(self):
        book.configure("book")  # leave the module pointing at the main book

    def test_the_main_book_keeps_its_settings(self):
        book.configure("book")
        self.assertEqual(book.rel(book.BOOK_DIR), "book")
        self.assertEqual(book.CHECK_DB, "tpbook_check")
        self.assertEqual(book.PACKAGE, "playground")
        self.assertEqual(book.EXAMPLE_PREFIX, "")
        self.assertEqual(book.BANNER, "> **Async PostgreSQL in Rust**")

    def test_a_project_directory_means_its_book_subdirectory(self):
        book.configure("orm")
        self.assertEqual(book.rel(book.BOOK_DIR), os.path.join("orm", "book"))
        self.assertEqual(book.PACKAGE, "shelf-playground")
        self.assertEqual(book.rel(book.EXAMPLES_DIR), os.path.join("orm", "playground", "examples"))
        self.assertNotEqual(book.CHECK_DB, "tpbook_check")
        self.assertTrue(book.EXAMPLE_PREFIX, "the playgrounds share target/debug/examples/")
        self.assertEqual(book.BANNER, f"> **{book.BOOK_TITLE}**")
        self.assertEqual(book.book_flag(), " --book orm")

    def test_an_unknown_book_names_the_known_ones(self):
        with self.assertRaises(SystemExit) as raised:
            book.configure("no-such-book")
        self.assertIn("orm/book", str(raised.exception))

    def test_workspace_inherited_dependencies_are_not_version_checked(self):
        book.configure("orm")  # its playground says `workspace = true` throughout
        self.assertEqual(book.version_problems(), [])


if __name__ == "__main__":
    unittest.main()
