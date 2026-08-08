use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned;

pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = syn::parse_macro_input!(item as syn::ItemFn);
    match expand_inner(attr, func) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_inner(attr: TokenStream, func: syn::ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[flyte::main] takes no arguments",
        ));
    }
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            func.sig.fn_token.span(),
            "#[flyte::main] requires an async fn (the same fn as #[flyte::task])",
        ));
    }
    if func.sig.ident == "main" {
        return Err(syn::Error::new(
            func.sig.ident.span(),
            "#[flyte::main] goes on the task fn, not on `main` — it generates `main` for you",
        ));
    }

    let entry_ident = syn::Ident::new(
        &format!("{}_entry", func.sig.ident),
        func.sig.ident.span(),
    );

    // The fn passes through untouched; we only add `main` beside it. That is what
    // makes the attribute order-independent with respect to #[flyte::task].
    Ok(quote! {
        #func

        fn main() -> ::std::process::ExitCode {
            ::flyte::worker_main(#entry_ident())
        }
    })
}
