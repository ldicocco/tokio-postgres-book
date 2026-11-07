//! `#[derive(FromRow)]`: `resolve` finds and checks each field's column once per statement,
//! and `from_row_at` reads a row through the positions it found.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::{Data, DeriveInput, Error, Field, Fields, Index, Result, Token, parse_quote};

use crate::attrs::{ContainerAttrs, Errors, FieldAttrs, Read};
use crate::generics::Bounds;

/// The `impl shelf::FromRow` for `input`, or every error found in it.
pub fn expand(input: &DeriveInput) -> Result<TokenStream> {
    let fields = named_fields(input)?;

    let mut errors = Errors::default();
    for lifetime in input.generics.lifetimes() {
        errors.push(Error::new_spanned(
            lifetime,
            "FromRow reads owned values, so the struct cannot borrow from the row: use `String` \
             instead of `&str`, and `Vec<u8>` instead of `&[u8]`",
        ));
    }
    let container = ContainerAttrs::parse(&input.attrs, &mut errors);
    let fields: Vec<(&Field, FieldAttrs)> = fields
        .iter()
        .map(|field| (field, FieldAttrs::parse(&field.attrs, &mut errors)))
        .collect();
    errors.finish()?;

    let model = input.ident.to_string();
    let mut bounds = Bounds::new(&input.generics);
    let mut column_types = Vec::new(); // the tuple `Columns`, one element per field
    let mut resolves = Vec::new(); // `resolve`'s tuple, element by element
    let mut reads = Vec::new(); // `from_row_at`'s field initializers

    for (position, (field, attrs)) in fields.iter().enumerate() {
        let ident = field.ident.as_ref().expect("named fields have identifiers");
        // A raw identifier names the column without its prefix: `r#type` reads `type`.
        let name = ident.unraw().to_string();
        let ty = &field.ty;
        let at = Index::from(position);

        // The type `resolve` checks the column against, and the expression that reads it.
        let (checked, read) = match attrs.read() {
            Read::Skip => {
                bounds.add_if_generic(&[ty], parse_quote!(#ty: ::core::default::Default));
                column_types.push(quote!(()));
                resolves.push(quote!(()));
                reads.push(quote!(#ident: ::core::default::Default::default()));
                continue;
            }
            Read::Flatten => {
                bounds.add_if_generic(&[ty], parse_quote!(#ty: ::shelf::FromRow));
                column_types.push(quote!(<#ty as ::shelf::FromRow>::Columns));
                resolves.push(quote!(<#ty as ::shelf::FromRow>::resolve(columns)?));
                reads.push(quote! {
                    #ident: <#ty as ::shelf::FromRow>::from_row_at(row, &columns.#at)?
                });
                continue;
            }
            Read::Column => {
                bounds.add_if_generic(&[ty], parse_quote!(#ty: ::shelf::FromColumn));
                (
                    quote!(#ty),
                    quote!(::shelf::__private::get::<#ty>(row, columns.#at, #model, #name)?),
                )
            }
            Read::Json => {
                let json = quote!(::shelf::__private::Json<#ty>);
                bounds.add_if_generic(&[ty], parse_quote!(#json: ::shelf::FromColumn));
                (
                    json.clone(),
                    quote!(::shelf::__private::get::<#json>(row, columns.#at, #model, #name)?.0),
                )
            }
            Read::TryFrom(source) => {
                bounds.add_if_generic(&[source], parse_quote!(#source: ::shelf::FromColumn));
                bounds.add_if_generic(
                    &[ty, source],
                    parse_quote!(#ty: ::core::convert::TryFrom<#source>),
                );
                bounds.add_if_generic(
                    &[ty, source],
                    parse_quote! {
                        <#ty as ::core::convert::TryFrom<#source>>::Error:
                            ::std::error::Error + ::core::marker::Send + ::core::marker::Sync + 'static
                    },
                );
                (
                    quote!(#source),
                    quote! {
                        ::shelf::__private::get_try_from::<#source, #ty>(
                            row, columns.#at, #model, #name,
                        )?
                    },
                )
            }
        };
        let column = match &attrs.rename {
            Some(rename) => rename.value.value(),
            None => container.column_for(&name),
        };
        column_types.push(quote!(usize));
        resolves.push(quote! {
            ::shelf::__private::column::<#checked>(columns, #model, #name, #column)?
        });
        reads.push(quote!(#ident: #read));
    }

    // With nothing to read (every field skipped, or none at all), the parameters go unused.
    let reads_any = column_types.iter().any(|t| t.to_string() != "()");
    let (row, columns) = if reads_any {
        (quote!(row), quote!(columns))
    } else {
        (quote!(_row), quote!(_columns))
    };

    let name = &input.ident;
    let generics = bounds.apply(&input.generics);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Every path is absolute (`::shelf`, `::core`): the expansion lands in the user's module,
    // where `Result` or even `shelf` may mean something else.
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::shelf::FromRow for #name #ty_generics #where_clause {
            type Columns = ( #(#column_types,)* );

            fn resolve(
                #columns: &[::shelf::__private::tokio_postgres::Column],
            ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
                ::core::result::Result::Ok(( #(#resolves,)* ))
            }

            fn from_row_at(
                #row: &::shelf::__private::tokio_postgres::Row,
                #columns: &Self::Columns,
            ) -> ::core::result::Result<Self, ::shelf::Error> {
                ::core::result::Result::Ok(Self { #(#reads,)* })
            }
        }
    })
}

/// The struct's named fields, or an error that says why the input cannot be mapped.
fn named_fields(input: &DeriveInput) -> Result<&Punctuated<Field, Token![,]>> {
    match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => Ok(&fields.named),
            _ => Err(Error::new_spanned(
                &input.ident,
                "FromRow needs named fields: each one is read from the column of that name",
            )),
        },
        Data::Enum(data) => Err(Error::new_spanned(
            data.enum_token,
            "FromRow maps a row to a struct; for a Postgres enum, derive \
             postgres_types::FromSql instead",
        )),
        Data::Union(data) => Err(Error::new_spanned(
            data.union_token,
            "FromRow cannot be derived for a union",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_of(input: DeriveInput) -> String {
        let errors: Vec<String> = expand(&input)
            .unwrap_err()
            .into_iter()
            .map(|e| e.to_string())
            .collect();
        errors.join("\n")
    }

    #[test]
    fn an_enum_is_refused_at_its_keyword() {
        let message = error_of(parse_quote! { enum Status { Pending, Paid } });
        assert!(
            message.contains("derive postgres_types::FromSql"),
            "{message}"
        );
    }

    #[test]
    fn a_tuple_struct_is_refused() {
        let message = error_of(parse_quote! { struct Pair(i32, i32); });
        assert!(
            message.starts_with("FromRow needs named fields"),
            "{message}"
        );
    }

    #[test]
    fn errors_from_several_fields_are_reported_together() {
        let message = error_of(parse_quote! {
            #[shelf(rename_all = "kebab-case")]
            struct Product<'a> {
                #[shelf(renamed = "x")]
                sku: &'a str,
                #[shelf(skip, rename = "y")]
                price: i64,
            }
        });
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(lines.len(), 4, "{message}");
        assert!(lines[0].starts_with("FromRow reads owned values"));
        assert!(lines[1].starts_with("unknown case"));
        assert!(lines[2].starts_with("unknown shelf attribute `renamed` on a field"));
        assert!(lines[3].starts_with("`rename` cannot apply to a field marked `skip`"));
    }

    #[test]
    fn a_key_given_twice_is_refused() {
        let message = error_of(parse_quote! {
            struct P { #[shelf(rename = "a")] #[shelf(rename = "b")] x: i32 }
        });
        assert_eq!(message, "duplicate `rename`");
    }

    #[test]
    fn a_flag_with_a_value_is_refused() {
        let message = error_of(parse_quote! { struct P { #[shelf(skip = true)] x: i32 } });
        assert_eq!(message, "`skip` takes no value");
    }
}
