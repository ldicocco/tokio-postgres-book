//! Helpers shared by the shelf book's runnable examples.
//!
//! The main book's helpers are re-exported, so an example can write
//! `shelf_playground::connect()`; the main book prints each one in the chapter that introduces
//! it (`connect` in its Chapter 1). The helpers this book adds are printed verbatim where they
//! are introduced: the regions marked `// region: <name>` are the exact text the chapters
//! mirror, and `tools/book.py --book orm check` compares the two.

pub use playground::{call, connect, connect_with_messages, pool, slice_iter};

// region: pretty
use proc_macro2::TokenStream;

/// Formats generated code as rustfmt would, so that an example can print what a derive writes
/// and assert on it. A derive generates items (`impl` blocks, functions, constants), and that
/// is what `tokens` must hold.
pub fn pretty(tokens: TokenStream) -> String {
    let file: syn::File = syn::parse2(tokens).expect("a derive generates items");
    prettyplease::unparse(&file)
}
// endregion: pretty
