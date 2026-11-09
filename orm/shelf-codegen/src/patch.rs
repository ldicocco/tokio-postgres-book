//! `#[derive(Patch)]`: a partial update of a model's row, as one statement with a flag beside
//! each value (orm/PLAN.md §10). The statement itself is built once, at run time, from the
//! model's constants (`shelf::__private::patch_sql`): its `$n` numbers depend on the model's key.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Error, Fields, Result, Type};

use crate::attrs::{ContainerAttrs, Derive, Errors, FieldAttrs};
use crate::sql::quote_ident;

/// The `impl shelf::Patch` for `input`, or every error found in it.
pub fn expand(input: &DeriveInput) -> Result<TokenStream> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(Error::new_spanned(
                    &input.ident,
                    "Patch needs named fields: each one may set the column of that name",
                ));
            }
        },
        _ => {
            return Err(Error::new_spanned(
                &input.ident,
                "Patch needs a struct, whose fields are the columns it may set",
            ));
        }
    };

    let mut errors = Errors::default();
    for param in input.generics.type_params() {
        errors.push(Error::new_spanned(
            param,
            "a Patch struct cannot have type parameters: its statement is built once for its \
             type (lifetimes are fine, for fields that borrow)",
        ));
    }
    let container = ContainerAttrs::parse(&input.attrs, Derive::Patch, &mut errors);
    for (name, keyed) in [("table", &container.table), ("schema", &container.schema)] {
        if let Some(keyed) = keyed {
            errors.push(Error::new(
                keyed.span,
                format!("`{name}` belongs on the model: a Patch changes its model's table"),
            ));
        }
    }

    let mut columns = Vec::new(); // (field, quoted column)
    let mut version = None; // the field that holds the version the patch expects
    for field in fields {
        let attrs = FieldAttrs::parse(&field.attrs, Derive::Patch, &mut errors);
        let meaningless = [
            ("flatten", &attrs.flatten),
            ("json", &attrs.json),
            ("primary_key", &attrs.primary_key),
            ("generated", &attrs.generated),
            ("updated_at", &attrs.updated_at),
            ("conflict", &attrs.conflict),
        ];
        for (name, keyed) in meaningless {
            if let Some(keyed) = keyed {
                errors.push(Error::new(
                    keyed.span,
                    format!("`{name}` has no meaning on a Patch struct; it belongs on the model"),
                ));
            }
        }
        if let Some(try_from) = &attrs.try_from {
            errors.push(Error::new(
                try_from.span,
                "`try_from` has no meaning on a Patch struct, which writes each value as it is",
            ));
        }
        if attrs.skip.is_some() {
            continue;
        }
        let ident = field.ident.as_ref().expect("named fields have identifiers");
        if let Some(keyed) = &attrs.version {
            if version.is_some() {
                errors.push(Error::new(keyed.span, "a Patch has one `version` field"));
            }
            version = Some(ident);
            continue;
        }
        if !is_option(&field.ty) {
            errors.push(Error::new_spanned(
                &field.ty,
                "a Patch field is an `Option`: `None` leaves the column as it is and `Some` sets \
                 it (`Option<Option<T>>` for a nullable column, whose `Some(None)` sets NULL)",
            ));
            continue;
        }
        let column = match &attrs.rename {
            Some(rename) => rename.value.value(),
            None => container.column_for(&ident.unraw().to_string()),
        };
        columns.push((ident, quote_ident(&column)));
    }
    let Some(model) = &container.model else {
        errors.push(Error::new_spanned(
            &input.ident,
            "Patch needs the model whose row it changes: #[shelf(model = Product)]",
        ));
        return Err(errors.finish().expect_err("an error was pushed"));
    };
    if columns.is_empty() && version.is_none() {
        errors.push(Error::new_spanned(
            &input.ident,
            "a Patch needs at least one field to set",
        ));
    }
    errors.finish()?;
    let model = &model.value;

    let quoted = columns.iter().map(|(_, column)| column);
    let quoted_again = quoted.clone();
    let mut params: Vec<TokenStream> = columns
        .iter()
        .flat_map(|(ident, _)| {
            [
                quote!(::shelf::__private::flag(&self.#ident)),
                quote!(::shelf::__private::param(&self.#ident)),
            ]
        })
        .collect();
    if let Some(ident) = version {
        params.push(quote!(::shelf::__private::param(&self.#ident)));
    }

    // A model with a version needs the patch to say which version it was made against, and a
    // model without one has nothing to check it against: a mismatch fails the build.
    let model_name = model.to_token_stream().to_string();
    let has_version = version.is_some();
    let mismatched = if has_version {
        quote!(is_none)
    } else {
        quote!(is_some)
    };
    let mismatch = if has_version {
        format!(
            "{}: the model {model_name} has no `version` column, so the patch's \
             #[shelf(version)] field has nothing to be checked against",
            input.ident
        )
    } else {
        format!(
            "{}: the model {model_name} has a `version` column, so the patch needs a \
             #[shelf(version)] field holding the version it was made against",
            input.ident
        )
    };

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let to_sql =
        quote!(&(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync));
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::shelf::Patch for #name #ty_generics #where_clause {
            type Model = #model;

            const COLUMNS: &'static [&'static ::core::primitive::str] = &[#(#quoted),*];

            fn sql() -> &'static ::core::primitive::str {
                static SQL: ::std::sync::LazyLock<::std::string::String> =
                    ::std::sync::LazyLock::new(|| {
                        ::shelf::__private::patch_sql::<#model>(&[#(#quoted_again),*])
                    });
                ::std::string::String::as_str(&SQL)
            }

            fn params(&self) -> ::std::vec::Vec<#to_sql> {
                ::std::vec::Vec::from([#(#params),*])
            }
        }

        const _: () = {
            if <#model as ::shelf::Table>::VERSION.#mismatched() {
                ::core::panic!(#mismatch);
            }
        };
    })
}

/// Whether `ty` is written `Option<…>` (or `std::option::Option<…>`): a patch field must be.
fn is_option(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => {
            path.qself.is_none()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "Option")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn a_patch_field_must_be_an_option() {
        let input: DeriveInput = parse_quote! {
            #[shelf(model = P)]
            struct PPatch { sku: String, note: Option<Option<String>> }
        };
        let errors: Vec<String> = expand(&input)
            .unwrap_err()
            .into_iter()
            .map(|e| e.to_string())
            .collect();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("a Patch field is an `Option`"));
    }
}
