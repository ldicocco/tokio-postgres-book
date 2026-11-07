//! Expansion tests: each `tests/expand/NAME.rs` holds structs with a shelf derive, and
//! `NAME.expanded.rs` holds what the derives generate for them, pretty-printed.
//!
//! A difference fails the test and prints both versions. After a deliberate change to the
//! generated code, rewrite the expected files with `SHELF_BLESS=1 cargo test -p shelf-codegen
//! --test expand`, then read the diff before committing it: that diff is the change users get.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use syn::{DeriveInput, Item};

/// The derives this test knows, by the name written in `#[derive(…)]`.
fn expand(derive: &str, input: &DeriveInput) -> Option<syn::Result<TokenStream>> {
    match derive {
        "FromRow" => Some(shelf_codegen::from_row::expand(input)),
        _ => None,
    }
}

fn derives_of(input: &DeriveInput) -> Vec<String> {
    let mut names = Vec::new();
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("derive")) {
        attr.parse_nested_meta(|meta| {
            names.push(meta.path.segments.last().unwrap().ident.to_string());
            Ok(())
        })
        .unwrap();
    }
    names
}

fn expanded(path: &Path) -> String {
    let file = syn::parse_file(&fs::read_to_string(path).unwrap()).unwrap();
    let mut out = TokenStream::new();
    for item in file.items {
        let input: DeriveInput = match item {
            Item::Struct(s) => s.into(),
            Item::Enum(e) => e.into(),
            _ => continue,
        };
        for derive in derives_of(&input) {
            if let Some(result) = expand(&derive, &input) {
                out.extend(result.unwrap_or_else(syn::Error::into_compile_error));
            }
        }
    }
    prettyplease::unparse(&syn::parse2(out).unwrap())
}

#[test]
fn derives_generate_the_expected_code() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expand");
    let mut inputs: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .filter(|p| !p.to_string_lossy().ends_with(".expanded.rs"))
        .collect();
    inputs.sort();
    assert!(!inputs.is_empty(), "no inputs in {}", dir.display());
    let bless = std::env::var_os("SHELF_BLESS").is_some();
    let mut failures = Vec::new();
    for input in &inputs {
        let actual = expanded(input);
        let expected_path = input.with_extension("expanded.rs");
        if bless {
            fs::write(&expected_path, &actual).unwrap();
            continue;
        }
        let expected = fs::read_to_string(&expected_path).unwrap_or_default();
        if actual != expected {
            failures.push(format!(
                "{} differs from {}:\n--- expected\n{expected}\n--- actual\n{actual}",
                input.display(),
                expected_path.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{}\n(rerun with SHELF_BLESS=1 to accept)",
        failures.join("\n")
    );
}
