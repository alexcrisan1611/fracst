//! Finding, downloading and running ffmpeg.
//!
//! fracst does not bundle an encoder. It uses, in this order, an ffmpeg
//! named by `FRACST_FFMPEG`, the one chosen with [`remember`], the one it
//! downloaded earlier into its data folder, one on the `PATH`, or one where
//! package managers put it. The last matters because their `PATH` entries
//! don't always reach apps: winget can fail to create its command links,
//! and apps opened from the macOS Finder don't see Homebrew's folder.
//!
//! If there is none, fracst can download a static build, pinned by version
//! and SHA-256 below, into its data folder (no administrator rights, nothing
//! else on the system changes).

use crate::project::Codec;
use sha2::{Digest, Sha256};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

/// A static ffmpeg build for this platform.
pub struct Download {
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// Size of the zip archive in bytes.
    pub size: u64,
    /// The executable's path inside the archive.
    pub entry: &'static str,
    /// Who builds it, for the consent prompt.
    pub source: &'static str,
}

const DOWNLOADS: &[(&str, &str, Download)] = &[
    (
        "windows",
        "x86_64",
        Download {
            version: "9.0.2",
            url: "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip",
            sha256: "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba",
            size: 114_768_076,
            entry: "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe",
            source: "gyan.dev",
        },
    ),
    (
        "macos",
        "aarch64",
        Download {
            version: "9.0.2",
            url: "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip",
            sha256: "c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924",
            size: 28_395_699,
            entry: "ffmpeg",
            source: "ffmpeg.martin-riedl.de",
        },
    ),
    (
        "macos",
        "x86_64",
        Download {
            version: "9.0.2",
            url: "https://ffmpeg.martin-riedl.de/download/macos/amd64/1789931006_9.0.2/ffmpeg.zip",
            sha256: "7c6b4125b191cbf773832dc51f424cf2b6bb7da43007d1e066f95909e47cacd4",
            size: 33_816_391,
            entry: "ffmpeg",
            source: "ffmpeg.martin-riedl.de",
        },
    ),
    (
        "linux",
        "x86_64",
        Download {
            version: "9.0.2",
            url: "https://ffmpeg.martin-riedl.de/download/linux/amd64/1789931100_9.0.2/ffmpeg.zip",
            sha256: "fa8ecf4abbd290d98f7d188b8649cc6b391ae209a98452be955a15aab1909d7f",
            size: 33_326_445,
            entry: "ffmpeg",
            source: "ffmpeg.martin-riedl.de",
        },
    ),
    (
        "linux",
        "aarch64",
        Download {
            version: "9.0.2",
            url: "https://ffmpeg.martin-riedl.de/download/linux/arm64/1789931697_9.0.2/ffmpeg.zip",
            sha256: "93a76ae90db5474eecdf951a729857c64f3de23567228d6a7d5e6e8e3cd1021b",
            size: 28_753_826,
            entry: "ffmpeg",
            source: "ffmpeg.martin-riedl.de",
        },
    ),
];

/// The build to download on this platform, if there is one.
pub fn download_info() -> Option<&'static Download> {
    DOWNLOADS
        .iter()
        .find(|(os, arch, _)| *os == std::env::consts::OS && *arch == std::env::consts::ARCH)
        .map(|(_, _, d)| d)
}

const EXE: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };

/// fracst's folder for per-user data: `%APPDATA%\fracst` on Windows,
/// `~/Library/Application Support/fracst` on macOS, and
/// `$XDG_DATA_HOME/fracst` (`~/.local/share/fracst`) elsewhere.
pub fn data_dir() -> Option<PathBuf> {
    let var = |k| std::env::var_os(k).map(PathBuf::from);
    #[cfg(windows)]
    let base = var("APPDATA");
    #[cfg(target_os = "macos")]
    let base = var("HOME").map(|h| h.join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")));
    base.map(|b| b.join("fracst"))
}

/// Where a downloaded ffmpeg lives.
pub fn managed_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("ffmpeg").join(EXE))
}

fn command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        // No console window flashing up from the GUI app.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn works(exe: &Path) -> bool {
    command(exe)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The file holding the ffmpeg chosen with [`remember`].
fn remembered_file() -> Option<PathBuf> {
    data_dir().map(|d| d.join("ffmpeg-path.txt"))
}

/// Use this ffmpeg from now on, if it runs.
pub fn remember(exe: &Path) -> Result<(), String> {
    if !works(exe) {
        return Err(format!("{} does not run as ffmpeg", exe.display()));
    }
    let file = remembered_file().ok_or("cannot find a folder for user data")?;
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&file, exe.to_string_lossy().as_bytes()).map_err(|e| e.to_string())
}

/// Where package managers install ffmpeg.
fn installed() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
    #[cfg(windows)]
    {
        if let Some(local) = var("LOCALAPPDATA") {
            let winget = local.join("Microsoft").join("WinGet");
            found.push(winget.join("Links").join(EXE));
            // winget unpacks archives into Packages/<id>_<source>/<folder>/bin.
            let packages = std::fs::read_dir(winget.join("Packages")).into_iter().flatten().flatten();
            for package in packages.filter(|p| p.file_name().to_string_lossy().to_lowercase().contains("ffmpeg")) {
                for inner in std::fs::read_dir(package.path()).into_iter().flatten().flatten() {
                    found.push(inner.path().join("bin").join(EXE));
                }
                found.push(package.path().join("bin").join(EXE));
            }
        }
        if let Some(home) = var("USERPROFILE") {
            found.push(home.join("scoop").join("shims").join(EXE));
        }
        let data = var("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        found.push(data.join("chocolatey").join("bin").join(EXE));
        found.push(Path::new("C:/ffmpeg/bin").join(EXE));
    }
    #[cfg(unix)]
    {
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin", "/snap/bin"] {
            found.push(Path::new(dir).join(EXE));
        }
        if let Some(home) = var("HOME") {
            found.push(home.join(".local/bin").join(EXE));
        }
    }
    found
}

/// The ffmpeg to use, if any.
pub fn find() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FRACST_FFMPEG").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let remembered = remembered_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty());
    let on_path = PathBuf::from(EXE);
    let candidates = remembered.into_iter().chain(managed_path()).filter(|p| p.is_file());
    candidates.chain(std::iter::once(on_path)).chain(installed().into_iter().filter(|p| p.is_file())).find(|p| works(p))
}

/// Download, verify and unpack the pinned build. `progress` gets bytes
/// downloaded so far and the total; setting `cancel` stops the download.
pub fn download(progress: &mut dyn FnMut(u64, u64), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let info = download_info().ok_or_else(|| {
        format!(
            "there is no ffmpeg download for {} {}; install ffmpeg and put it on the PATH, or set FRACST_FFMPEG",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let target = managed_path().ok_or("cannot find a folder for user data")?;
    let dir = target.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let zip_path = dir.join("download.zip.part");
    let fail = |e: String| {
        let _ = std::fs::remove_file(&zip_path);
        e
    };

    let response = ureq::get(info.url).call().map_err(|e| format!("cannot download {}: {e}", info.url))?;
    let mut body = response.into_body();
    let mut reader = body.as_reader();
    let mut file = std::fs::File::create(&zip_path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            return Err(fail("download cancelled".into()));
        }
        let n = reader.read(&mut buf).map_err(|e| fail(format!("download failed: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| fail(e.to_string()))?;
        got += n as u64;
        progress(got, info.size);
    }
    drop(file);
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != info.sha256 {
        return Err(fail(format!(
            "the download's checksum is wrong ({digest}, expected {}); not using it",
            info.sha256
        )));
    }

    let unpack = || -> Result<(), String> {
        let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("bad archive: {e}"))?;
        let mut entry =
            archive.by_name(info.entry).map_err(|e| format!("{} is not in the archive: {e}", info.entry))?;
        let tmp = target.with_extension("part");
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        drop(out);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())
    };
    let unpacked = unpack();
    let _ = std::fs::remove_file(&zip_path);
    unpacked?;
    if !works(&target) {
        return Err(format!("the downloaded ffmpeg at {} does not run", target.display()));
    }
    Ok(target)
}

/// The names of the video encoders this ffmpeg has.
pub fn encoders(ffmpeg: &Path) -> Result<Vec<String>, String> {
    let out = command(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", ffmpeg.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let flags = parts.next()?;
            (flags.len() == 6 && (flags.starts_with('V') || flags.starts_with('A'))).then(|| parts.next())?
        })
        .map(String::from)
        .collect())
}

/// Length of a media file in seconds, read from ffmpeg's report on it.
pub fn duration(ffmpeg: &Path, file: &Path) -> Result<f64, String> {
    let out = command(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", ffmpeg.display()))?;
    let text = String::from_utf8_lossy(&out.stderr);
    parse_duration(&text).ok_or_else(|| {
        let last = text.lines().last().unwrap_or("").trim().to_string();
        format!("cannot read the length of {}: {last}", file.display())
    })
}

fn parse_duration(report: &str) -> Option<f64> {
    let rest = &report[report.find("Duration: ")? + 10..];
    let hms = rest.split(',').next()?.trim();
    let mut it = hms.split(':');
    let (h, m, s) = (it.next()?.parse::<f64>().ok()?, it.next()?.parse::<f64>().ok()?, it.next()?.parse::<f64>().ok()?);
    Some(h * 3600.0 + m * 60.0 + s)
}

/// The encoder and its options for `codec`. `crf` overrides the default quality.
fn codec_args(codec: Codec, crf: Option<u32>, available: &[String]) -> Result<Vec<String>, String> {
    let has = |e: &str| available.iter().any(|a| a == e);
    let need = |e: &str| {
        if has(e) { Ok(()) } else { Err(format!("this ffmpeg has no {e} encoder for {}", codec.name())) }
    };
    let crf = |default: u32| crf.unwrap_or(default).to_string();
    let args: Vec<String> = match codec {
        Codec::H264 => {
            need("libx264")?;
            ["-c:v", "libx264", "-preset", "slow", "-crf", &crf(18), "-pix_fmt", "yuv420p"].map(String::from).into()
        }
        Codec::H265 => {
            need("libx265")?;
            ["-c:v", "libx265", "-preset", "slow", "-crf", &crf(20), "-pix_fmt", "yuv420p", "-tag:v", "hvc1"]
                .map(String::from)
                .into()
        }
        Codec::Av1 if has("libsvtav1") => {
            ["-c:v", "libsvtav1", "-preset", "6", "-crf", &crf(28), "-pix_fmt", "yuv420p"].map(String::from).into()
        }
        Codec::Av1 => {
            need("libaom-av1")?;
            [
                "-c:v",
                "libaom-av1",
                "-cpu-used",
                "6",
                "-row-mt",
                "1",
                "-crf",
                &crf(28),
                "-b:v",
                "0",
                "-pix_fmt",
                "yuv420p",
            ]
            .map(String::from)
            .into()
        }
        Codec::ProRes => {
            need("prores_ks")?;
            ["-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le"].map(String::from).into()
        }
    };
    Ok(args)
}

/// What to encode.
pub struct EncodeJob<'a> {
    pub ffmpeg: &'a Path,
    pub output: &'a Path,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: Codec,
    pub crf: Option<u32>,
    /// An audio track and its length in seconds.
    pub audio: Option<(&'a Path, f64)>,
    /// The video's length in seconds.
    pub length: f64,
}

/// A running ffmpeg taking RGB frames on its input. Frames are written from
/// a separate thread, so that ffmpeg encodes while the next frame is made.
pub struct Encoder {
    child: Child,
    frames: Option<SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<std::io::Result<()>>>,
    log: Option<JoinHandle<String>>,
}

impl Encoder {
    pub fn start(job: &EncodeJob) -> Result<Encoder, String> {
        let available = encoders(job.ffmpeg)?;
        let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostats", "-y"].map(String::from).into();
        let fps = format!("{}", job.fps);
        args.extend(
            [
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-s",
                &format!("{}x{}", job.width, job.height),
                "-framerate",
                &fps,
                "-i",
                "-",
            ]
            .map(String::from),
        );
        if let Some((audio, _)) = job.audio {
            args.extend(["-i".to_string(), audio.to_string_lossy().into_owned()]);
        }
        args.extend(["-map", "0:v"].map(String::from));
        // RGB to BT.709 YUV, tagged as such, so players show the colours as rendered.
        args.extend(
            [
                "-vf",
                "scale=out_color_matrix=bt709:out_range=tv",
                "-colorspace",
                "bt709",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
            ]
            .map(String::from),
        );
        args.extend(codec_args(job.codec, job.crf, &available)?);
        if let Some((_, audio_len)) = job.audio {
            args.extend(["-map", "1:a"].map(String::from));
            // Pad short audio with silence, and fade out audio that runs past the end.
            let mut filter = "apad".to_string();
            if audio_len > job.length + 0.05 {
                let fade = (job.length / 4.0).min(2.0);
                filter = format!("afade=t=out:st={:.3}:d={fade:.3},apad", job.length - fade);
            }
            args.extend(["-af".to_string(), filter]);
            if job.codec == Codec::ProRes {
                args.extend(["-c:a", "pcm_s16le"].map(String::from));
            } else {
                args.extend(["-c:a", "aac", "-b:a", "256k"].map(String::from));
            }
            args.push("-shortest".into());
        }
        if job.codec != Codec::ProRes {
            args.extend(["-movflags", "+faststart"].map(String::from));
        }
        args.push(job.output.to_string_lossy().into_owned());

        let mut child = command(job.ffmpeg)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", job.ffmpeg.display()))?;
        let stderr = child.stderr.take().unwrap();
        let log = std::thread::spawn(move || {
            let mut text = String::new();
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                text.push_str(&line);
                text.push('\n');
            }
            text
        });
        let mut stdin: ChildStdin = child.stdin.take().unwrap();
        let (frames, rx): (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) = sync_channel(2);
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            for frame in rx {
                stdin.write_all(&frame)?;
            }
            stdin.flush()
        });
        Ok(Encoder { child, frames: Some(frames), writer: Some(writer), log: Some(log) })
    }

    /// Queue an RGBA frame of the job's size.
    pub fn write_rgba(&mut self, rgba: &[u8]) -> Result<(), String> {
        let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let sender = self.frames.as_ref().ok_or("the encoder is closed")?;
        if sender.send(rgb).is_err() {
            // The writer stopped: ffmpeg exited. Its log says why.
            return Err(self.fail());
        }
        Ok(())
    }

    fn fail(&mut self) -> String {
        self.frames = None;
        let _ = self.writer.take().map(|w| w.join());
        let _ = self.child.wait();
        let log = self.log.take().and_then(|l| l.join().ok()).unwrap_or_default();
        format!("ffmpeg failed: {}", log.trim())
    }

    /// Close the input and wait for ffmpeg to finish the file.
    pub fn finish(mut self) -> Result<(), String> {
        self.frames = None;
        let written = self.writer.take().map(|w| w.join());
        let status = self.child.wait().map_err(|e| e.to_string())?;
        let log = self.log.take().and_then(|l| l.join().ok()).unwrap_or_default();
        if !status.success() || !matches!(written, Some(Ok(Ok(())))) {
            return Err(format!("ffmpeg failed ({status}): {}", log.trim()));
        }
        Ok(())
    }

    /// Stop ffmpeg, leaving whatever it wrote.
    pub fn abort(mut self) {
        self.frames = None;
        let _ = self.child.kill();
        let _ = self.writer.take().map(|w| w.join());
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_durations() {
        let report = "Input #0, flac, from 'song.flac':\n  Duration: 00:03:21.45, start: 0.000000, bitrate: 900 kb/s\n";
        assert_eq!(parse_duration(report), Some(201.45));
        assert_eq!(parse_duration("nothing here"), None);
    }

    #[test]
    fn every_download_is_pinned() {
        for (_, _, d) in DOWNLOADS {
            assert_eq!(d.sha256.len(), 64);
            assert!(d.url.starts_with("https://") && d.url.contains(d.version));
        }
    }
}
