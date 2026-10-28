Prebuilt 64-bit packages of **fracst**, a fast deep-zoom Mandelbrot explorer. Each package contains the explorer (`fracst`), the command-line renderer (`fracst-cli`), the README and the licenses. The Windows and Linux packages also contain `fracst-server`, which renders for the app on another machine, on an NVIDIA GPU with CUDA when it has one.

| Platform | Download | Notes |
|---|---|---|
| Windows 10/11 (x86_64) | `fracst-*-windows-x86_64-setup.exe` | Run the installer: it installs for you without administrator rights, adds fracst to the Start menu, and can put `fracst-cli` on PATH and open `.kfr` files with fracst. Uninstall from **Settings → Apps**. Or take the portable `fracst-*-windows-x86_64.zip`. The builds are unsigned, so SmartScreen may ask you to confirm: choose **More info → Run anyway**. |
| macOS 11+ (Apple Silicon and Intel) | `fracst-*-macos-universal.dmg` | Open the disk image and drag fracst to Applications. The app is not notarised: on first launch, right-click it and choose **Open**, or run `xattr -dr com.apple.quarantine /Applications/fracst.app`. `fracst-cli` is in `fracst-*-macos-universal.zip`. |
| Debian, Ubuntu and relatives (x86_64, Ubuntu 22.04+ / Debian 12+) | `fracst-*-linux-amd64.deb` | `sudo apt install ./fracst-*-linux-amd64.deb`. Adds fracst to the applications menu and `fracst`, `fracst-cli` and `fracst-server` to PATH. |
| Other Linux (x86_64, glibc 2.35+) | `fracst-*-linux-x86_64.tar.gz` | Extract and run `./install.sh` to install into `~/.local` with a menu entry (`./install.sh --uninstall` removes it), or just run `./fracst`. |

On Linux, fracst needs Vulkan or OpenGL drivers (Mesa is fine) and X11 or Wayland; file dialogs use the XDG desktop portal.

Open a location by passing a `.kfr` file or a PNG with an embedded location: `fracst location.kfr`. Run `fracst-cli --help` for batch rendering.

To render on a cloud GPU, open **Remote GPU** in the app and connect to a fracst gateway or server with your key; see [docs/remote.md](https://github.com/alexcrisan1611/fracst/blob/main/docs/remote.md).
