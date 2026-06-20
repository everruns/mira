//! Procedural macros for [Mira](https://docs.rs/mira-eval).
//!
//! This crate is an implementation detail of `mira-eval`; use it through the
//! re-export `mira::eval`, not directly.

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// Register an eval factory for `cargo test`-style discovery.
///
/// `#[eval]` annotates a `fn() -> Eval` (or `-> EvalBuilder`) factory. It leaves
/// the function untouched and additionally submits it to the global registry, so
/// [`serve_registered`] / [`registered_evals`] pick it up with no central list.
/// It is the ergonomic form of [`register_eval!`]; these are equivalent:
///
/// ```ignore
/// #[eval]
/// fn greet() -> Eval { /* … */ }
///
/// // …is the same as:
/// fn greet() -> Eval { /* … */ }
/// register_eval!(greet);
/// ```
///
/// [`serve_registered`]: ../mira/fn.serve_registered.html
/// [`registered_evals`]: ../mira/fn.registered_evals.html
/// [`register_eval!`]: ../mira/macro.register_eval.html
#[proc_macro_attribute]
pub fn eval(args: TokenStream, item: TokenStream) -> TokenStream {
    if !args.is_empty() {
        let msg = "#[eval] takes no arguments; configure the eval inside the function body \
                   (e.g. `.models(...)`, `.axis(...)`)";
        let err = syn::Error::new(proc_macro2::Span::call_site(), msg);
        return err.to_compile_error().into();
    }

    let func = parse_macro_input!(item as ItemFn);
    let name = &func.sig.ident;

    quote! {
        #func
        ::mira::register_eval!(#name);
    }
    .into()
}
