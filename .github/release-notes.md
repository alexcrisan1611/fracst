Prebuilt 64-bit packages of **fracst**, a fast deep-zoom Mandelbrot explorer and an alternative to Fraktaler 3. Each package contains the explorer (`fracst`), the command-line renderer (`fracst-cli`), the README and the licenses.

| Platform | Download | Notes |
|---|---|---|
| Windows 10/11 (x86_64) | `fracst-*-windows-x86_64.zip` | Unzip and run `fracst.exe`. The build is unsigned, so SmartScreen may ask you to confirm: choose **More info → Run anyway**. |
| macOS 11+ (Apple Silicon and Intel) | `fracst-*-macos-universal.zip` | Unzip and move `fracst.app` to Applications. The app is not notarised: on first launch, right-click it and choose **Open**, or run `xattr -dr com.apple.quarantine fracst.app`. |
| Linux (x86_64, glibc 2.35+, e.g. Ubuntu 22.04+) | `fracst-*-linux-x86_64.tar.gz` | Extract and run `./fracst`. Needs Vulkan or OpenGL drivers (Mesa is fine) and X11 or Wayland. File dialogs use the XDG desktop portal. |

Open a location by passing a `.kfr` file or a PNG with an embedded location: `fracst location.kfr`. Run `fracst-cli --help` for batch rendering.
