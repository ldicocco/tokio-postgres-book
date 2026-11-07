//! The `#[shelf(…)]` attributes, parsed into typed structs that the derives read.
//!
//! Every derive parses every attribute and rejects the ones nobody knows; each derive then uses
//! the keys that concern it. So a key one derive needs (a later phase's `table`, say) is not an
//! error for another derive on the same struct.

use proc_macro2::Span;
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;
use syn::{Attribute, Error, LitStr, Result, Token, Type};

use crate::case::Case;

/// Errors gathered while parsing, so that one build reports all of them, not only the first.
#[derive(Default)]
pub struct Errors(Option<Error>);

impl Errors {
    pub fn push(&mut self, error: Error) {
        match &mut self.0 {
            Some(all) => all.combine(error),
            None => self.0 = Some(error),
        }
    }

    pub fn finish(self) -> Result<()> {
        self.0.map_or(Ok(()), Err)
    }
}

/// A parsed value, with the span of the key that set it, for error messages.
pub struct Keyed<T> {
    pub span: Span,
    pub value: T,
}

/// `#[shelf(…)]` on the struct.
#[derive(Default)]
pub struct ContainerAttrs {
    pub rename_all: Option<Keyed<Case>>,
}

impl ContainerAttrs {
    pub fn parse(attrs: &[Attribute], errors: &mut Errors) -> Self {
        let mut out = ContainerAttrs::default();
        for attr in shelf_attrs(attrs) {
            let parsed = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    let lit: LitStr = meta.value()?.parse()?;
                    let Some(case) = Case::from_name(&lit.value()) else {
                        return Err(Error::new_spanned(
                            &lit,
                            format!("unknown case: expected {}", Case::NAMES),
                        ));
                    };
                    once(&mut out.rename_all, case, &meta)
                } else {
                    Err(unknown(&meta, "a struct", &["rename_all"]))
                }
            });
            if let Err(error) = parsed {
                errors.push(error);
            }
        }
        out
    }

    /// The column name for a field without `rename`.
    pub fn column_for(&self, field: &str) -> String {
        self.rename_all
            .as_ref()
            .map_or(Case::Snake, |c| c.value)
            .apply(field)
    }
}

/// `#[shelf(…)]` on a field.
#[derive(Default)]
pub struct FieldAttrs {
    pub rename: Option<Keyed<LitStr>>,
    pub skip: Option<Keyed<()>>,
    pub flatten: Option<Keyed<()>>,
    pub json: Option<Keyed<()>>,
    pub try_from: Option<Keyed<Type>>,
}

/// How a field is read, once its attributes are known to be consistent.
pub enum Read<'a> {
    /// From one column, by its own `FromSql`.
    Column,
    /// From one column, through `Json<T>`.
    Json,
    /// From one column as the given type, then `TryFrom`.
    TryFrom(&'a Type),
    /// As a nested `FromRow`, from the same row.
    Flatten,
    /// Not at all: `Default::default()`.
    Skip,
}

impl FieldAttrs {
    pub fn parse(attrs: &[Attribute], errors: &mut Errors) -> Self {
        let mut out = FieldAttrs::default();
        for attr in shelf_attrs(attrs) {
            let parsed = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if lit.value().is_empty() {
                        return Err(Error::new_spanned(&lit, "a column name cannot be empty"));
                    }
                    once(&mut out.rename, lit, &meta)
                } else if meta.path.is_ident("skip") {
                    flag(&mut out.skip, &meta)
                } else if meta.path.is_ident("flatten") {
                    flag(&mut out.flatten, &meta)
                } else if meta.path.is_ident("json") {
                    flag(&mut out.json, &meta)
                } else if meta.path.is_ident("try_from") {
                    let ty = type_value(&meta)?;
                    once(&mut out.try_from, ty, &meta)
                } else {
                    Err(unknown(
                        &meta,
                        "a field",
                        &["rename", "skip", "flatten", "json", "try_from"],
                    ))
                }
            });
            if let Err(error) = parsed {
                errors.push(error);
            }
        }
        out.check_combinations(errors);
        out
    }

    /// Refuses keys that cannot go together, pointing at the one that does not fit.
    fn check_combinations(&self, errors: &mut Errors) {
        let others = [
            ("rename", self.rename.as_ref().map(|a| a.span)),
            ("flatten", self.flatten.as_ref().map(|a| a.span)),
            ("json", self.json.as_ref().map(|a| a.span)),
            ("try_from", self.try_from.as_ref().map(|a| a.span)),
        ];
        if self.skip.is_some() {
            for (name, span) in others {
                if let Some(span) = span {
                    errors.push(Error::new(
                        span,
                        format!(
                            "`{name}` cannot apply to a field marked `skip`, which is not read"
                        ),
                    ));
                }
            }
        } else if self.flatten.is_some() {
            for (name, span) in others {
                if let (Some(span), true) = (span, name != "flatten") {
                    errors.push(Error::new(
                        span,
                        format!(
                            "`{name}` cannot apply to a `flatten` field, whose struct reads its \
                             own columns"
                        ),
                    ));
                }
            }
        } else if let (Some(_), Some(try_from)) = (&self.json, &self.try_from) {
            errors.push(Error::new(
                try_from.span,
                "`try_from` and `json` cannot be combined: read the JSON into a type that \
                 converts itself",
            ));
        }
    }

    /// How to read the field. Call after `parse` reported no errors.
    pub fn read(&self) -> Read<'_> {
        if self.skip.is_some() {
            Read::Skip
        } else if self.flatten.is_some() {
            Read::Flatten
        } else if self.json.is_some() {
            Read::Json
        } else if let Some(ty) = &self.try_from {
            Read::TryFrom(&ty.value)
        } else {
            Read::Column
        }
    }
}

fn shelf_attrs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
    attrs.iter().filter(|attr| attr.path().is_ident("shelf"))
}

/// Stores a key's value, refusing a second one.
fn once<T>(slot: &mut Option<Keyed<T>>, value: T, meta: &ParseNestedMeta) -> Result<()> {
    if slot.is_some() {
        return Err(meta.error(format!("duplicate `{}`", name_of(meta))));
    }
    *slot = Some(Keyed {
        span: meta.path.span(),
        value,
    });
    Ok(())
}

/// A key without a value, such as `skip`.
fn flag(slot: &mut Option<Keyed<()>>, meta: &ParseNestedMeta) -> Result<()> {
    if meta.input.peek(Token![=]) || meta.input.peek(syn::token::Paren) {
        return Err(meta.error(format!("`{}` takes no value", name_of(meta))));
    }
    once(slot, (), meta)
}

/// A type after `=`: `try_from = i64`, or quoted as serde writes it, `try_from = "i64"`.
fn type_value(meta: &ParseNestedMeta) -> Result<Type> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        let lit: LitStr = value.parse()?;
        lit.parse()
            .map_err(|_| Error::new_spanned(&lit, "expected a type, as in `try_from = i64`"))
    } else {
        value.parse()
    }
}

fn unknown(meta: &ParseNestedMeta, on: &str, known: &[&str]) -> Error {
    let known: Vec<String> = known.iter().map(|k| format!("`{k}`")).collect();
    meta.error(format!(
        "unknown shelf attribute `{}` on {on}; expected {}",
        name_of(meta),
        known.join(", ")
    ))
}

fn name_of(meta: &ParseNestedMeta) -> String {
    let segments: Vec<String> = meta
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    segments.join("::")
}
