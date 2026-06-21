//! A SWE-bench-style eval: fix a bug in a seeded repository so its failing test
//! passes. This is the canonical "harness as a Mira eval" shape.
//!
//! ```bash
//! mira --bin swe_bench list
//! mira --bin swe_bench run
//! mira --bin swe_bench run --group-by difficulty   # resolve rate per difficulty
//! ```
//!
//! Each [`Sample`] seeds a buggy source file (and records the `FAIL_TO_PASS`
//! test ids in metadata, exactly like a SWE-bench task instance). The subject
//! produces a patched workspace; the custom `fail_to_pass` scorer plays the role
//! of the test harness's `FAIL_TO_PASS` gate.
//!
//! To stay offline and free in CI, the subject here is a deterministic
//! in-process "fixer" and the gate is a pure check on the patched file. In a
//! real run you swap two things and nothing else changes:
//!
//! * **Subject** → a [`CliSubject`](mira::CliSubject) that shells out to your agent / Docker
//!   harness (the polyglot path), seeding `sample.files` into the container and
//!   capturing the patched tree back.
//! * **Scorer** → a `fail_to_pass` that actually runs the instance's tests
//!   (e.g. `pytest <ids>`) inside the container and checks they pass.
//!
//! ```ignore
//! .subject(
//!     CliSubject::new("python")
//!         .arg("run_swebench_instance.py")
//!         .arg("{workdir}")
//!         .capture_files(),
//! )
//! ```

use mira::scorer::{succeeded, tool_called};
use mira::subject::subject_fn;
use mira::{Dataset, Eval, Sample, Score, Scorer, Transcript, eval};

/// The buggy + fixed forms of each instance's target file. Kept tiny so the
/// example is self-checking without a language runtime.
struct Instance {
    id: &'static str,
    path: &'static str,
    buggy: &'static str,
    fixed: &'static str,
    fail_to_pass: &'static str,
    /// SWE-bench-style provenance, carried as sample metadata so the host can
    /// break resolve-rate down by it: `mira --bin swe_bench run --group-by difficulty`.
    difficulty: &'static str,
}

const INSTANCES: &[Instance] = &[
    Instance {
        id: "calc__off-by-one-sum",
        path: "calc.py",
        buggy: "def sum_to(n):\n    # BUG: drops the last term\n    return sum(range(n))\n",
        fixed: "def sum_to(n):\n    return sum(range(n + 1))\n",
        fail_to_pass: "tests/test_calc.py::test_sum_to_10",
        difficulty: "easy",
    },
    Instance {
        id: "strutil__wrong-casing",
        path: "strutil.py",
        buggy: "def shout(s):\n    # BUG: lowercases instead of upper\n    return s.lower()\n",
        fixed: "def shout(s):\n    return s.upper()\n",
        fail_to_pass: "tests/test_strutil.py::test_shout",
        difficulty: "medium",
    },
];

fn dataset() -> Dataset {
    Dataset::new(
        INSTANCES
            .iter()
            .map(|inst| {
                Sample::new(
                    inst.id,
                    format!("Fix the bug in {} so its tests pass.", inst.path),
                )
                .file(inst.path, inst.buggy)
                .tag("swe-bench")
                .meta("FAIL_TO_PASS", inst.fail_to_pass)
                .meta("repo", "example/repo")
                .meta("difficulty", inst.difficulty)
            })
            .collect(),
    )
}

/// Stands in for the SWE-bench `FAIL_TO_PASS` gate: the patched file must match
/// the known-good fix (a real harness would run the listed tests instead).
fn fail_to_pass() -> Box<dyn Scorer> {
    mira::scorer::scorer("fail_to_pass", |sample: &Sample, t: &Transcript| {
        let Some(inst) = INSTANCES.iter().find(|i| i.id == sample.id) else {
            return Score::fail("fail_to_pass", "unknown instance");
        };
        match t.files.get(inst.path) {
            Some(patched) if patched.contains(inst.fixed.trim()) => {
                Score::pass("fail_to_pass", format!("{} now passes", inst.fail_to_pass))
            }
            Some(_) => Score::fail("fail_to_pass", format!("{} still fails", inst.fail_to_pass)),
            None => Score::fail("fail_to_pass", format!("no {} in patch", inst.path)),
        }
    })
}

#[eval]
fn swe_bench() -> Eval {
    Eval::new("swe_bench")
        .describe("Fix a seeded bug so the instance's FAIL_TO_PASS test passes")
        .dataset(dataset())
        .subject(subject_fn(|sample, _cx| async move {
            // Deterministic "agent": apply the known fix for this instance.
            let inst = INSTANCES.iter().find(|i| i.id == sample.id);
            let mut t = Transcript::response("Applied patch.");
            t.tool_calls = vec!["read_file".into(), "apply_patch".into()];
            t.tool_calls_count = 2;
            t.iterations = 2;
            if let Some(inst) = inst {
                t.files.insert(inst.path.into(), inst.fixed.into());
            }
            t
        }))
        .scorer(succeeded())
        .scorer(tool_called("apply_patch"))
        .scorer(fail_to_pass())
        .build()
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // A real SWE-bench dataset is thousands of instances — far too many to
    // enumerate in one `list` line. We force a tiny page size here so even this
    // two-instance demo paginates: `list` returns the first page plus a cursor,
    // and the host pages the rest via `list_samples`. Drop this call to send the
    // whole dataset inline (the default page size handles realistic studies).
    mira::Study::registered().page_size(1).serve().await
}
