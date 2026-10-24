//! On Windows, puts the icon and version information into the executable so Explorer,
//! the taskbar and the installer show RustZIP's icon. Elsewhere there is nothing to do.

fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icons/rustzip.ico");
        res.set("ProductName", "RustZIP");
        res.set("FileDescription", "RustZIP archive manager");
        // A missing resource compiler must not stop the build; the program just has no icon.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
