/// The built-in modem bundle (M5 G4): a release build is given the bundle's
/// `MANIFEST.sha256` path in `MC_BUNDLE_MANIFEST`. Its text, and the
/// `BUILD.txt` beside it (the versions shown in the window), are copied into
/// OUT_DIR for `include_str!`. Without it both are empty: no built-in modem.
fn bundle_files() {
    println!("cargo:rerun-if-env-changed=MC_BUNDLE_MANIFEST");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let (manifest, build) = match std::env::var("MC_BUNDLE_MANIFEST") {
        Ok(p) if !p.is_empty() => {
            let p = std::path::PathBuf::from(p);
            println!("cargo:rerun-if-changed={}", p.display());
            let build = p.with_file_name("BUILD.txt");
            println!("cargo:rerun-if-changed={}", build.display());
            (
                std::fs::read_to_string(&p).expect("MC_BUNDLE_MANIFEST names a readable MANIFEST.sha256"),
                std::fs::read_to_string(&build).expect("BUILD.txt beside the manifest"),
            )
        }
        _ => (String::new(), String::new()),
    };
    std::fs::write(out.join("bundle-manifest.txt"), manifest).expect("write bundle manifest");
    std::fs::write(out.join("bundle-build.txt"), build).expect("write bundle build");
}

fn main() {
    bundle_files();
    // Rebuild the native resources when the app icons are regenerated.
    println!("cargo:rerun-if-changed=icons");

    // An explicit command allowlist (docs/02 §7): the window may call only the
    // commands the capability file grants, by name.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "app_info",
        "app_call",
        "events_since",
        "ui_ready",
        "report_ipc",
        "ping",
        "open_link",
        "set_waterfall_visible",
        "modem_probe",
        "splash_status",
        "quit_app",
        "appearance_load",
        "appearance_save",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)).expect("tauri-build failed");
}
