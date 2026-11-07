//! Bounds for generic structs: a field whose type mentions a type parameter needs a bound
//! on that type, such as `T: shelf::FromColumn`, before the generated code can read it.

use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::{Generics, Ident, Type, WherePredicate};

/// The struct's type parameters, and the predicates the derive adds for them.
pub struct Bounds {
    params: Vec<Ident>,
    predicates: Vec<WherePredicate>,
}

impl Bounds {
    pub fn new(generics: &Generics) -> Bounds {
        Bounds {
            params: generics.type_params().map(|p| p.ident.clone()).collect(),
            predicates: Vec::new(),
        }
    }

    /// Whether `ty` mentions one of the type parameters, anywhere in its tokens.
    pub fn mentions_param(&self, ty: &Type) -> bool {
        !self.params.is_empty() && mentions(ty.to_token_stream(), &self.params)
    }

    /// Adds `predicate` if any of `types` mentions a type parameter. A bound on a concrete
    /// type would be checked where the derive is, far from the field, so it is left out: the
    /// generated code reports a concrete type that does not fit at the field itself.
    pub fn add_if_generic(&mut self, types: &[&Type], predicate: WherePredicate) {
        if types.iter().any(|ty| self.mentions_param(ty)) {
            self.predicates.push(predicate);
        }
    }

    /// `generics` with the predicates added to its `where` clause.
    pub fn apply(self, generics: &Generics) -> Generics {
        let mut generics = generics.clone();
        if !self.predicates.is_empty() {
            generics
                .make_where_clause()
                .predicates
                .extend(self.predicates);
        }
        generics
    }
}

fn mentions(tokens: TokenStream, params: &[Ident]) -> bool {
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => params.contains(&ident),
        TokenTree::Group(group) => mentions(group.stream(), params),
        _ => false,
    })
}
