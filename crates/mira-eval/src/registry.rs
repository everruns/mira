//! Automatic eval registration.
//!
//! Instead of hand-building a `Vec<Eval>`, annotate factory functions and let
//! the study collect them. This is Mira's `cargo test`-style discovery: write
//! the eval, register it, and `Study::registered()` exposes it.
//!
//! ```no_run
//! use mira::{register_eval, Eval, Transcript};
//! use mira::subject::subject_fn;
//! use mira::scorer::contains;
//!
//! fn greet() -> Eval {
//!     Eval::new("greet")
//!         .sample("hi", "say hi")
//!         .subject(subject_fn(|_, _| async { Transcript::response("hi there") }))
//!         .scorer(contains("hi"))
//!         .build()
//! }
//! register_eval!(greet);
//!
//! #[tokio::main]
//! async fn main() -> std::io::Result<()> {
//!     mira::Study::registered().serve().await
//! }
//! ```
//!
//! Registration is collected at link time via [`inventory`], so evals can be
//! spread across modules and files with no central list to maintain.

use crate::eval::Eval;

/// A registered eval factory. Built by [`register_eval!`](crate::register_eval);
/// iterated by [`registered_evals`].
pub struct EvalFactory(pub fn() -> Eval);

inventory::collect!(EvalFactory);

/// Register a zero-argument `fn() -> Eval` so it is picked up by
/// [`registered_evals`] / [`Study::registered`](crate::Study::registered).
///
/// ```
/// # use mira::{register_eval, Eval, Transcript};
/// # use mira::subject::subject_fn;
/// fn my_eval() -> Eval {
///     Eval::new("e")
///         .sample("a", "x")
///         .subject(subject_fn(|_, _| async { Transcript::default() }))
///         .build()
/// }
/// register_eval!(my_eval);
/// ```
#[macro_export]
macro_rules! register_eval {
    ($factory:path) => {
        $crate::inventory::submit! {
            $crate::registry::EvalFactory($factory)
        }
    };
}

/// Build every registered eval, in registration order within each compilation
/// unit (order across units is unspecified, like inventory generally).
pub fn registered_evals() -> Vec<Eval> {
    inventory::iter::<EvalFactory>
        .into_iter()
        .map(|f| (f.0)())
        .collect()
}

// `register_eval!` is re-exported at the crate root by `#[macro_export]`; the
// doctest above exercises it. A unit test here would register into the same
// global inventory and perturb other tests, so registration is covered by the
// `registration` integration test instead.
