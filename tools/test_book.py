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


if __name__ == "__main__":
    unittest.main()
