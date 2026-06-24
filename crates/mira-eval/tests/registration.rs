//! Integration test for eval registration (`register_eval!` and, when the
//! `macros` feature is on, the `#[eval]` attribute) plus an in-process
//! end-to-end run through the public API.

use mira::scorer::{contains, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Runner, Transcript, register_eval, registered_evals};

fn alpha() -> Eval {
    Eval::new("alpha")
        .sample("a", "alpha prompt")
        .subject(subject_fn(|_, _| async {
            Transcript::response("alpha ok")
        }))
        .scorer(succeeded())
        .scorer(contains("ok"))
        .build()
}
register_eval!(alpha);

fn beta() -> Eval {
    Eval::new("beta")
        .sample("b", "beta prompt")
        .subject(subject_fn(|_, _| async { Transcript::response("beta ok") }))
        .scorer(contains("ok"))
        .build()
}
register_eval!(beta);

// `#[eval]` is the ergonomic form of `register_eval!`; it registers the same
// way. Only compiled when the default `macros` feature is enabled.
#[cfg(feature = "macros")]
#[mira::eval]
fn gamma() -> Eval {
    Eval::new("gamma")
        .sample("g", "gamma prompt")
        .subject(subject_fn(|_, _| async {
            Transcript::response("gamma ok")
        }))
        .scorer(contains("ok"))
        .build()
}

/// Evals registered regardless of features.
const BASE_EVALS: usize = 2;
/// Plus `gamma` when the `#[eval]` attribute is available.
#[cfg(feature = "macros")]
const TOTAL_EVALS: usize = BASE_EVALS + 1;
#[cfg(not(feature = "macros"))]
const TOTAL_EVALS: usize = BASE_EVALS;

#[test]
fn registration_collects_macro_and_bang() {
    let names: Vec<String> = registered_evals().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"alpha".to_string()));
    assert!(names.contains(&"beta".to_string()));
    #[cfg(feature = "macros")]
    assert!(names.contains(&"gamma".to_string()));
}

#[tokio::test]
async fn registered_evals_run_green_in_process() {
    let report = Runner::new().extend(registered_evals()).run().await;
    assert_eq!(report.total(), TOTAL_EVALS);
    assert!(report.all_passed(), "skipped: {:?}", report.skipped);
}

#[tokio::test]
async fn filter_selects_one_registered_eval() {
    let report = Runner::new()
        .extend(registered_evals())
        .filter(Some("alpha".into()))
        .run()
        .await;
    assert_eq!(report.total(), 1);
    assert_eq!(report.outcomes[0].eval, "alpha");
}
