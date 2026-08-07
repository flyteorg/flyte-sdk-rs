use md5::{Digest as _, Md5};
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::spanned::Spanned;

/// Extracted `(ok_type, is_unit)` from a `Result<T, E>` return type.
pub(crate) fn result_ok_type(sig: &syn::Signature) -> syn::Result<(syn::Type, bool)> {
    let err = || {
        syn::Error::new(
            sig.output.span(),
            "flyte fns must return Result<T, E> where T: FlyteType and E: From<flyte::Error>",
        )
    };
    let syn::ReturnType::Type(_, ty) = &sig.output else {
        return Err(err());
    };
    let syn::Type::Path(type_path) = ty.as_ref() else {
        return Err(err());
    };
    let last = type_path.path.segments.last().ok_or_else(err)?;
    if last.ident != "Result" {
        return Err(err());
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return Err(err());
    };
    let Some(syn::GenericArgument::Type(ok_ty)) = args.args.first() else {
        return Err(err());
    };
    let is_unit = matches!(ok_ty, syn::Type::Tuple(t) if t.elems.is_empty());
    Ok((ok_ty.clone(), is_unit))
}

/// Extracted `(ident, type)` pairs; only simple by-value ident params allowed.
pub(crate) fn typed_params(sig: &syn::Signature) -> syn::Result<Vec<(syn::Ident, syn::Type)>> {
    let mut params = Vec::new();
    for input in &sig.inputs {
        let syn::FnArg::Typed(pat_ty) = input else {
            return Err(syn::Error::new(
                input.span(),
                "flyte fns cannot take self parameters",
            ));
        };
        let syn::Pat::Ident(pat_ident) = pat_ty.pat.as_ref() else {
            return Err(syn::Error::new(
                pat_ty.pat.span(),
                "flyte fn parameters must be simple identifiers",
            ));
        };
        if matches!(pat_ty.ty.as_ref(), syn::Type::Reference(_)) {
            return Err(syn::Error::new(
                pat_ty.ty.span(),
                "flyte fn parameters must be by-value (owned) types",
            ));
        }
        params.push((pat_ident.ident.clone(), pat_ty.ty.as_ref().clone()));
    }
    Ok(params)
}

fn parse_version_attr(attr: TokenStream) -> syn::Result<Option<String>> {
    if attr.is_empty() {
        return Ok(None);
    }
    let meta: syn::MetaNameValue = syn::parse(attr)?;
    if !meta.path.is_ident("version") {
        return Err(syn::Error::new(
            meta.path.span(),
            "the only supported attribute is `version = \"...\"`",
        ));
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &meta.value
    else {
        return Err(syn::Error::new(
            meta.value.span(),
            "version must be a string literal",
        ));
    };
    Ok(Some(s.value()))
}

pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = syn::parse_macro_input!(item as syn::ItemFn);
    match expand_inner(attr, func) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_inner(attr: TokenStream, func: syn::ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let version = parse_version_attr(attr)?;
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            func.sig.fn_token.span(),
            "#[flyte::trace] requires an async fn",
        ));
    }
    if !func.sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            func.sig.generics.span(),
            "#[flyte::trace] does not support generic fns",
        ));
    }

    let (ok_ty, is_unit) = result_ok_type(&func.sig)?;
    let params = typed_params(&func.sig)?;

    let vis = &func.vis;
    let attrs = &func.attrs;
    let sig = &func.sig;
    let body = &func.block;
    let fn_name = &sig.ident;
    let fn_name_str = fn_name.to_string();
    let inner_ident = syn::Ident::new(&format!("__flyte_impl_{fn_name}"), fn_name.span());
    let mut inner_sig = sig.clone();
    inner_sig.ident = inner_ident.clone();

    // Identity mirrors Python's fn-name + AST-hash: editing the body changes the
    // identity, so stale recorded outputs are not replayed. `version = "..."`
    // pins it explicitly instead.
    let identity = match version {
        Some(v) => format!("{fn_name_str}-{v}"),
        None => {
            let digest = Md5::digest(body.to_token_stream().to_string().as_bytes());
            format!("{fn_name_str}-{:x}", digest)
        }
    };

    let arg_idents: Vec<_> = params.iter().map(|(ident, _)| ident).collect();
    let arg_names: Vec<_> = params.iter().map(|(ident, _)| ident.to_string()).collect();
    let arg_types: Vec<_> = params.iter().map(|(_, ty)| ty).collect();

    let iface_outputs = if is_unit {
        quote! { &[] }
    } else {
        quote! { &[("o0", <#ok_ty as ::flyte::FlyteType>::literal_type())] }
    };
    let has_outputs = !is_unit;

    let replay_arm = if is_unit {
        quote! { ::core::result::Result::Ok(()) }
    } else {
        quote! {
            {
                let __lit = ::flyte::types::output_literal(&__outs, "o0")?;
                ::core::result::Result::Ok(<#ok_ty as ::flyte::FlyteType>::from_literal(__lit)?)
            }
        }
    };

    let record_outputs = if is_unit {
        quote! { ::core::option::Option::None }
    } else {
        quote! {
            ::core::option::Option::Some(::flyte::types::build_outputs(
                ::std::vec![("o0", ::flyte::FlyteType::to_literal(&__value)?)],
            ))
        }
    };

    Ok(quote! {
        #(#attrs)*
        #vis #sig {
            #inner_sig #body

            let __state = match ::flyte::context::current() {
                ::core::option::Option::Some(st) if !::flyte::context::in_trace() => st,
                _ => return #inner_ident(#(#arg_idents),*).await,
            };

            const __IDENTITY: &str = #identity;
            let __inputs = ::flyte::types::build_inputs(::std::vec![
                #((#arg_names, ::flyte::FlyteType::to_literal(&#arg_idents)?)),*
            ]);
            let __iface = ::flyte::types::build_typed_interface(
                &[#((#arg_names, <#arg_types as ::flyte::FlyteType>::literal_type())),*],
                #iface_outputs,
            );

            match ::flyte::trace::prepare_trace(
                &__state, __IDENTITY, #fn_name_str, __inputs, __iface, #has_outputs,
            )
            .await?
            {
                ::flyte::trace::TracePrep::Replay(__outs) => #replay_arm,
                ::flyte::trace::TracePrep::Run(__handle) => {
                    match ::flyte::context::IN_TRACE
                        .scope(true, #inner_ident(#(#arg_idents),*))
                        .await
                    {
                        ::core::result::Result::Ok(__value) => {
                            __handle
                                .record(&__state, #record_outputs, ::flyte::trace::now_f64())
                                .await;
                            ::core::result::Result::Ok(__value)
                        }
                        ::core::result::Result::Err(__e) => ::core::result::Result::Err(__e),
                    }
                }
            }
        }
    })
}
