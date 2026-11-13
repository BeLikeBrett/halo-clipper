//! Embeds the application icon and version metadata into the Windows
//! executable, so Explorer, Start-menu shortcuts, the taskbar and Task
//! Manager all show "Halo Clipper" with the real icon. No-op elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "Halo Clipper")
        // Task Manager and the "Open with" list show FileDescription.
        .set("FileDescription", "Halo Clipper")
        .set("OriginalFilename", "halo-clipper.exe")
        .set("InternalName", "halo-clipper");
    if let Err(err) = res.compile() {
        eprintln!("failed to embed Windows resources: {err}");
        std::process::exit(1);
    }
}
