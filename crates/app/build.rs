//! Embeds the icon and version information into `perch.exe` (Windows targets
//! only). Uses `windres` (MinGW) or `rc.exe` (MSVC) through `winresource`.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/perch.ico");
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let version = env!("CARGO_PKG_VERSION");
            let mut res = winresource::WindowsResource::new();
            res.set_icon("../../assets/perch.ico")
                .set("ProductName", "Perch")
                .set("FileDescription", "Perch - a lightweight dynamic notch")
                .set("CompanyName", "Perch contributors")
                .set("LegalCopyright", "MIT OR Apache-2.0")
                .set("OriginalFilename", "perch.exe")
                .set("ProductVersion", version)
                .set("FileVersion", version);
            if let Err(e) = res.compile() {
                panic!("embedding the Windows icon/version resource failed: {e}");
            }
        }
    }
}
