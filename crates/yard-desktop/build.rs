fn main() {
    #[cfg(target_os = "macos")]
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new()
                .commands(&["launch_machine_add", "launch_machine_reconnect"]),
        ),
    )
    .expect("failed to build Yard desktop application");
}
