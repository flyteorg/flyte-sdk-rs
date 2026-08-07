use proc_macro::TokenStream;
use quote::quote;

pub fn expand(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    quote! {
        impl #impl_generics ::flyte::FlyteType for #name #ty_generics #where_clause {
            fn literal_type() -> ::flyte::idl::LiteralType {
                ::flyte::types::struct_literal_type()
            }
            fn to_literal(&self) -> ::core::result::Result<::flyte::idl::Literal, ::flyte::Error> {
                ::flyte::types::msgpack_to_literal(self)
            }
            fn from_literal(
                lit: &::flyte::idl::Literal,
            ) -> ::core::result::Result<Self, ::flyte::Error> {
                ::flyte::types::msgpack_from_literal(lit)
            }
        }
    }
    .into()
}
