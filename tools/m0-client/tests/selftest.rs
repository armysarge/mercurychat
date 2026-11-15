//! Runs the full M0 scenario against the in-process fake Mercury pair, so the
//! same L0 gate runs in CI on Windows x64, Linux amd64 and Linux arm64.
#[test]
fn m0_scenario_passes_against_fake_pair() {
    let work = std::env::temp_dir().join(format!("m0-selftest-{}", std::process::id()));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_m0-client"))
        .args(["selftest", "--work"])
        .arg(&work)
        .output()
        .expect("run m0-client");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "selftest failed:\n{stderr}");
    let report = std::fs::read_to_string(work.join("report.json")).unwrap();
    assert!(report.contains("\"failed\": 0"), "{report}");
    let _ = std::fs::remove_dir_all(&work);
}
