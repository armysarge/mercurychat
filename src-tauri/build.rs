fn main() {
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
        "modem_probe",
        "splash_status",
        "quit_app",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)).expect("tauri-build failed");
}
