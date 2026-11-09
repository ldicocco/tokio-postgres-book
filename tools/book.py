#!/usr/bin/env python3
"""The book's tool: keeps the chapters, their runnable examples and the code they quote in
step, and renders the book as a PDF.

    python3 tools/book.py list                       chapters: status, examples, TODOs; the book PDF
    python3 tools/book.py check                      everything: registry, versions, mirrors, links,
                                                     excerpts, then build + clippy + rustfmt and run
                                                     every runnable example against a fresh database
        --chapter N|ID  (repeatable)                 compile and run only these chapters' examples
        --no-run                                     compile and lint only; no database needed
        --keep-db                                    leave the check database behind for inspection
    python3 tools/book.py extract                    write playground/examples/ from the book
    python3 tools/book.py links                      relative links and #anchors resolve
    python3 tools/book.py excerpts                   ~~~ excerpts still match the files they quote
    python3 tools/book.py index                      regenerate README.md's contents table
    python3 tools/book.py fmt [--chapter N]          rustfmt every ```rust block, in place
    python3 tools/book.py book                       the whole book as one PDF (the only one tracked)
    python3 tools/book.py pdf N|ID… | --all          chapter PDFs for local use, in target/book/

Every command takes --book DIR (before or after the command; default: book) to pick the book:
the repository has two, the main one in book/ and the shelf book in orm/book/
(`python3 tools/book.py --book orm check`). Each book's registry, book.toml, starts with a
[book] table that names its playground, check database, PDF and so on.

The conventions it enforces are in CLAUDE.md ("Writing the book"). In short:

  * A ```rust block is a complete program. It is extracted to playground/examples/, built,
    linted with clippy -D warnings, checked with rustfmt, and run with DATABASE_URL pointing
    at a freshly created database. An HTML comment on the line before it can adjust that:
    <!-- check: no-run -->, <!-- check: needs=tls -->, <!-- check: needs=lagproxy --> (DATABASE_URL
    then goes through apps/lagproxy, +5 ms each way; DIRECT_DATABASE_URL does not),
    <!-- check: should-panic -->, <!-- check: timeout=120 -->.
  * A ~~~ block is never compiled. If its first line is a comment naming a repository file
    (`// stockroom/src/repo.rs (excerpt)`), every line of it except comments and `…` lines
    must still appear in that file.
  * <!-- mirror: path#region --> before a block asserts that the block is exactly that file
    (or the lines between `// region: NAME` and `// endregion: NAME` in it).

Settings come from the environment, falling back to .env at the repository root:
ADMIN_DATABASE_URL (a postgres:// URL with rights to create and drop the check database),
TLS_DATABASE_URL and TLS_CA_FILE (for examples marked needs=tls).

Requires Python 3.11+, cargo and rustfmt, psql for running examples (found on the PATH, in
/Library/PostgreSQL/*/bin, or named by $PSQL), and pandoc and typst for the PDFs.
"""

import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
from dataclasses import dataclass, field
from urllib.parse import urlsplit, urlunsplit

if sys.version_info < (3, 11):  # tomllib landed in 3.11
    sys.exit("tools/book.py needs Python 3.11 or newer (it reads TOML with tomllib).")
import tomllib

TOOLS_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(TOOLS_DIR)

# The selected book's settings. These are the main book's; `configure` rebinds them from the
# [book] table of the book that --book names, before any command runs.
BOOK_DIR = os.path.join(REPO_ROOT, "book")
REGISTRY = os.path.join(BOOK_DIR, "book.toml")
README = os.path.join(REPO_ROOT, "README.md")
PLAYGROUND = os.path.join(REPO_ROOT, "playground")
PACKAGE = "playground"  # the playground's cargo package
EXAMPLES_DIR = os.path.join(PLAYGROUND, "examples")
# Prepended to example names. Both playgrounds build into the same target/debug/examples/,
# so the books' names must not collide.
EXAMPLE_PREFIX = ""
BOOK_PDF = os.path.join(BOOK_DIR, "tokio-postgres-book.pdf")
# Chapter PDFs are local previews: rendered on demand, never committed.
CHAPTER_PDF_DIR = os.path.join(REPO_ROOT, "target", "book")

CHECK_DB = "tpbook_check"
BOOK_TITLE = "Async PostgreSQL in Rust"
PDF_KICKER = "A practical course"
PDF_SUBTITLE = ("tokio-postgres from the first query to a production-shaped axum service — "
                "for developers who know Rust, a little Tokio and some SQL.")
STATUSES = ("planned", "sketch", "draft", "current")
# The chapters the book PDF contains, by status: the [book] table's `pdf_statuses`.
PDF_STATUSES = ("current",)
# The card that opens a sketch (orm/SKETCHES.md): a block quote whose first words are
# "**Sketch, <date>.**", within the first lines of the file.
SKETCH_CARD = re.compile(r"^>\s*\*\*Sketch, [^*]+\.\*\*", re.M)
SKETCH_CARD_LINES = 12
CHECK_OPTIONS = ("no-run", "needs", "should-panic", "timeout")
NEEDS = ("tls", "lagproxy")
DEFAULT_TIMEOUT = 60  # seconds, per example
TODO = "**TODO:**"

# Markdown outside book/ whose links are checked too.
EXTRA_DOCS = ("README.md", "PLAN.md", "CLAUDE.md")

# Top-level directories an excerpt may quote. A ~~~ block whose first line names a path
# under one of these must match that file; anything else (`// sqlx (illustration)`) is
# simply not an excerpt.
SOURCE_ROOTS = ("stockroom", "apps", "playground", "tools", "docker", "orm")

# The navigation each chapter carries and the book does not need: the banner under the
# title and the same links again at the very end.
BANNER = f"> **{BOOK_TITLE}**"
FOOTER = re.compile(r"\n---\n\n(?:Previous|Next):[\s\S]*\Z")

GENERATED = "// GENERATED by tools/book.py from {src}. Edit the chapter, not this file."

README_BEGIN = "<!-- book:contents:begin -->"
README_END = "<!-- book:contents:end -->"

# The keys of a registry's [book] table, all required.
BOOK_KEYS = {
    "title": "the book's name: the banner's first words, and the PDFs' title",
    "pdf": "the book PDF's file name, in the book's directory",
    "playground": "the crate the runnable examples are generated into, from the repository root",
    "package": "that crate's cargo package name",
    "example_prefix": "prepended to example names (playgrounds share target/debug/examples/)",
    "check_db": "the database `check` drops and recreates",
    "readme": "the Markdown file whose contents table `index` writes",
    "docs": "Markdown outside the chapters whose links are checked too",
    "chapter_pdfs": "where `pdf` renders chapter previews, from the repository root",
    "pdf_statuses": "the statuses of the chapters the book PDF contains, e.g. [\"current\"]",
    "kicker": "the line above the title on the book PDF's cover",
    "subtitle": "the line under the title on the book PDF's cover",
}
BOOK_KEYS_HELP = "\n".join(f"  {key:15} {meaning}" for key, meaning in BOOK_KEYS.items())


def find_book(arg):
    """The directory of the book `arg` names: one that holds a book.toml, or one whose book/
    subdirectory does (`--book orm` means orm/book)."""
    base = os.path.join(REPO_ROOT, arg)
    for candidate in (base, os.path.join(base, "book")):
        if os.path.isfile(os.path.join(candidate, "book.toml")):
            return os.path.normpath(candidate)
    found = glob.glob(os.path.join(REPO_ROOT, "*", "book.toml"))
    found += glob.glob(os.path.join(REPO_ROOT, "*", "book", "book.toml"))
    books = ", ".join(sorted(rel(os.path.dirname(path)) for path in found))
    sys.exit(f"--book {arg}: there is no book.toml in {arg} or {arg}/book (books: {books})")


def configure(arg):
    """Point the module-level settings at the book `arg` names (see `find_book`)."""
    global BOOK_DIR, REGISTRY, README, PLAYGROUND, PACKAGE, EXAMPLES_DIR, EXAMPLE_PREFIX
    global BOOK_PDF, CHAPTER_PDF_DIR, CHECK_DB, BOOK_TITLE, BANNER, EXTRA_DOCS
    global PDF_KICKER, PDF_SUBTITLE, PDF_STATUSES
    book_dir = find_book(arg)
    registry = os.path.join(book_dir, "book.toml")
    with open(registry, "rb") as f:
        settings = tomllib.load(f).get("book", {})
    missing = [key for key in BOOK_KEYS if key not in settings]
    unknown = [key for key in settings if key not in BOOK_KEYS]
    if missing or unknown:
        sys.exit(f"{rel(registry)}: the [book] table "
                 + (f"lacks {', '.join(missing)}" if missing else "")
                 + ("; " if missing and unknown else "")
                 + (f"has unknown keys {', '.join(unknown)}" if unknown else "")
                 + f". Its keys are:\n{BOOK_KEYS_HELP}")
    if not re.fullmatch(r"[a-z_][a-z0-9_]*", settings["check_db"]):
        sys.exit(f"{rel(registry)}: check_db must be a plain lower-case identifier")
    bad = [s for s in settings["pdf_statuses"] if s not in STATUSES or s == "planned"]
    if bad or not settings["pdf_statuses"]:
        sys.exit(f"{rel(registry)}: pdf_statuses must list statuses from {STATUSES[1:]}")
    BOOK_DIR, REGISTRY = book_dir, registry
    README = os.path.join(REPO_ROOT, settings["readme"])
    PLAYGROUND = os.path.join(REPO_ROOT, settings["playground"])
    PACKAGE = settings["package"]
    EXAMPLES_DIR = os.path.join(PLAYGROUND, "examples")
    EXAMPLE_PREFIX = settings["example_prefix"]
    BOOK_PDF = os.path.join(book_dir, settings["pdf"])
    CHAPTER_PDF_DIR = os.path.join(REPO_ROOT, settings["chapter_pdfs"])
    CHECK_DB = settings["check_db"]
    BOOK_TITLE = settings["title"]
    BANNER = f"> **{BOOK_TITLE}**"
    EXTRA_DOCS = tuple(settings["docs"])
    PDF_KICKER, PDF_SUBTITLE = settings["kicker"], settings["subtitle"]
    PDF_STATUSES = tuple(settings["pdf_statuses"])


# ───────────────────────────────────────────────────────────────────────────── settings ──


def load_dotenv():
    """Read KEY=VALUE lines from .env into os.environ, never overriding what is set."""
    path = os.path.join(REPO_ROOT, ".env")
    if not os.path.exists(path):
        return
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            key, _, value = line.partition("=")
            os.environ.setdefault(key.strip(), value.strip().strip("'\""))


def with_dbname(url, dbname):
    """The same postgres:// URL, naming another database."""
    parts = urlsplit(url)
    if parts.scheme not in ("postgres", "postgresql"):
        raise ValueError("expected a postgres:// URL")
    return urlunsplit(parts._replace(path="/" + dbname))


def redact(text):
    """Hide the password of every postgres:// URL in `text`."""
    return re.sub(r"(postgres(?:ql)?://[^:/@\s]+:)[^@\s]+@", r"\1***@", text)


def without_password(url):
    """`(url, password)` with the password moved out of the URL, so it reaches psql through
    PGPASSWORD instead of the process list."""
    parts = urlsplit(url)
    if parts.password is None:
        return url, None
    netloc = parts.username or ""
    netloc += "@" + (parts.hostname or "")
    if parts.port:
        netloc += f":{parts.port}"
    return urlunsplit(parts._replace(netloc=netloc)), parts.password


def reachable(host, port, timeout=1.0):
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def rel(path):
    return os.path.relpath(path, REPO_ROOT)


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


# ───────────────────────────────────────────────────────────────────────────── registry ──


@dataclass
class Chapter:
    id: str
    path: str
    title: str
    summary: str
    status: str

    @property
    def md_path(self):
        return os.path.join(BOOK_DIR, self.path)

    @property
    def exists(self):
        return os.path.exists(self.md_path)

    @property
    def active(self):
        """Begun (a sketch, a draft or current): its file exists and everything in it is
        checked."""
        return self.status != "planned"

    @property
    def number(self):
        m = re.match(r"^(\d+)-", os.path.basename(self.path))
        return m.group(1) if m else None

    @property
    def src_path(self):
        """The chapter file, relative to the repository root (for messages and stamps)."""
        return rel(self.md_path)

    @property
    def local_pdf_path(self):
        """Where `book.py pdf` renders this chapter: a gitignored preview."""
        return os.path.join(CHAPTER_PDF_DIR, os.path.splitext(self.path)[0] + ".pdf")

    def text(self):
        return read(self.md_path)

    def has_sketch_card(self):
        """Whether the file opens with a sketch's card (orm/SKETCHES.md)."""
        head = "\n".join(self.text().split("\n")[:SKETCH_CARD_LINES])
        return bool(SKETCH_CARD.search(head))

    def heading(self):
        m = re.search(r"^# (.+?)\s*$", self.text(), re.MULTILINE)
        return m.group(1) if m else None


def load_registry():
    with open(REGISTRY, "rb") as f:
        data = tomllib.load(f)
    return [Chapter(**entry) for entry in data.get("chapter", [])]


def registry_problems(chapters):
    problems = []
    for key in ("id", "path"):
        seen = set()
        for ch in chapters:
            value = getattr(ch, key)
            if value in seen:
                problems.append(f"{rel(REGISTRY)}: duplicate {key} {value!r}")
            seen.add(value)
    for ch in chapters:
        where = ch.src_path
        if ch.status not in STATUSES:
            problems.append(f"{rel(REGISTRY)}: {ch.id}: status {ch.status!r} is not one of {STATUSES}")
        if ch.active and not ch.exists:
            problems.append(f"{where}: status is {ch.status} but the file does not exist")
        if ch.exists and ch.heading() != ch.title:
            problems.append(
                f"{where}: heading {ch.heading()!r} differs from the registry title {ch.title!r}"
            )
        if ch.exists and not ch.active:
            problems.append(f"{where}: the file exists but its status is still planned")
        if ch.exists and ch.active and ch.has_sketch_card() != (ch.status == "sketch"):
            problems.append(
                f"{where}: status is sketch but the file has no sketch card "
                "(`> **Sketch, <date>.**` in its first lines)"
                if ch.status == "sketch"
                else f"{where}: the file opens with a sketch card but its status is {ch.status}"
            )
    registered = {ch.path for ch in chapters}
    for name in sorted(os.listdir(BOOK_DIR)):
        if name.endswith(".md") and name not in registered:
            problems.append(f"{rel(os.path.join(BOOK_DIR, name))} is not registered in {rel(REGISTRY)}")
    return problems


# ───────────────────────────────────────────────────────────────────── markdown blocks ──

FENCE_OPEN = re.compile(r"^( {0,3})(`{3,}|~{3,})(.*)$")
DIRECTIVE = re.compile(r"^<!--\s*(check|mirror):\s*(.*?)\s*-->\s*$")


@dataclass
class Block:
    fence: str  # the opening run: ``` or ~~~ (or longer)
    info: str  # the info string, stripped
    line: int  # 1-based line of the opening fence
    body: list
    closed: bool = True
    check: dict = field(default_factory=dict)  # from <!-- check: … -->
    mirror: str | None = None  # from <!-- mirror: … -->

    @property
    def runnable(self):
        return self.fence[0] == "`" and self.info == "rust"

    @property
    def last_line(self):
        """1-based line of the closing fence (or of the last line, if unclosed)."""
        return self.line + len(self.body) + (1 if self.closed else 0)


def parse_options(text):
    options = {}
    for item in filter(None, (x.strip() for x in text.split(","))):
        key, _, value = item.partition("=")
        options[key.strip()] = value.strip() or True
    return options


def parse_blocks(text):
    """Every fenced block in `text`, with the directives on the lines right above it."""
    lines = text.split("\n")
    blocks, i = [], 0
    while i < len(lines):
        m = FENCE_OPEN.match(lines[i])
        if not m or (m.group(2)[0] == "`" and "`" in m.group(3)):
            i += 1
            continue
        fence, info, start = m.group(2), m.group(3).strip(), i
        close = re.compile(r"^ {0,3}" + re.escape(fence[0]) + "{" + str(len(fence)) + r",}\s*$")
        body = []
        i += 1
        while i < len(lines) and not close.match(lines[i]):
            body.append(lines[i])
            i += 1
        block = Block(fence, info, start + 1, body, closed=i < len(lines))
        j = start - 1
        while j >= 0 and (d := DIRECTIVE.match(lines[j])):
            if d.group(1) == "check":
                block.check.update(parse_options(d.group(2)))
            else:
                block.mirror = d.group(2)
            j -= 1
        blocks.append(block)
        i += 1
    return blocks


def prose_lines(text):
    """`(line_no, line)` for every line outside fenced blocks."""
    fenced = set()
    for b in parse_blocks(text):
        fenced.update(range(b.line, b.last_line + 1))
    return [(n, line) for n, line in enumerate(text.split("\n"), 1) if n not in fenced]


# ──────────────────────────────────────────────────────────────────────────── examples ──


@dataclass
class Example:
    name: str
    chapter: Chapter
    block: Block

    @property
    def src(self):
        return f"{self.chapter.src_path}:{self.block.line}"

    @property
    def file(self):
        return os.path.join(EXAMPLES_DIR, self.name + ".rs")

    def book_line(self, file_line):
        """The chapter line a line of the generated file came from (line 1 is the header)."""
        return self.block.line + file_line - 1


def collect_examples(chapters):
    out = []
    for ch in chapters:
        if not (ch.active and ch.exists):
            continue
        k = 0
        for block in parse_blocks(ch.text()):
            if block.runnable:
                k += 1
                out.append(Example(f"{EXAMPLE_PREFIX}ch{ch.number}_{k:02}", ch, block))
    return out


def write_examples(examples):
    """Regenerate the playground's examples/: one file per runnable block, stale ones removed."""
    os.makedirs(EXAMPLES_DIR, exist_ok=True)
    wanted = {ex.name + ".rs" for ex in examples}
    for name in os.listdir(EXAMPLES_DIR):
        if name.endswith(".rs") and name not in wanted:
            os.remove(os.path.join(EXAMPLES_DIR, name))
    for ex in examples:
        content = GENERATED.format(src=ex.src) + "\n" + "\n".join(ex.block.body) + "\n"
        if not os.path.exists(ex.file) or read(ex.file) != content:
            with open(ex.file, "w", encoding="utf-8") as f:
                f.write(content)


def option_problems(examples, chapters):
    problems = []
    for ex in examples:
        for key, value in ex.block.check.items():
            if key not in CHECK_OPTIONS:
                problems.append(f"{ex.src}: unknown check option {key!r}")
            elif key == "needs" and value not in NEEDS:
                problems.append(f"{ex.src}: unknown need {value!r} (known: {', '.join(NEEDS)})")
            elif key == "timeout" and not str(value).isdigit():
                problems.append(f"{ex.src}: timeout must be a number of seconds")
    for ch in chapters:
        if not (ch.active and ch.exists):
            continue
        for block in parse_blocks(ch.text()):
            if block.check and not block.runnable:
                problems.append(
                    f"{ch.src_path}:{block.line}: a check directive on a block that is not ```rust"
                )
    return problems


def map_lines(output, examples):
    """Point compiler messages about generated files back at the chapter."""
    by_name = {ex.name: ex for ex in examples}

    def one(m):
        ex = by_name.get(m.group(1))
        if not ex:
            return m.group(0)
        return f"{m.group(0)} [{ex.chapter.src_path}:{ex.book_line(int(m.group(2)))}]"

    return re.sub(r"examples/(\w*?ch\d+_\d+)\.rs:(\d+)(?::\d+)?", one, output)


def cargo(argv, examples):
    """Run cargo in the repository; `(ok, output)` with chapter locations added."""
    res = subprocess.run(["cargo", *argv], cwd=REPO_ROOT, capture_output=True, text=True)
    return res.returncode == 0, map_lines(res.stdout + res.stderr, examples)


def target_dir():
    res = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=REPO_ROOT, capture_output=True, text=True, check=True,
    )
    return json.loads(res.stdout)["target_directory"]


def compile_problems(examples):
    """Build, clippy and rustfmt the examples. Empty list = all clean."""
    selection = [arg for ex in examples for arg in ("--example", ex.name)]
    ok, out = cargo(["build", "-p", PACKAGE, "--message-format", "short", *selection], examples)
    if not ok:
        return [f"cargo build failed:\n{out}"]
    ok, out = cargo(
        ["clippy", "-p", PACKAGE, "--message-format", "short", *selection, "--", "-D", "warnings"],
        examples,
    )
    problems = [] if ok else [f"clippy found problems:\n{out}"]
    res = subprocess.run(
        ["rustfmt", "--check", "--edition", "2024", *(ex.file for ex in examples)],
        cwd=REPO_ROOT, capture_output=True, text=True,
    )
    if res.returncode != 0:
        problems.append(
            "rustfmt would reformat these examples (format the code in the chapter):\n"
            + map_lines(res.stdout + res.stderr, examples)
        )
    return problems


# ──────────────────────────────────────────────────────────────────── running examples ──


def psql_binary():
    """$PSQL, else `psql` on the PATH, else the newest EDB install, else Homebrew's libpq."""
    if os.environ.get("PSQL"):
        return os.environ["PSQL"]
    found = shutil.which("psql")
    if found:
        return found

    def major(path):
        m = re.search(r"/PostgreSQL/(\d+)/", path)
        return int(m.group(1)) if m else 0

    candidates = sorted(glob.glob("/Library/PostgreSQL/*/bin/psql"), key=major, reverse=True)
    candidates += ["/opt/homebrew/opt/libpq/bin/psql", "/usr/local/opt/libpq/bin/psql"]
    for path in candidates:
        if os.access(path, os.X_OK):
            return path
    raise RuntimeError("psql not found: put it on the PATH or set PSQL=/path/to/psql")


def psql(admin_url, *statements):
    url, password = without_password(admin_url)
    env = dict(os.environ)
    if password is not None:
        env["PGPASSWORD"] = password
    argv = [psql_binary(), url, "-X", "-q", "-v", "ON_ERROR_STOP=1"]
    for sql in statements:
        argv += ["-c", sql]
    try:
        res = subprocess.run(argv, env=env, capture_output=True, text=True)
    except FileNotFoundError:
        raise RuntimeError(f"{argv[0]} not found: it is needed to create the check database")
    if res.returncode != 0:
        raise RuntimeError(f"psql failed: {redact(res.stderr.strip())}")


def admin_url():
    url = os.environ.get("ADMIN_DATABASE_URL")
    if url:
        return url
    if os.environ.get("DATABASE_URL"):
        return with_dbname(os.environ["DATABASE_URL"], "postgres")
    raise RuntimeError("set ADMIN_DATABASE_URL (or DATABASE_URL) in the environment or in .env")


LAGPROXY_DELAY_MS = 5  # one way; a round trip through the proxy costs at least twice this


class LagProxy:
    """apps/lagproxy, started once per `check` for the examples marked `needs=lagproxy`."""

    def __init__(self, upstream_url):
        parts = urlsplit(upstream_url)
        build = subprocess.run(
            ["cargo", "build", "-q", "-p", "lagproxy"], cwd=REPO_ROOT, capture_output=True, text=True
        )
        if build.returncode != 0:
            raise RuntimeError(f"cannot build lagproxy:\n{build.stderr}")
        self.process = subprocess.Popen(
            [os.path.join(target_dir(), "debug", "lagproxy"), "--listen", "127.0.0.1:0",
             "--upstream", f"{parts.hostname}:{parts.port or 5432}",
             "--delay-ms", str(LAGPROXY_DELAY_MS)],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
        line = self.process.stdout.readline()  # "listening on 127.0.0.1:PORT → …"
        m = re.search(r"listening on 127\.0\.0\.1:(\d+)", line)
        if not m:
            self.stop()
            raise RuntimeError(f"lagproxy did not start: {line!r}")
        self.port = int(m.group(1))

    def through(self, url):
        """`url`, pointed at the proxy instead of the server."""
        parts = urlsplit(url)
        userinfo = parts.netloc.rpartition("@")[0]
        netloc = (userinfo + "@" if userinfo else "") + f"127.0.0.1:{self.port}"
        return urlunsplit(parts._replace(netloc=netloc))

    def stop(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()


class Needs:
    """Decides how to run an example with `needs=…`, starting what it needs on first use."""

    def __init__(self, base_env):
        self.base_env = base_env
        self.proxy = None

    def environment_for(self, ex):
        """`(env, None)` to run the example with, or `(None, reason)` to skip it."""
        need = ex.block.check.get("needs")
        if not need:
            return self.base_env, None
        if need == "tls":
            url = os.environ.get("TLS_DATABASE_URL")
            if not url:
                return None, "needs=tls: TLS_DATABASE_URL is not set"
            parts = urlsplit(url)
            if not reachable(parts.hostname, parts.port or 5432):
                return None, (
                    f"needs=tls: nothing listens on {parts.hostname}:{parts.port or 5432} "
                    "(start it with `docker compose up -d`)"
                )
            return {**self.base_env, "DATABASE_URL": url}, None
        if need == "lagproxy":
            if self.proxy is None:
                self.proxy = LagProxy(self.base_env["DATABASE_URL"])
            return {
                **self.base_env,
                "DATABASE_URL": self.proxy.through(self.base_env["DATABASE_URL"]),
                "DIRECT_DATABASE_URL": self.base_env["DATABASE_URL"],
                "LAGPROXY_DELAY_MS": str(LAGPROXY_DELAY_MS),
            }, None
        return None, f"unknown need {need!r}"

    def stop(self):
        if self.proxy:
            self.proxy.stop()


def run_example(ex, env, exe):
    timeout = int(ex.block.check.get("timeout", DEFAULT_TIMEOUT))
    started = time.monotonic()
    try:
        res = subprocess.run(
            [exe], env=env, cwd=REPO_ROOT, capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return "fail", time.monotonic() - started, f"timed out after {timeout}s"
    elapsed = time.monotonic() - started
    output = map_lines(redact((res.stdout + res.stderr).strip()), [ex])
    if ex.block.check.get("should-panic"):
        if res.returncode != 0 and "panicked" in res.stderr:
            return "ok", elapsed, ""
        return "fail", elapsed, "expected a panic, but the example " + (
            "exited normally" if res.returncode == 0 else f"failed without one:\n{output}"
        )
    if res.returncode != 0:
        return "fail", elapsed, f"exit status {res.returncode}:\n{output}"
    return "ok", elapsed, ""


def run_examples(examples, keep_db):
    """Run the examples one after another against a fresh database; returns the results."""
    admin = admin_url()
    check_url = with_dbname(admin, CHECK_DB)
    psql(admin, f"DROP DATABASE IF EXISTS {CHECK_DB} WITH (FORCE)", f"CREATE DATABASE {CHECK_DB}")
    base_env = {**os.environ, "DATABASE_URL": check_url, "RUST_BACKTRACE": "0"}
    exe_dir = os.path.join(target_dir(), "debug", "examples")
    results = []
    needs = Needs(base_env)
    try:
        for ex in examples:
            if ex.block.check.get("no-run"):
                results.append((ex, "skip", 0.0, "no-run: compiled only"))
                continue
            env, reason = needs.environment_for(ex)
            if reason:
                results.append((ex, "skip", 0.0, reason))
                continue
            status, elapsed, detail = run_example(ex, env, os.path.join(exe_dir, ex.name))
            results.append((ex, status, elapsed, detail))
    finally:
        needs.stop()
        if not keep_db:
            psql(admin, f"DROP DATABASE IF EXISTS {CHECK_DB} WITH (FORCE)")
    return results


# ───────────────────────────────────────────────────────────────────── mirrors, versions ──

REGION = r"^\s*(?://|#|--)\s*{kind}:\s*{name}\s*$"


def mirror_lines(spec):
    """The lines a `<!-- mirror: path#region -->` names, or raise ValueError."""
    path, _, region = spec.partition("#")
    full = os.path.join(REPO_ROOT, path)
    if not os.path.exists(full):
        raise ValueError(f"{path} does not exist")
    lines = read(full).split("\n")
    if region:
        begin = re.compile(REGION.format(kind="region", name=re.escape(region)))
        end = re.compile(REGION.format(kind="endregion", name=re.escape(region)))
        starts = [i for i, line in enumerate(lines) if begin.match(line)]
        ends = [i for i, line in enumerate(lines) if end.match(line)]
        if len(starts) != 1 or len(ends) != 1 or ends[0] < starts[0]:
            raise ValueError(f"{path} has no single `region: {region}` … `endregion: {region}` pair")
        lines = lines[starts[0] + 1 : ends[0]]
    return lines


def trim(lines):
    lines = [line.rstrip() for line in lines]
    while lines and not lines[-1]:
        lines.pop()
    while lines and not lines[0]:
        lines.pop(0)
    return lines


def mirror_problems(chapters):
    problems = []
    for ch in chapters:
        if not (ch.active and ch.exists):
            continue
        for block in parse_blocks(ch.text()):
            if not block.mirror:
                continue
            where = f"{ch.src_path}:{block.line}"
            try:
                expected = trim(mirror_lines(block.mirror))
            except ValueError as error:
                problems.append(f"{where}: mirror: {error}")
                continue
            actual = trim(block.body)
            if actual != expected:
                n = next(
                    (i for i, (a, b) in enumerate(zip(actual, expected)) if a != b),
                    min(len(actual), len(expected)),
                )
                problems.append(
                    f"{where}: the block differs from {block.mirror} at its line {n + 1}:\n"
                    f"    book: {actual[n] if n < len(actual) else '(ends)'}\n"
                    f"    file: {expected[n] if n < len(expected) else '(ends)'}"
                )
    return problems


def dependency_spec(spec):
    """`"1.2.3"` or `"1.2.3 [feature, …]"`: what is compared, in a readable form."""
    if isinstance(spec, str):
        return spec
    features = sorted(spec.get("features", ()))
    return spec.get("version", "?") + (f" [{', '.join(features)}]" if features else "")


def version_problems():
    """The main playground's Cargo.toml spells its versions out (Chapter 0 prints it); they
    must equal the workspace's. A dependency that says `workspace = true` cannot drift."""
    with open(os.path.join(REPO_ROOT, "Cargo.toml"), "rb") as f:
        workspace = tomllib.load(f)["workspace"]["dependencies"]
    manifest = os.path.join(PLAYGROUND, "Cargo.toml")
    with open(manifest, "rb") as f:
        playground = tomllib.load(f).get("dependencies", {})
    problems = []
    for name, spec in playground.items():
        if isinstance(spec, dict) and spec.get("workspace"):
            continue
        if name not in workspace:
            problems.append(f"{rel(manifest)}: {name} is not declared in the workspace")
        elif dependency_spec(spec) != dependency_spec(workspace[name]):
            problems.append(
                f"{rel(manifest)}: {name} = {dependency_spec(spec)}, but the workspace "
                f"declares {dependency_spec(workspace[name])}"
            )
    return problems


# ──────────────────────────────────────────────────────────────────────── links, anchors ──

SKIP_LINK = re.compile(r"^(https?:|mailto:)")
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
CODE_SPAN = re.compile(r"`[^`\n]*`")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")


def github_slug(heading):
    """The anchor GitHub (and pandoc's gfm reader) gives a heading."""
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", heading)  # a link counts as its text
    text = re.sub(r"<[^>]+>", "", text).strip().lower()
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")


def anchors(text):
    seen, out = {}, set()
    for _, line in prose_lines(text):
        m = HEADING.match(line)
        if not m:
            continue
        slug = github_slug(m.group(2))
        if slug in seen:
            seen[slug] += 1
            slug = f"{slug}-{seen[slug] - 1}"
        else:
            seen[slug] = 1
        out.add(slug)
    return out


def link_problems(chapters):
    planned = {os.path.normpath(ch.md_path) for ch in chapters if not ch.active}
    docs = [ch.md_path for ch in chapters if ch.active and ch.exists]
    docs += [os.path.join(REPO_ROOT, name) for name in EXTRA_DOCS]
    problems, anchor_cache = [], {}
    for doc in docs:
        if not os.path.exists(doc):
            continue
        for n, line in prose_lines(read(doc)):
            for target in LINK.findall(CODE_SPAN.sub("", line)):
                if SKIP_LINK.match(target):
                    continue
                path, _, fragment = target.partition("#")
                dest = os.path.normpath(os.path.join(os.path.dirname(doc), path)) if path else doc
                where = f"{rel(doc)}:{n}"
                if not os.path.exists(dest):
                    if dest not in planned:  # a chapter still to be written is fine
                        problems.append(f"{where}: broken link {target}")
                    continue
                if fragment and dest.endswith(".md"):
                    if dest not in anchor_cache:
                        anchor_cache[dest] = anchors(read(dest))
                    if fragment not in anchor_cache[dest]:
                        problems.append(f"{where}: {target}: no heading with that anchor")
    return problems


# ──────────────────────────────────────────────────────────────────────────── excerpts ──

EXCERPT_SOURCE = re.compile(r"^\s*(?://|--|#)\s*(\S+\.\w+)(?:\s+\(.*\))?\s*$")
COMMENT_MARKERS = {".rs": "//", ".sql": "--", ".toml": "#", ".py": "#", ".sh": "#",
                   ".yml": "#", ".yaml": "#", ".http": "#"}
ELLIPSIS = "…"


def excerpt_source(block):
    """The repository path an excerpt quotes, or None if the block is not an excerpt."""
    if block.fence[0] != "~":
        return None
    first = next((line for line in block.body if line.strip()), None)
    m = first and EXCERPT_SOURCE.match(first)
    if not m:
        return None
    path = m.group(1)
    if path.split("/")[0] in SOURCE_ROOTS or os.path.exists(os.path.join(REPO_ROOT, path)):
        return path
    return None


def strip_comment(line, marker):
    """`line` without a trailing comment; markers inside string literals are kept."""
    quotes = "\"'" if marker == "--" else '"'
    in_string, escaped = None, False
    for i, ch in enumerate(line):
        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\" and marker != "--":
                escaped = True
            elif ch == in_string:
                in_string = None
        elif ch in quotes:
            in_string = ch
        elif line.startswith(marker, i):
            return line[:i]
    return line


def normalize(line, marker):
    return " ".join(strip_comment(line, marker).split())


def excerpt_problems(chapters):
    problems = []
    for ch in chapters:
        if not (ch.active and ch.exists):
            continue
        for block in parse_blocks(ch.text()):
            path = excerpt_source(block)
            if not path:
                continue
            where = f"{ch.src_path}:{block.line}"
            full = os.path.join(REPO_ROOT, path)
            if not os.path.exists(full):
                problems.append(f"{where}: quotes {path}, which does not exist")
                continue
            marker = COMMENT_MARKERS.get(os.path.splitext(path)[1], "//")
            source = {normalize(line, marker) for line in read(full).split("\n")}
            missing = []
            # The naming comment on the first line normalizes to nothing, like every other
            # comment-only line, so the whole body can be compared.
            for offset, line in enumerate(block.body, 1):
                norm = normalize(line, marker)
                if norm and ELLIPSIS not in norm and norm not in source:
                    missing.append(f"    line {block.line + offset}: {line.strip()}")
            if missing:
                problems.append(
                    f"{where}: excerpt of {path} has lines that are not in the file:\n"
                    + "\n".join(missing)
                )
    return problems


# ───────────────────────────────────────────────────────────────────────────────── PDFs ──


def renderer():
    sys.path.insert(0, TOOLS_DIR)
    import render_pdf  # noqa: E402 — tools/ is not a package

    return render_pdf


def book_chapters(chapters):
    """What the book PDF contains, in order: the chapters whose status is in `pdf_statuses`
    (`current` for the main book; the shelf book adds its drafts and sketches)."""
    return [ch for ch in chapters if ch.status in PDF_STATUSES and ch.exists]


def book_stamp(chapters):
    return "".join(
        f"{hashlib.sha256(ch.text().encode('utf-8')).hexdigest()}  {ch.src_path}\n"
        for ch in chapters
    )


def book_state(chapters):
    included = book_chapters(chapters)
    if not included:
        return "none"
    if not os.path.exists(BOOK_PDF):
        return "missing"
    stamp = BOOK_PDF + ".source-sha256"
    if not os.path.exists(stamp):
        return "stale"
    return "fresh" if read(stamp) == book_stamp(included) else "stale"


def for_book(ch):
    """The chapter's Markdown without its own navigation (banner and footer links)."""
    out, fence, in_banner = [], None, False
    for line in FOOTER.sub("", ch.text()).split("\n"):
        opened = re.match(r"^\s*(```|~~~)", line)
        if fence:
            if line.strip() == fence:
                fence = None
            out.append(line)
            continue
        if opened:
            fence = opened.group(1)
            out.append(line)
            continue
        if line.startswith(BANNER):
            in_banner = True
            continue
        if in_banner:
            if line.startswith(">"):
                continue
            in_banner = False
        out.append(line)
    return "\n".join(out)


# ───────────────────────────────────────────────────────────────────────────── commands ──


def report(failures, success):
    if failures:
        print("\n".join(f"✗ {f}" for f in failures), file=sys.stderr)
        return 1
    print(f"✓ {success}")
    return 0


def select(examples, wanted):
    if not wanted:
        return examples
    keys = {w.zfill(2) if w.isdigit() else w for w in wanted}
    return [ex for ex in examples if ex.chapter.number in keys or ex.chapter.id in keys]


def cmd_list(args, chapters):
    examples = collect_examples(chapters)
    counts = {}
    for ex in examples:
        counts[ex.chapter.id] = counts.get(ex.chapter.id, 0) + 1
    width = max(len(ch.id) for ch in chapters)
    print(f"{'chapter'.ljust(width)}  {'status':8} {'examples':>8} {'TODOs':>5}")
    for ch in chapters:
        todos = ch.text().count(TODO) if ch.exists else 0
        print(f"{ch.id.ljust(width)}  {ch.status:8} {counts.get(ch.id, 0):>8} {todos:>5}")
    included = len(book_chapters(chapters))
    print(f"\n{len(examples)} runnable examples. {rel(BOOK_PDF)}: {book_state(chapters)} "
          f"({included} chapter(s) with status {'/'.join(PDF_STATUSES)}; "
          f"`python3 tools/book.py{book_flag()} book` rebuilds it)")
    return 0


def cmd_extract(args, chapters):
    examples = collect_examples(chapters)
    write_examples(examples)
    print(f"✓ wrote {len(examples)} examples to {rel(EXAMPLES_DIR)}/")
    return 0


def cmd_links(args, chapters):
    return report(link_problems(chapters), "every relative link and anchor resolves")


def cmd_excerpts(args, chapters):
    return report(excerpt_problems(chapters), "every excerpt matches the file it quotes")


def cmd_check(args, chapters):
    examples = collect_examples(chapters)
    failures = registry_problems(chapters)
    failures += version_problems()
    failures += option_problems(examples, chapters)
    failures += mirror_problems(chapters)
    failures += link_problems(chapters)
    failures += excerpt_problems(chapters)
    write_examples(examples)

    selected = select(examples, args.chapter)
    if args.chapter and not selected:
        failures.append(f"no runnable examples in chapter(s) {', '.join(args.chapter)}")
    if selected:
        print(f"· building and linting {len(selected)} example(s)…", flush=True)
        compiled = compile_problems(selected)
        failures += compiled
        if not compiled and not args.no_run:
            print(f"· running them against a fresh database ({CHECK_DB})…", flush=True)
            try:
                results = run_examples(selected, args.keep_db)
            except RuntimeError as error:
                failures.append(str(error))
                results = []
            for ex, status, elapsed, detail in results:
                mark = {"ok": "✓", "skip": "-", "fail": "✗"}[status]
                note = f"  {detail.splitlines()[0]}" if status == "skip" else ""
                print(f"  {mark} {ex.name}  {ex.src.ljust(40)} {elapsed:6.2f}s{note}")
                if status == "fail":
                    failures.append(f"{ex.name} ({ex.src}) {detail}")
            ran = sum(1 for r in results if r[1] == "ok")
            skipped = sum(1 for r in results if r[1] == "skip")
            summary = f"{ran} example(s) ran, {skipped} skipped"
        else:
            summary = f"{len(selected)} example(s) compiled" + (" (not run)" if args.no_run else "")
    else:
        summary = "no runnable examples yet"
    state = book_state(chapters)
    if state in ("missing", "stale"):
        why = "it has not been built" if state == "missing" else "its chapters changed since it was built"
        print(f"! {rel(BOOK_PDF)} is {state}: {why}; "
              f"build it with `python3 tools/book.py{book_flag()} book`", file=sys.stderr)
    return report(failures, f"registry, versions, mirrors, links and excerpts clean; {summary}")


def cmd_fmt(args, chapters):
    """Format every runnable block with rustfmt, in place in the chapter."""
    changed = 0
    for ch in chapters:
        if not (ch.active and ch.exists):
            continue
        if args.chapter and ch.number not in {c.zfill(2) for c in args.chapter} and ch.id not in args.chapter:
            continue
        lines = ch.text().split("\n")
        # Bottom to top, so that earlier blocks keep their line numbers.
        for block in reversed([b for b in parse_blocks(ch.text()) if b.runnable]):
            source = "\n".join(block.body) + "\n"
            res = subprocess.run(
                ["rustfmt", "--edition", "2024", "--emit", "stdout"],
                input=source, capture_output=True, text=True,
            )
            if res.returncode != 0:
                sys.exit(f"{ch.src_path}:{block.line}: rustfmt failed:\n{res.stderr}")
            formatted = res.stdout.rstrip("\n").split("\n")
            if formatted != block.body:
                lines[block.line : block.line + len(block.body)] = formatted
                changed += 1
        new_text = "\n".join(lines)
        if new_text != ch.text():
            with open(ch.md_path, "w", encoding="utf-8") as f:
                f.write(new_text)
    print(f"✓ reformatted {changed} block(s)")
    return 0


def cmd_index(args, chapters):
    rows = ["| Chapter | You will learn | Status |", "|---|---|---|"]
    for ch in chapters:
        link = os.path.relpath(ch.md_path, os.path.dirname(README))
        name = f"[{ch.title}]({link})" if ch.exists else ch.title
        rows.append(f"| {name} | {ch.summary} | {ch.status} |")
    text = read(README)
    begin, end = text.find(README_BEGIN), text.find(README_END)
    if begin < 0 or end < begin:
        sys.exit(f"{rel(README)} has no {README_BEGIN} … {README_END} region")
    new = text[: begin + len(README_BEGIN)] + "\n" + "\n".join(rows) + "\n" + text[end:]
    if new != text:
        with open(README, "w", encoding="utf-8") as f:
            f.write(new)
    print(f"✓ {rel(README)} contents table is up to date")
    return 0


def cmd_pdf(args, chapters):
    """Render chapters to PDF for local use (printing, reading offline), in target/book/.

    Only the whole book is kept in the repository (`book`); these are never committed.
    """
    render_pdf = renderer()
    if args.all:
        targets = [ch for ch in chapters if ch.active and ch.exists]
    else:
        by_key = {ch.id: ch for ch in chapters} | {ch.number: ch for ch in chapters}
        unknown = [i for i in args.ids if i not in by_key and i.zfill(2) not in by_key]
        if unknown or not args.ids:
            sys.exit(f"name chapters by number or id (e.g. `pdf 3` or `pdf rows-and-types`), "
                     f"or pass --all (unknown: {unknown})")
        targets = [by_key.get(i) or by_key[i.zfill(2)] for i in args.ids]
    for ch in targets:
        if not ch.exists:
            sys.exit(f"{ch.src_path} does not exist yet")
        try:
            pdf = render_pdf.render(ch.md_path, ch.local_pdf_path, series=BOOK_TITLE)
        except RuntimeError as error:
            sys.exit(str(error))
        print(f"Wrote {rel(pdf)}")
    return 0


def cmd_book(args, chapters):
    render_pdf = renderer()
    included = book_chapters(chapters)
    if not included:
        sys.exit("no chapter is written yet")
    try:
        pdf = render_pdf.render_book(
            [(os.path.splitext(ch.path)[0], for_book(ch)) for ch in included],
            BOOK_PDF,
            title=BOOK_TITLE,
            kicker=PDF_KICKER,
            subtitle=PDF_SUBTITLE,
            left_note=book_contents_note(included)
            + " · every runnable example was compiled and run against PostgreSQL as written",
            right_note="python3 tools/book.py book" + book_flag(),
        )
    except RuntimeError as error:
        sys.exit(str(error))
    with open(BOOK_PDF + ".source-sha256", "w", encoding="utf-8") as f:
        f.write(book_stamp(included))
    print(f"Wrote {rel(pdf)} ({len(included)} chapters)")
    return 0


def book_contents_note(included):
    """The cover's count: `19 chapters`, or how many are written when some are sketches."""
    sketches = sum(ch.status == "sketch" for ch in included)
    if not sketches:
        return f"{len(included)} chapters"
    if sketches == len(included):
        return f"{len(included)} chapters, sketched"
    return f"{len(included) - sketches} of {len(included)} chapters written, the others sketched"


def book_flag():
    """` --book DIR` when a book other than the main one is selected, for printed commands."""
    selected = rel(BOOK_DIR)
    if selected == "book":
        return ""
    return f" --book {selected.removesuffix('/book')}"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    book_help = "the book: a directory with a book.toml, or one whose book/ has it (default: book)"
    parser.add_argument("--book", default="book", help=book_help)
    # The same option after the command; SUPPRESS keeps an absent one from resetting it.
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--book", default=argparse.SUPPRESS, help=book_help)
    sub = parser.add_subparsers(dest="command", required=True)

    def command(name, help):
        return sub.add_parser(name, help=help, parents=[common])

    command("list", "chapter status, examples and TODOs; the book PDF's freshness")
    check = command("check", "verify everything and run the examples")
    check.add_argument("--chapter", action="append", default=[], help="chapter number or id")
    check.add_argument("--no-run", action="store_true", help="compile and lint only")
    check.add_argument("--keep-db", action="store_true", help="keep the check database")
    command("extract", "write the playground's examples/ from the book")
    command("links", "check relative links and anchors")
    command("excerpts", "check excerpts against the files they quote")
    command("index", "regenerate the README's contents table")
    fmt = command("fmt", "rustfmt every runnable block, in place")
    fmt.add_argument("--chapter", action="append", default=[], help="chapter number or id")
    pdf = command("pdf", "render chapter PDFs for local use, under target/book/")
    pdf.add_argument("ids", nargs="*", help="chapter numbers or ids")
    pdf.add_argument("--all", action="store_true", help="every written chapter")
    command("book", "render the whole book as one PDF")
    args = parser.parse_args(argv)

    configure(args.book)
    load_dotenv()
    chapters = load_registry()
    command = {
        "list": cmd_list, "check": cmd_check, "extract": cmd_extract, "links": cmd_links,
        "excerpts": cmd_excerpts, "index": cmd_index, "pdf": cmd_pdf, "book": cmd_book,
        "fmt": cmd_fmt,
    }[args.command]
    return command(args, chapters)


if __name__ == "__main__":
    sys.exit(main())
