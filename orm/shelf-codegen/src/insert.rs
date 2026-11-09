//! `#[derive(Insert)]`: a `New…` struct's `INSERT … RETURNING`, and its upsert when fields are
//! marked `conflict`. The table and the columns read back are the model's constants, joined
//! into the statement at compile time (orm/PLAN.md §12 #32).

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Error, Fields, Ident, Result, Type};

use crate::attrs::{ContainerAttrs, Derive, Errors, FieldAttrs};
use crate::sql::{column_list, quote_ident};

/// A field `insert` writes.
struct Column<'a> {
    ident: &'a Ident,
    name: String,
    json: bool,
    conflict: bool,
}

/// The `impl shelf::Insert` (and `shelf::Upsert`) for `input`, or every error found in it.
pub fn expand(input: &DeriveInput) -> Result<TokenStream> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(Error::new_spanned(
                    &input.ident,
                    "Insert needs named fields: each one is written to the column of that name",
                ));
            }
        },
        _ => {
            return Err(Error::new_spanned(
                &input.ident,
                "Insert needs a struct, whose fields are the row's columns",
            ));
        }
    };

    let mut errors = Errors::default();
    for param in input.generics.type_params() {
        errors.push(Error::new_spanned(
            param,
            "an Insert struct cannot have type parameters: its SQL is a constant built from its \
             model's (lifetimes are fine, for fields that borrow)",
        ));
    }
    let container = ContainerAttrs::parse(&input.attrs, Derive::Insert, &mut errors);
    for (name, keyed) in [("table", &container.table), ("schema", &container.schema)] {
        if let Some(keyed) = keyed {
            errors.push(Error::new(
                keyed.span,
                format!(
                    "`{name}` belongs on the model: an Insert struct writes to its model's table"
                ),
            ));
        }
    }

    let mut columns = Vec::new();
    for field in fields {
        let attrs = FieldAttrs::parse(&field.attrs, Derive::Insert, &mut errors);
        let meaningless = [
            ("flatten", attrs.flatten.as_ref().map(|k| k.span)),
            ("try_from", attrs.try_from.as_ref().map(|k| k.span)),
            ("primary_key", attrs.primary_key.as_ref().map(|k| k.span)),
            ("generated", attrs.generated.as_ref().map(|k| k.span)),
            ("version", attrs.version.as_ref().map(|k| k.span)),
            ("updated_at", attrs.updated_at.as_ref().map(|k| k.span)),
        ];
        for (name, span) in meaningless {
            if let Some(span) = span {
                errors.push(Error::new(
                    span,
                    format!(
                        "`{name}` has no meaning on an Insert struct, which writes each field to \
                         its column; it belongs on the model"
                    ),
                ));
            }
        }
        if attrs.skip.is_some() {
            if let Some(conflict) = &attrs.conflict {
                errors.push(Error::new(
                    conflict.span,
                    "`conflict` cannot apply to a field marked `skip`, which is not written",
                ));
            }
            continue;
        }
        let ident = field.ident.as_ref().expect("named fields have identifiers");
        let name = match &attrs.rename {
            Some(rename) => rename.value.value(),
            None => container.column_for(&ident.unraw().to_string()),
        };
        columns.push(Column {
            ident,
            name,
            json: attrs.json.is_some(),
            conflict: attrs.conflict.is_some(),
        });
    }
    let Some(model) = &container.model else {
        errors.push(Error::new_spanned(
            &input.ident,
            "Insert needs the model the row becomes: #[shelf(model = Product)]",
        ));
        return Err(errors.finish().expect_err("an error was pushed"));
    };
    errors.finish()?;
    let model = &model.value;

    // The statement's own text; the model's table and columns are joined in at compile time.
    let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let values = if columns.is_empty() {
        " DEFAULT VALUES".to_owned()
    } else {
        let placeholders: Vec<String> = (1..=columns.len()).map(|n| format!("${n}")).collect();
        format!(
            " ({}) VALUES ({})",
            column_list(&names),
            placeholders.join(", ")
        )
    };
    let insert = joined(model, &format!("{values} RETURNING "));

    let params = columns.iter().map(|column| {
        let ident = column.ident;
        if column.json {
            quote!(::shelf::__private::param(::shelf::__private::Json::from_ref(&self.#ident)))
        } else {
            quote!(::shelf::__private::param(&self.#ident))
        }
    });

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let to_sql =
        quote!(&(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync));
    let mut out = quote! {
        #[automatically_derived]
        impl #impl_generics ::shelf::Insert for #name #ty_generics #where_clause {
            type Model = #model;

            const INSERT: &'static ::core::primitive::str = #insert;

            fn params(&self) -> ::std::vec::Vec<#to_sql> {
                ::std::vec::Vec::from([#(#params),*])
            }
        }
    };

    let conflict: Vec<String> = columns
        .iter()
        .filter(|c| c.conflict)
        .map(|c| c.name.clone())
        .collect();
    if !conflict.is_empty() {
        // Every other column takes the new value. With none, the conflict key is set to itself,
        // so that the existing row is still returned (`DO NOTHING` would return no row).
        let mut updated: Vec<&String> = names.iter().filter(|n| !conflict.contains(n)).collect();
        if updated.is_empty() {
            updated = conflict.iter().take(1).collect();
        }
        let set: Vec<String> = updated
            .iter()
            .map(|n| format!("{0} = excluded.{0}", quote_ident(n)))
            .collect();
        let on_conflict = format!(
            " ON CONFLICT ({}) DO UPDATE SET {} RETURNING ",
            column_list(&conflict),
            set.join(", ")
        );
        let upsert = joined(model, &format!("{values}{on_conflict}"));
        out.extend(quote! {
            #[automatically_derived]
            impl #impl_generics ::shelf::Upsert for #name #ty_generics #where_clause {
                const UPSERT: &'static ::core::primitive::str = #upsert;
            }
        });
    }
    Ok(out)
}

/// A block that joins, at compile time, `INSERT INTO <model's table>`, `middle`, and the
/// model's columns: `RETURNING` reads back what the model reads.
fn joined(model: &Type, middle: &str) -> TokenStream {
    quote! {
        {
            const PARTS: &[&::core::primitive::str] = &[
                "INSERT INTO ",
                <#model as ::shelf::Table>::TABLE,
                #middle,
                <#model as ::shelf::Table>::COLUMNS,
            ];
            const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] =
                ::shelf::__private::join(PARTS);
            ::shelf::__private::utf8(&BYTES)
        }
    }
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
    fn an_insert_needs_its_model() {
        let errors = errors_of(parse_quote! { struct NewP { sku: String } });
        assert_eq!(
            errors,
            ["Insert needs the model the row becomes: #[shelf(model = Product)]"]
        );
    }

    #[test]
    fn insert_reports_every_key_it_cannot_use() {
        let errors = errors_of(parse_quote! {
            #[shelf(model = P, table = "p")]
            struct NewP { #[shelf(primary_key)] id: i64, #[shelf(renamed = "x")] sku: String }
        });
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].starts_with("`table` belongs on the model"));
        // Field by field, in the order of the source.
        assert!(errors[1].starts_with("`primary_key` has no meaning"));
        assert!(errors[2].starts_with("unknown shelf attribute `renamed`"));
    }
}
