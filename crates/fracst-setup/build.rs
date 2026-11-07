//! Embeds the icon and a manifest, and points the build at the Inno Setup
//! engine to carry.
//!
//! FRACST_SETUP_ENGINE names the engine (packaging/windows/fracst.iss,
//! compiled). Without it the build carries nothing, and the installer needs
//! `--engine <path>` at run time; that is for development only.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=FRACST_SETUP_ENGINE");
    println!("cargo:rerun-if-env-changed=FRACST_SETUP_APP_ID");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    let engine = match std::env::var_os("FRACST_SETUP_ENGINE") {
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            path
        }
        None => {
            let empty = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("no-engine");
            std::fs::write(&empty, []).unwrap();
            empty
        }
    };
    println!("cargo:rustc-env=FRACST_SETUP_ENGINE_FILE={}", engine.display());

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("FileDescription", "Fracst Setup");
        res.set("ProductName", "Fracst");
        // asInvoker: without a manifest, Windows elevates any exe named
        // "*setup*". The engine asks for elevation itself when it needs it.
        res.set_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
      processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
</assembly>
"#,
        );
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon and manifest: {e}");
        }
    }
}
