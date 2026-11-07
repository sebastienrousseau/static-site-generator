// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Proc-macro implementation of `#[ssg_rpc]` for the Edge RPC layer.
//!
//! The attribute does three things to its target function:
//!
//! 1. Re-emits the original function untouched, so direct callers in
//!    Rust keep working.
//! 2. Generates a sibling `__SSG_RPC_<fn_name>` static of type
//!    `ssg_rpc::RpcDescriptor` which carries the function name plus
//!    a type-erased dispatcher trampoline. (Plain code reference
//!    rather than an intra-doc link — proc-macro crates can't
//!    resolve symbols in their sibling runtime crate at doc time.)
//! 3. Wires the static into the inventory-based dispatch registry so
//!    `ssg_rpc::dispatch(name, json)` resolves it at runtime.
//!
//! The macro is intentionally narrow: it only accepts free functions
//! of the shape `fn(Input) -> Result<Output, RpcError>` where `Input`
//! and `Output` are both `serde::Serialize + serde::Deserialize +
//! schemars::JsonSchema`. Anything else is rejected at compile time
//! with a pointed error message.
//!
//! # Examples
//!
//! ```ignore
//! use ssg_rpc::{ssg_rpc, RpcError};
//! use serde::{Deserialize, Serialize};
//! use schemars::JsonSchema;
//!
//! #[derive(Serialize, Deserialize, JsonSchema)]
//! pub struct EchoIn { pub msg: String }
//!
//! #[derive(Serialize, Deserialize, JsonSchema)]
//! pub struct EchoOut { pub msg: String }
//!
//! #[ssg_rpc]
//! pub fn echo(input: EchoIn) -> Result<EchoOut, RpcError> {
//!     Ok(EchoOut { msg: input.msg })
//! }
//!
//! // The dispatcher registry now resolves "echo" to this function.
//! let body = ssg_rpc::dispatch("echo", r#"{"msg":"hi"}"#).unwrap();
//! assert!(body.contains("\"msg\":\"hi\""));
//! ```

#![allow(clippy::module_name_repetitions)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ItemFn, ReturnType, Type};

/// Attribute that registers a function in the Edge RPC dispatch
/// table and emits its schema for the TypeScript client.
///
/// # Example
///
/// ```ignore
/// use ssg_rpc::{ssg_rpc, RpcError};
/// use serde::{Serialize, Deserialize};
/// use schemars::JsonSchema;
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// pub struct LikeInput { pub post_id: String }
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// pub struct LikeOutput { pub likes: u64 }
///
/// #[ssg_rpc]
/// pub fn like_post(input: LikeInput) -> Result<LikeOutput, RpcError> {
///     Ok(LikeOutput { likes: 1 })
/// }
/// ```
#[proc_macro_attribute]
pub fn ssg_rpc(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_name_str = fn_name.to_string();
    let vis = &input_fn.vis;

    // Validate signature: exactly one argument, returns Result<_, RpcError>.
    if input_fn.sig.inputs.len() != 1 {
        return syn::Error::new(
            Span::call_site(),
            "#[ssg_rpc] functions must take exactly one input argument",
        )
        .to_compile_error()
        .into();
    }

    let Some(FnArg::Typed(arg)) = input_fn.sig.inputs.first() else {
        return syn::Error::new(
            Span::call_site(),
            "#[ssg_rpc] functions cannot take a `self` receiver",
        )
        .to_compile_error()
        .into();
    };

    let input_ty: &Type = &arg.ty;

    let output_ty: Type = match &input_fn.sig.output {
        ReturnType::Type(_, ty) => (**ty).clone(),
        ReturnType::Default => {
            return syn::Error::new(
                Span::call_site(),
                "#[ssg_rpc] functions must return Result<T, RpcError>",
            )
            .to_compile_error()
            .into();
        }
    };

    // Names of generated artefacts.
    let descriptor_ident =
        format_ident!("__SSG_RPC_DESCRIPTOR_{}", fn_name_str.to_uppercase());
    let trampoline_ident = format_ident!("__ssg_rpc_trampoline_{}", fn_name);
    let schema_ident = format_ident!("__ssg_rpc_schema_{}", fn_name);
    let ctor_ident = format_ident!("__ssg_rpc_register_{}", fn_name);

    let expanded = quote! {
        // 1. Original function — emitted untouched.
        #input_fn

        // 2. Type-erased trampoline: JSON in → JSON out, never panics on
        // malformed JSON (returns RpcError::BadRequest).
        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #trampoline_ident(
            payload: &str,
        ) -> ::core::result::Result<::std::string::String, ::ssg_rpc::RpcError>
        {
            let input: #input_ty = ::serde_json::from_str(payload)
                .map_err(|e| ::ssg_rpc::RpcError::BadRequest(
                    ::std::format!("invalid JSON payload: {e}"),
                ))?;
            let result: #output_ty = #fn_name(input);
            let output = result?;
            ::serde_json::to_string(&output).map_err(|e| {
                ::ssg_rpc::RpcError::Internal(::std::format!(
                    "response serialisation failed: {e}"
                ))
            })
        }

        // 3. Schema producer — emitted as a thunk so the registry can
        // pull both input + output schemas on demand without pulling
        // schemars into every call site.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #schema_ident() -> ::ssg_rpc::RpcSchema {
            ::ssg_rpc::RpcSchema {
                name: #fn_name_str,
                input: ::ssg_rpc::schema_for::<#input_ty>(),
                output: ::ssg_rpc::schema_for_result::<#output_ty>(),
            }
        }

        // 4. Descriptor (the registry entry).
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        #vis static #descriptor_ident: ::ssg_rpc::RpcDescriptor =
            ::ssg_rpc::RpcDescriptor {
                name: #fn_name_str,
                dispatch: #trampoline_ident,
                schema: #schema_ident,
            };

        // 5. Register on first use via the inventory crate.
        ::ssg_rpc::inventory::submit! {
            ::ssg_rpc::RpcDescriptorRef(&#descriptor_ident)
        }

        // Keep ctor name reachable to silence dead_code under
        // pathological feature combos.
        #[doc(hidden)]
        #[allow(dead_code, non_snake_case)]
        fn #ctor_ident() {
            let _ = &#descriptor_ident;
        }
    };

    expanded.into()
}
