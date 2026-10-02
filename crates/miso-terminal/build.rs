//! Embeds the icon and version information into the Windows executable, so
//! Explorer, the taskbar and the file's Properties dialog show them.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/miso-terminal.ico");
    // Build scripts run on the host; only embed when the host can (Windows) and
    // the target is Windows.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/miso-terminal.ico")
            .set("ProductName", "MISO Terminal")
            .set("FileDescription", "MISO Terminal")
            .set(
                "LegalCopyright",
                "MISO data is public information published by MISO.",
            );
        // A missing resource compiler (rc.exe) should cost the icon, not the build.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
