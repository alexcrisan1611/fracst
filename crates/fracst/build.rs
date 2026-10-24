//! Embeds the application icon in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the executable icon: {e}");
        }
    }
}
