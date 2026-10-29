//! Embeds the application icon and display name in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        // The name Windows shows for the running app (taskbar, Task Manager).
        res.set("FileDescription", "Fracst");
        res.set("ProductName", "Fracst");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the executable icon: {e}");
        }
    }
}
