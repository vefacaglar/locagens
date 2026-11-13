//! Embeds the Locagens logo as the Windows executable icon (Explorer, taskbar
//! pins). On every other platform this build script does nothing.

fn main() {
    println!("cargo:rerun-if-changed=assets/locagens.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/locagens.ico");
        // A missing resource compiler must not break `cargo build`; the
        // window icon is also set at runtime from the rasterized logo.
        if let Err(err) = resource.compile() {
            println!("cargo:warning=could not embed the exe icon: {err}");
        }
    }
}
