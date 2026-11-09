//! The `#[shelf(…)]` attributes, parsed into typed structs that the derives read.
//!
//! All of shelf's derives share `#[shelf(…)]`. Every derive parses every key, so that `FromRow`
//! and `Table` on one struct read the same column names, but each reports only the errors in
//! the keys it owns ([`Derive::reports`]): a struct with two derives hears about each mistake
//! once. A key that no derive knows is reported by `FromRow`, which a `Table` always has.
//! `Insert` and `Patch` are each the only shelf derive on their struct, so they report
//! everything.

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

/// The derives that read `#[shelf(…)]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Derive {
    FromRow,
    Table,
    Insert,
    Patch,
}

impl Derive {
    /// Whether this derive reports an error found while parsing `key`. `None` is a key no
    /// derive knows, or a mistake in the attribute's syntax: `FromRow` reports those, because a
    /// `Table` always comes with it.
    pub fn reports(self, key: Option<&str>) -> bool {
        match (self, key) {
            (Derive::Insert | Derive::Patch, _) => true,
            (_, None) => self == Derive::FromRow,
            (_, Some(key)) => KEYS
                .iter()
                .any(|(name, owner)| *name == key && *owner == self),
        }
    }
}

/// Every key, and the derive that owns it: the one that reports its errors. A derive may read
/// a key it does not own (`Table` reads `rename` to name its columns).
const KEYS: &[(&str, Derive)] = &[
    ("rename_all", Derive::FromRow),
    ("table", Derive::Table),
    ("schema", Derive::Table),
    ("rename", Derive::FromRow),
    ("skip", Derive::FromRow),
    ("flatten", Derive::FromRow),
    ("json", Derive::FromRow),
    ("try_from", Derive::FromRow),
    ("primary_key", Derive::Table),
    ("generated", Derive::Table),
    ("version", Derive::Table),
    ("updated_at", Derive::Table),
    ("model", Derive::Insert),
    ("conflict", Derive::Insert),
];

/// The keys allowed on a struct, and on a field, in the order error messages list them.
const CONTAINER_KEYS: &[&str] = &["rename_all", "table", "schema", "model"];
const FIELD_KEYS: &[&str] = &[
    "rename",
    "skip",
    "flatten",
    "json",
    "try_from",
    "primary_key",
    "generated",
    "version",
    "updated_at",
    "conflict",
];

/// `#[shelf(…)]` on the struct.
#[derive(Default)]
pub struct ContainerAttrs {
    pub rename_all: Option<Keyed<Case>>,
    pub table: Option<Keyed<LitStr>>,
    pub schema: Option<Keyed<LitStr>>,
    pub model: Option<Keyed<Type>>,
}

impl ContainerAttrs {
    pub fn parse(attrs: &[Attribute], derive: Derive, errors: &mut Errors) -> Self {
        let mut out = ContainerAttrs::default();
        for attr in shelf_attrs(attrs) {
            // The key being parsed, so that its error goes to the derive that owns it.
            let mut key = None;
            let parsed = attr.parse_nested_meta(|meta| {
                key = known(&meta, CONTAINER_KEYS);
                let parsed = out.parse_key(&meta);
                if parsed.is_ok() {
                    key = None;
                }
                parsed
            });
            if let Err(error) = parsed
                && derive.reports(key)
            {
                errors.push(error);
            }
        }
        out
    }

    fn parse_key(&mut self, meta: &ParseNestedMeta) -> Result<()> {
        if meta.path.is_ident("rename_all") {
            let lit: LitStr = meta.value()?.parse()?;
            let Some(case) = Case::from_name(&lit.value()) else {
                return Err(Error::new_spanned(
                    &lit,
                    format!("unknown case: expected {}", Case::NAMES),
                ));
            };
            once(&mut self.rename_all, case, meta)
        } else if meta.path.is_ident("table") {
            let lit = name_value(meta, "a table name")?;
            once(&mut self.table, lit, meta)
        } else if meta.path.is_ident("schema") {
            let lit = name_value(meta, "a schema name")?;
            once(&mut self.schema, lit, meta)
        } else if meta.path.is_ident("model") {
            let ty: Type = meta.value()?.parse()?;
            once(&mut self.model, ty, meta)
        } else {
            Err(unknown(meta, "a struct", CONTAINER_KEYS))
        }
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
    pub primary_key: Option<Keyed<()>>,
    pub generated: Option<Keyed<()>>,
    pub version: Option<Keyed<()>>,
    pub updated_at: Option<Keyed<()>>,
    pub conflict: Option<Keyed<()>>,
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
    pub fn parse(attrs: &[Attribute], derive: Derive, errors: &mut Errors) -> Self {
        let mut out = FieldAttrs::default();
        for attr in shelf_attrs(attrs) {
            // The key being parsed, so that its error goes to the derive that owns it.
            let mut key = None;
            let parsed = attr.parse_nested_meta(|meta| {
                key = known(&meta, FIELD_KEYS);
                let parsed = out.parse_key(&meta);
                if parsed.is_ok() {
                    key = None;
                }
                parsed
            });
            if let Err(error) = parsed
                && derive.reports(key)
            {
                errors.push(error);
            }
        }
        if derive == Derive::FromRow {
            out.check_combinations(errors);
        }
        out
    }

    fn parse_key(&mut self, meta: &ParseNestedMeta) -> Result<()> {
        if meta.path.is_ident("rename") {
            let lit = name_value(meta, "a column name")?;
            once(&mut self.rename, lit, meta)
        } else if meta.path.is_ident("skip") {
            flag(&mut self.skip, meta)
        } else if meta.path.is_ident("flatten") {
            flag(&mut self.flatten, meta)
        } else if meta.path.is_ident("json") {
            flag(&mut self.json, meta)
        } else if meta.path.is_ident("try_from") {
            let ty = type_value(meta)?;
            once(&mut self.try_from, ty, meta)
        } else if meta.path.is_ident("primary_key") {
            flag(&mut self.primary_key, meta)
        } else if meta.path.is_ident("generated") {
            flag(&mut self.generated, meta)
        } else if meta.path.is_ident("version") {
            flag(&mut self.version, meta)
        } else if meta.path.is_ident("updated_at") {
            flag(&mut self.updated_at, meta)
        } else if meta.path.is_ident("conflict") {
            flag(&mut self.conflict, meta)
        } else {
            Err(unknown(meta, "a field", FIELD_KEYS))
        }
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

/// The key `meta` names, if it is one of `keys`.
fn known(meta: &ParseNestedMeta, keys: &[&'static str]) -> Option<&'static str> {
    keys.iter().copied().find(|key| meta.path.is_ident(key))
}

/// A name after `=`: a string literal that is not empty.
fn name_value(meta: &ParseNestedMeta, what: &str) -> Result<LitStr> {
    let lit: LitStr = meta.value()?.parse()?;
    if lit.value().is_empty() {
        return Err(Error::new_spanned(&lit, format!("{what} cannot be empty")));
    }
    Ok(lit)
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
