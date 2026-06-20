//! Integration test for `register_eval!` + `registered_evals` and an in-process
//! end-to-end run through the public API.

use mira::scorer::{contains, succeeded};
use mira::subject::subject_fn;
use mira::{Eval, Runner, Transcript, register_eval, registered_evals};

fn alpha() -> Eval {
    Eval::new("alpha")
        .case("a", "alpha prompt")
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
        .case("b", "beta prompt")
        .subject(subject_fn(|_, _| async { Transcript::response("beta ok") }))
        .scorer(contains("ok"))
        .build()
}
register_eval!(beta);

#[test]
fn registration_collects_both() {
    let names: Vec<String> = registered_evals().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"alpha".to_string()));
    assert!(names.contains(&"beta".to_string()));
}

#[tokio::test]
async fn registered_evals_run_green_in_process() {
    let report = Runner::new().extend(registered_evals()).run().await;
    assert_eq!(report.total(), 2);
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
