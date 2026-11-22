//! A resumed run must retain its original catalog and input identities. Content
//! hashes cover declared files; opaque environment variables are never scanned
//! or copied, so credentials do not accidentally become provenance artifacts.

use mira::{
    protocol::{InitializeResult, ListResult},
    provenance::fingerprint,
};
use std::path::Path;

pub fn catalog(
    dir: &Path,
    info: &InitializeResult,
    listing: &ListResult,
    selection: &[String],
    resume: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let git = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let diff = std::process::Command::new("git")
        .args(["diff", "--binary", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| fingerprint(&o.stdout));
    let value = serde_json::json!({"git_commit":git,"tracked_diff_sha256":diff,"study":info.study,"study_version":info.study_version,"catalog":listing,"selection":selection});
    let bytes = serde_json::to_vec(&value)?;
    let record = serde_json::json!({"sha256":fingerprint(&bytes),"definition":value});
    let path = dir.join("provenance.json");
    if resume {
        let previous: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path)
                .map_err(|e| format!("cannot verify resumed run provenance: {e}"))?,
        )?;
        if previous != record {
            return Err("study, dataset, scorers, or selection changed; start a new run instead of resuming".into());
        }
    } else {
        std::fs::create_dir_all(dir)?;
        std::fs::write(path, serde_json::to_vec_pretty(&record)?)?;
    }
    Ok(())
}

pub fn files(paths: &[String]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut hashes = std::collections::BTreeMap::new();
    for path in paths {
        hashes.insert(
            path,
            fingerprint(
                &std::fs::read(path).map_err(|e| format!("cannot fingerprint {path}: {e}"))?,
            ),
        );
    }
    Ok(serde_json::to_value(hashes)?)
}
