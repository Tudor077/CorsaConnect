//! Embeds the app icon (and version info) as a Windows resource so it shows up
//! in Explorer, Start, Search and the taskbar pin — not just the runtime window.

fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "CorsaConnect");
        res.set("ProductName", "CorsaConnect");
        if let Err(e) = res.compile() {
            // Don't fail the build on a machine without a resource compiler;
            // the app still runs, it just won't have the embedded icon.
            println!("cargo:warning=icon embed skipped: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
