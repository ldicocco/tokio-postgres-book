//! `#[derive(Table)]`: a `FromRow` struct's table, key and column list, and the SQL that reads
//! it, written at macro time as string literals.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{DeriveInput, Error, Field, Index, Result};

use crate::attrs::{ContainerAttrs, Derive, Errors, FieldAttrs};
use crate::columns;
use crate::from_row::named_fields;
use crate::relations;
use crate::sql::{column_list, equal_params, quote_ident};

/// The `impl shelf::Table` for `input`, or every error in the keys `Table` owns.
pub fn expand(input: &DeriveInput) -> Result<TokenStream> {
    // A struct that FromRow refuses gets nothing from Table either: FromRow, which every Table
    // has, reports the mistake, once.
    let Ok(fields) = named_fields(input) else {
        return Ok(TokenStream::new());
    };

    let mut errors = Errors::default();
    for param in input.generics.type_params() {
        errors.push(Error::new_spanned(
            param,
            "a Table model cannot be generic: its SQL names one table",
        ));
    }
    let container = ContainerAttrs::parse(&input.attrs, Derive::Table, &mut errors);
    let fields: Vec<(&Field, FieldAttrs)> = fields
        .iter()
        .map(|field| {
            (
                field,
                FieldAttrs::parse(&field.attrs, Derive::Table, &mut errors),
            )
        })
        .collect();

    let mut columns = Vec::new(); // every column the model reads, in field order
    let mut key = Vec::new(); // the key's fields and columns
    let mut sets = Vec::new(); // what `update` writes: neither key nor generated
    let mut version = None; // the version column and its field, for optimistic locking
    let mut updated_at = None; // the column `update` sets to `now()`
    let mut constants = Vec::new(); // the query builder's `Product::SKU`, one per column
    let mut checks = Vec::new(); // `FIELDS`, what `verify` checks, one per column
    for (field, attrs) in &fields {
        if let Some(flatten) = &attrs.flatten {
            errors.push(Error::new(
                flatten.span,
                "a Table model lists its own columns, and a flattened struct's are not known \
                 here: write them out as fields, or read this struct with FromRow and SQL",
            ));
            continue;
        }
        if attrs.skip.is_some() {
            for (name, keyed) in [
                ("primary_key", &attrs.primary_key),
                ("generated", &attrs.generated),
                ("version", &attrs.version),
                ("updated_at", &attrs.updated_at),
            ] {
                if let Some(keyed) = keyed {
                    errors.push(Error::new(
                        keyed.span,
                        format!(
                            "`{name}` cannot apply to a field marked `skip`, which is not a column"
                        ),
                    ));
                }
            }
            continue;
        }
        let ident = field.ident.as_ref().expect("named fields have identifiers");
        let column = match &attrs.rename {
            Some(rename) => rename.value.value(),
            None => container.column_for(&ident.unraw().to_string()),
        };
        if let (Some(_), Some(keyed)) = (
            &attrs.primary_key,
            attrs.version.as_ref().or(attrs.updated_at.as_ref()),
        ) {
            errors.push(Error::new(
                keyed.span,
                "a key column cannot also be a version or an `updated_at`: an update would change \
                 the key it looks the row up by",
            ));
        }
        for (name, keyed, slot) in [
            ("version", &attrs.version, &mut version),
            ("updated_at", &attrs.updated_at, &mut updated_at),
        ] {
            if let Some(keyed) = keyed {
                if slot.is_some() {
                    errors.push(Error::new(
                        keyed.span,
                        format!("a model has one `{name}` column"),
                    ));
                }
                *slot = Some((ident, column.clone()));
            }
        }
        if attrs.primary_key.is_some() {
            key.push((*field, column.clone()));
        } else if attrs.generated.is_none() && attrs.version.is_none() && attrs.updated_at.is_none()
        {
            sets.push((ident, column.clone(), attrs.json.is_some()));
        }
        constants.push(columns::constant(field, attrs, &column));
        checks.push(columns::field(field, attrs, &column));
        columns.push(column);
    }
    let Some(table) = &container.table else {
        errors.push(Error::new_spanned(
            &input.ident,
            "Table needs the table's name: #[shelf(table = \"…\")]",
        ));
        return Err(errors.finish().expect_err("an error was pushed"));
    };
    let mut methods = Vec::new(); // what `has_many` and `belongs_to` add
    if key.is_empty() {
        errors.push(Error::new_spanned(
            &input.ident,
            "Table needs a key: mark its column, or columns, #[shelf(primary_key)]",
        ));
    } else {
        methods = relations::methods(input, &container.relations, key.len(), &fields, &mut errors);
    }
    errors.finish()?;
    // A mistake in a key FromRow owns: FromRow reports it.
    if crate::from_row::expand(input).is_err() {
        return Ok(TokenStream::new());
    }

    let table = match &container.schema {
        Some(schema) => format!(
            "{}.{}",
            quote_ident(&schema.value.value()),
            quote_ident(&table.value.value())
        ),
        None => quote_ident(&table.value.value()),
    };
    let column_sql = column_list(&columns);
    let key_columns: Vec<String> = key.iter().map(|(_, column)| column.clone()).collect();
    let find = format!(
        "SELECT {column_sql} FROM {table} WHERE {}",
        equal_params(&key_columns, 1)
    );
    let all = format!(
        "SELECT {column_sql} FROM {table} ORDER BY {}",
        column_list(&key_columns)
    );
    let count = format!("SELECT count(*) FROM {table}");
    // `update`: the columns it writes ($1…), then the version it expects, then the key.
    let mut set: Vec<String> = sets
        .iter()
        .enumerate()
        .map(|(i, (_, column, _))| format!("{} = ${}", quote_ident(column), i + 1))
        .collect();
    if let Some((_, column)) = &version {
        set.push(format!("{0} = {0} + 1", quote_ident(column)));
    }
    if let Some((_, column)) = &updated_at {
        set.push(format!("{} = now()", quote_ident(column)));
    }
    let update = match &version {
        // Nothing to write: the row as it is.
        _ if set.is_empty() => find.clone(),
        None => format!(
            "UPDATE {table} SET {} WHERE {} RETURNING {column_sql}",
            set.join(", "),
            equal_params(&key_columns, sets.len() + 1)
        ),
        Some((_, column)) => versioned_update(
            &table,
            &set.join(", "),
            &equal_params(&key_columns, sets.len() + 2),
            &quote_ident(column),
            sets.len() + 1,
            &column_sql,
        ),
    };
    let key_column_list = key_columns.iter().map(|c| quote_ident(c));
    let version_const = match &version {
        Some((_, column)) => {
            let column = quote_ident(column);
            quote!(::core::option::Option::Some(#column))
        }
        None => quote!(::core::option::Option::None),
    };
    let updated_at_const = match &updated_at {
        Some((_, column)) => {
            let column = quote_ident(column);
            quote!(::core::option::Option::Some(#column))
        }
        None => quote!(::core::option::Option::None),
    };
    let delete = format!(
        "DELETE FROM {table} WHERE {}",
        equal_params(&key_columns, 1)
    );
    // A `json` field is written through shelf's `Json`, seen in place (`Json::from_ref`).
    let mut set_params: Vec<TokenStream> = sets
        .iter()
        .map(|(ident, _, json)| {
            if *json {
                quote!(::shelf::__private::param(::shelf::__private::Json::from_ref(&self.#ident)))
            } else {
                quote!(::shelf::__private::param(&self.#ident))
            }
        })
        .collect();
    if let Some((ident, _)) = &version {
        set_params.push(quote!(::shelf::__private::param(&self.#ident)));
    }

    // The key: the field's own type, or a tuple of the fields' types for a composite key.
    let to_sql =
        quote!(&(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync));
    let (key_type, key_value, key_params) = if let [(field, _)] = key.as_slice() {
        let ty = &field.ty;
        let ident = &field.ident;
        (
            quote!(#ty),
            quote!(::core::clone::Clone::clone(&self.#ident)),
            quote!(::shelf::__private::param(key)),
        )
    } else {
        let types = key.iter().map(|(field, _)| &field.ty);
        let idents = key.iter().map(|(field, _)| &field.ident);
        let at = (0..key.len()).map(Index::from);
        (
            quote!(( #(#types,)* )),
            quote!(( #(::core::clone::Clone::clone(&self.#idents),)* )),
            quote!(#(::shelf::__private::param(&key.#at)),*),
        )
    };

    let name = &input.ident;
    let model = name.to_string();
    Ok(quote! {
        #[automatically_derived]
        impl ::shelf::Table for #name {
            type Key = #key_type;

            const MODEL: &'static ::core::primitive::str = #model;
            const TABLE: &'static ::core::primitive::str = #table;
            const COLUMNS: &'static ::core::primitive::str = #column_sql;
            const FIND: &'static ::core::primitive::str = #find;
            const ALL: &'static ::core::primitive::str = #all;
            const COUNT: &'static ::core::primitive::str = #count;
            const UPDATE: &'static ::core::primitive::str = #update;
            const DELETE: &'static ::core::primitive::str = #delete;
            const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &[#(#key_column_list),*];
            const VERSION: ::core::option::Option<&'static ::core::primitive::str> = #version_const;
            const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> =
                #updated_at_const;
            const FIELDS: &'static [::shelf::__private::Field] = &[#(#checks),*];

            fn key(&self) -> Self::Key {
                #key_value
            }

            fn key_params(key: &Self::Key) -> ::std::vec::Vec<#to_sql> {
                ::std::vec::Vec::from([#key_params])
            }

            fn update_params(&self) -> ::std::vec::Vec<#to_sql> {
                ::std::vec::Vec::from([#(#set_params),*])
            }
        }

        // Unused constants and relations are not worth a warning: a model has one per column.
        #[automatically_derived]
        #[allow(dead_code)]
        impl #name {
            #(#constants)*
            #(#methods)*
        }
    })
}

/// An `UPDATE` that also says why it changed nothing: the one row it returns has the updated
/// columns (NULL if nothing was updated), `shelf_updated` (1 or 0), and `shelf_version`, the
/// row's version before the update (NULL if there is no row). So a stale version and a missing
/// row are told apart in one round trip (orm/PLAN.md §12 #31). shelf's `Patch` builds the same
/// statement at run time (`shelf/src/patch.rs`); the two must agree.
fn versioned_update(
    table: &str,
    set: &str,
    key_where: &str,
    version: &str,
    version_param: usize,
    columns: &str,
) -> String {
    format!(
        "WITH updated AS (UPDATE {table} SET {set} WHERE {key_where} AND {version} = \
         ${version_param} RETURNING {columns}) SELECT {columns}, (SELECT count(*) FROM updated) \
         AS shelf_updated, (SELECT {version}::bigint FROM {table} WHERE {key_where}) AS \
         shelf_version FROM (VALUES (1)) AS one LEFT JOIN updated ON true"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn errors_of(input: DeriveInput) -> Vec<String> {
        expand(&input)
            .unwrap_err()
            .into_iter()
            .map(|e| e.to_string())
            .collect()
    }

    #[test]
    fn a_table_needs_a_name_and_a_key() {
        let errors = errors_of(parse_quote! { struct P { id: i64 } });
        assert_eq!(
            errors,
            ["Table needs the table's name: #[shelf(table = \"…\")]"]
        );
        let errors = errors_of(parse_quote! { #[shelf(table = "p")] struct P { id: i64 } });
        assert!(errors[0].starts_with("Table needs a key"), "{errors:?}");
    }

    #[test]
    fn table_leaves_the_keys_it_does_not_own_to_from_row() {
        // `renamed` is unknown and `rename = 1` malformed: FromRow reports both, Table nothing.
        let input: DeriveInput = parse_quote! {
            #[shelf(table = "p")]
            struct P { #[shelf(primary_key, renamed = "x")] id: i64, #[shelf(rename = 1)] n: i64 }
        };
        assert!(expand(&input).unwrap().is_empty());
        assert!(crate::from_row::expand(&input).is_err());
    }

    #[test]
    fn from_row_ignores_the_keys_table_owns() {
        let input: DeriveInput = parse_quote! {
            #[shelf(table = "products", schema = "inventory")]
            struct P { #[shelf(primary_key, generated)] id: i64 }
        };
        assert!(crate::from_row::expand(&input).is_ok());
    }
}
