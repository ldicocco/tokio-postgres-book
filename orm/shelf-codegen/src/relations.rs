//! `has_many` and `belongs_to`: the methods a relation adds to a model. Each one is a call to a
//! loader in `shelf/src/relations.rs`, which sends one query however many rows it loads for.
//!
//! - `#[shelf(has_many(OrderLine, key = order_id, as = lines))]` on `Order` adds
//!   `order.lines(&db)` and `Order::load_lines(&db, &orders)`.
//! - `#[shelf(belongs_to(Order, key = order_id))]` on `OrderLine` adds `line.order(&db)` and
//!   `OrderLine::load_order(&db, &lines)`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::{DeriveInput, Error, Field, Ident, Path, Type};

use crate::attrs::{Errors, FieldAttrs, Keyed, Relation, RelationKind};

/// The methods `relations` add to the model `input`, whose key has `key_len` columns.
pub fn methods(
    input: &DeriveInput,
    relations: &[Keyed<Relation>],
    key_len: usize,
    fields: &[(&Field, FieldAttrs)],
    errors: &mut Errors,
) -> Vec<TokenStream> {
    let mut out = Vec::new();
    for relation in relations {
        let Relation {
            kind, model, key, ..
        } = &relation.value;
        match kind {
            RelationKind::HasMany => {
                if key_len != 1 {
                    errors.push(Error::new(
                        relation.span,
                        "`has_many` loads by this model's key, which must be one column: \
                         a relation follows a single-column key",
                    ));
                    continue;
                }
                out.push(has_many(input, &relation.value));
            }
            RelationKind::BelongsTo => {
                let field = fields.iter().find(|(field, _)| {
                    field
                        .ident
                        .as_ref()
                        .is_some_and(|ident| ident.unraw() == key.unraw())
                });
                match field {
                    Some((field, attrs)) if attrs.skip.is_none() => {
                        out.push(belongs_to(input, &relation.value, &field.ty));
                    }
                    Some(_) => errors.push(Error::new_spanned(
                        key,
                        "`belongs_to`'s key is a field marked `skip`, which is not a column",
                    )),
                    None => errors.push(Error::new_spanned(
                        key,
                        format!(
                            "`belongs_to`'s key is a field of `{}` that holds the foreign key; \
                             it has no field `{key}` ({} points at `{}`'s key)",
                            input.ident,
                            input.ident,
                            last_ident(model),
                        ),
                    )),
                }
            }
        }
    }
    out
}

/// `order.lines(&db)` and `Order::load_lines(&db, &orders)`.
fn has_many(input: &DeriveInput, relation: &Relation) -> TokenStream {
    let Relation {
        model: child,
        key,
        name,
        ..
    } = relation;
    let name = name
        .as_ref()
        .expect("has_many requires `as`, checked when parsed");
    let load = format_ident!("load_{}", name.unraw());
    // The child's column constant, spanned on `key` so that a wrong name is reported there.
    let column = format_ident!(
        "{}",
        key.unraw().to_string().to_uppercase(),
        span = key.span()
    );
    let vis = &input.vis;
    let (parent, child_name) = (&input.ident, last_ident(child));
    let one_doc = format!(
        " The `{child_name}` rows whose `{key}` holds this `{parent}`'s key, in their key \
         order: one query."
    );
    let many_doc = format!(
        " The `{child_name}` rows of each of `parents`, in one query (`= ANY($1)`): the `Vec` \
         at `i` holds `parents[i]`'s."
    );
    let one = returns(quote!(::std::vec::Vec<#child>));
    let many = returns(quote!(::std::vec::Vec<::std::vec::Vec<#child>>));
    // The calls carry the span of `key = …`: a type that does not fit is reported there.
    let one_body = quote_spanned! {key.span()=>
        ::shelf::__private::children(db, #child::#column, <Self as ::shelf::Table>::key(self))
    };
    let many_body = quote_spanned! {key.span()=>
        ::shelf::__private::load_children(db, parents, #child::#column, |child: &#child| {
            &child.#key
        })
    };
    quote! {
        #[doc = #one_doc]
        #vis fn #name<'a, E: ::shelf::Executor>(&'a self, db: &'a E) -> #one {
            #one_body
        }

        #[doc = #many_doc]
        #vis fn #load<'a, E: ::shelf::Executor>(db: &'a E, parents: &'a [Self]) -> #many {
            #many_body
        }
    }
}

/// `line.order(&db)` and `OrderLine::load_order(&db, &lines)`. A nullable foreign key (a field
/// written `Option<…>`) makes the first one return an `Option`.
fn belongs_to(input: &DeriveInput, relation: &Relation, key_type: &Type) -> TokenStream {
    let Relation {
        model: parent, key, ..
    } = relation;
    let name = match &relation.name {
        Some(name) => name.clone(),
        None => format_ident!("{}", snake_case(&last_ident(parent).to_string())),
    };
    let load = format_ident!("load_{}", name.unraw());
    let vis = &input.vis;
    let parent_name = last_ident(parent);
    let optional = is_option(key_type);
    let (output, body) = if optional {
        (
            quote!(::core::option::Option<#parent>),
            quote_spanned! {key.span()=>
                ::shelf::__private::parent_opt::<#parent, E>(
                    db,
                    ::core::option::Option::as_ref(&self.#key),
                )
            },
        )
    } else {
        (
            quote!(#parent),
            quote_spanned!(key.span()=> <#parent as ::shelf::Table>::find(db, &self.#key)),
        )
    };
    let one_doc = match optional {
        true => format!(" The `{parent_name}` that `{key}` points at, or `None` if it is NULL."),
        false => format!(" The `{parent_name}` that `{key}` points at."),
    };
    let many_doc = format!(
        " The `{parent_name}` rows that `children` point at, by key, in one query (`= ANY($1)`)."
    );
    let one = returns(output);
    let many_body = quote_spanned! {key.span()=>
        ::shelf::__private::load_parents::<#parent, Self, _, E>(db, children, |child: &Self| {
            &child.#key
        })
    };
    let many = returns(quote! {
        ::std::collections::HashMap<<#parent as ::shelf::Table>::Key, #parent>
    });
    quote! {
        #[doc = #one_doc]
        #vis fn #name<'a, E: ::shelf::Executor>(&'a self, db: &'a E) -> #one {
            #body
        }

        #[doc = #many_doc]
        #vis fn #load<'a, E: ::shelf::Executor>(db: &'a E, children: &'a [Self]) -> #many {
            #many_body
        }
    }
}

/// A method's return type: a `Send` future of `Result<output, shelf::Error>`. `+ 'a` is spelled
/// out, so that the generated code means the same in an edition 2021 crate, where a returned
/// `impl Trait` captures only the lifetimes it names.
fn returns(output: TokenStream) -> TokenStream {
    quote! {
        impl ::core::future::Future<Output = ::core::result::Result<#output, ::shelf::Error>>
            + ::core::marker::Send
            + 'a
    }
}

fn last_ident(path: &Path) -> &Ident {
    &path.segments.last().expect("a path has a segment").ident
}

/// `OrderLine` → `order_line`: a method named after a model, without guessing any plural.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether `ty` is written `Option<…>`: the derive sees tokens, not types, so an alias of
/// `Option` is not recognized.
fn is_option(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Option"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::snake_case;

    #[test]
    fn a_model_name_becomes_a_method_name() {
        assert_eq!(snake_case("Order"), "order");
        assert_eq!(snake_case("OrderLine"), "order_line");
        assert_eq!(snake_case("Store"), "store");
    }
}
