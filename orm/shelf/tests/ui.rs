//! UI tests: each file in `tests/ui/` must fail to compile, with exactly the errors in its
//! `.stderr` file. They pin down what users see when they get a derive wrong: the message, and
//! the code it points at.
//!
//! The expected output is rustc's, so it can change with the toolchain (pinned to 1.99.1). To
//! accept new output deliberately: `TRYBUILD=overwrite cargo test -p shelf --test ui`, then read
//! the diff.

#[test]
fn derive_errors() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
