//! Embeds the p-track icon and version details into `p-track.exe`, so the
//! Start menu entry, Explorer, and the portable folder show the app icon.

fn main() {
    println!("cargo:rerun-if-changed=../../src-tauri/icons/icon.ico");
    println!("cargo:rerun-if-env-changed=PTRACK_BUILD_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = tauri_winres::WindowsResource::new();
    resource
        .set_icon("../../src-tauri/icons/icon.ico")
        .set("ProductName", "p-track")
        .set("FileDescription", "p-track")
        .set("InternalName", "p-track")
        .set("OriginalFilename", "p-track.exe");
    // Release builds stamp the product version, as they do for ptrack.exe.
    if let Ok(version) = std::env::var("PTRACK_BUILD_VERSION")
        && !version.is_empty()
    {
        resource
            .set("ProductVersion", &version)
            .set("FileVersion", &version);
    }
    if let Err(error) = resource.compile() {
        panic!("embed the p-track launcher resources: {error}");
    }
}
