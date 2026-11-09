//! SQL joined at compile time: what lets one derive's statement name another type's table.
//!
//! `concat!` takes literals only, and an `Insert` derive knows its model's name, not the
//! model's table and columns, which are the model's constants. Generated code joins those with
//! [`join`] in a `const` item, so the finished statement is a constant like any other
//! (orm/PLAN.md §12 #32):
//!
//! ```ignore
//! const INSERT: &'static str = {
//!     const PARTS: &[&str] = &["INSERT INTO ", <Product as Table>::TABLE, " (…) VALUES (…)"];
//!     const BYTES: [u8; total_len(PARTS)] = join(PARTS);
//!     utf8(&BYTES)
//! };
//! ```

/// The length of `parts` joined.
pub const fn total_len(parts: &[&str]) -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < parts.len() {
        len += parts[i].len();
        i += 1;
    }
    len
}

/// `parts` joined, as bytes; `N` is their [`total_len`].
pub const fn join<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut out = [0; N];
    let mut at = 0;
    let mut i = 0;
    while i < parts.len() {
        let bytes = parts[i].as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            out[at] = bytes[j];
            at += 1;
            j += 1;
        }
        i += 1;
    }
    out
}

/// `bytes` as a `&str`. They are always whole `&str`s joined, so this cannot fail.
pub const fn utf8(bytes: &[u8]) -> &str {
    match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => panic!("joined SQL is UTF-8"),
    }
}
