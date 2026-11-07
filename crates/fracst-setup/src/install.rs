//! The work behind the screens: finding an existing install, closing Fracst,
//! and running the Inno Setup engine (packaging/windows/fracst.iss) silently.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The version this installer installs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The engine's AppId, which names its uninstall key. Test builds of the
/// engine use another one, and set FRACST_SETUP_APP_ID to match.
const APP_ID: &str = match option_env!("FRACST_SETUP_APP_ID") {
    Some(id) => id,
    None => "{6C1E2F43-8B0A-4E52-9D7C-3F1A5B0E7D21}",
};

/// The compiled engine, when the build was given one (see build.rs).
static ENGINE: &[u8] = include_bytes!(env!("FRACST_SETUP_ENGINE_FILE"));

/// Options from the command line, in Inno Setup's spelling: /DIR=, /ALLUSERS,
/// /CURRENTUSER, and --engine <path> for development builds.
#[derive(Default)]
pub struct Args {
    pub dir: Option<PathBuf>,
    pub all_users: Option<bool>,
    pub engine: Option<PathBuf>,
}

impl Args {
    pub fn parse() -> Self {
        let mut args = Args::default();
        let mut it = std::env::args_os().skip(1);
        while let Some(arg) = it.next() {
            let s = arg.to_string_lossy();
            let upper = s.to_ascii_uppercase();
            if let Some(dir) = upper.strip_prefix("/DIR=").map(|_| &s[5..]) {
                args.dir = Some(PathBuf::from(dir.trim_matches('"')));
            } else if upper == "/ALLUSERS" {
                args.all_users = Some(true);
            } else if upper == "/CURRENTUSER" {
                args.all_users = Some(false);
            } else if s == "--engine" {
                args.engine = it.next().map(PathBuf::from);
            }
        }
        args
    }
}

/// An installed Fracst, from its uninstall entry.
#[derive(Clone)]
pub struct Existing {
    pub version: String,
    pub dir: PathBuf,
}

impl Existing {
    /// How this installer's version compares with the installed one.
    pub fn order(&self) -> std::cmp::Ordering {
        let parse = |v: &str| v.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>();
        match (parse(VERSION), parse(&self.version)) {
            (Some(new), Some(old)) => new.cmp(&old),
            // An unreadable version (a dev build, say) is treated as older.
            _ => std::cmp::Ordering::Greater,
        }
    }
}

pub fn find_existing() -> Option<Existing> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6432KEY, RRF_SUBKEY_WOW6464KEY,
    };
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP_ID}_is1");
    [(HKEY_CURRENT_USER, 0), (HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6464KEY), (HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6432KEY)]
        .into_iter()
        .find_map(|(root, view)| {
            let version = reg_string(root, view, &key, "DisplayVersion")?;
            let dir = reg_string(root, view, &key, "Inno Setup: App Path")
                .or_else(|| reg_string(root, view, &key, "InstallLocation"))?;
            Some(Existing { version, dir: PathBuf::from(dir.trim_end_matches('\\')) })
        })
}

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

fn reg_string(root: windows_sys::Win32::System::Registry::HKEY, view: u32, key: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let (key, name) = (wide(key), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: null-terminated strings, and a buffer of the size given.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | view,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Where a first install goes unless the user picks another folder: what the
/// engine would choose ({autopf}\fracst).
pub fn default_dir(all_users: bool) -> PathBuf {
    let base = if all_users {
        std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).map(PathBuf::from)
    } else {
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs"))
    };
    base.unwrap_or_else(|| PathBuf::from(r"C:\Program Files")).join("fracst")
}

/// Asks every Fracst running from `dir` to close, as its own close button
/// would, and waits up to ten seconds for each. A Fracst that doesn't close
/// is left to the engine, which closes it too.
pub fn close_fracst(dir: &Path) {
    use windows_sys::Win32::Foundation::{CloseHandle, HWND};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
        WaitForSingleObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };

    let exe = dir.join("fracst.exe");
    let title = wide("Fracst");
    let mut wnd: HWND = std::ptr::null_mut();
    loop {
        // SAFETY: plain Win32 calls with valid arguments; the handle is closed below.
        unsafe {
            wnd = FindWindowExW(std::ptr::null_mut(), wnd, std::ptr::null(), title.as_ptr());
            if wnd.is_null() {
                break;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(wnd, &mut pid);
            let process = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                continue;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            // Only this install's Fracst: an Explorer window on a folder
            // named fracst has the same title.
            let ours = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0
                && String::from_utf16_lossy(&buf[..len as usize]).eq_ignore_ascii_case(&exe.to_string_lossy());
            if ours {
                PostMessageW(wnd, WM_CLOSE, 0, 0);
                WaitForSingleObject(process, 10_000);
            }
            CloseHandle(process);
        }
    }
}

/// What to install, for a first install. Updates keep the earlier choices.
pub struct Choices {
    pub dir: PathBuf,
    pub desktop: bool,
    pub path: bool,
    pub kfr: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Closing,
    Copying,
    Finishing,
}

/// A running install, shared with the screen that shows it.
#[derive(Default)]
pub struct Job {
    pub stage: Option<Stage>,
    /// Files the engine has written so far.
    pub files: usize,
    /// Set when it's over: where Fracst now is, or what went wrong.
    pub result: Option<Result<PathBuf, Failure>>,
}

#[derive(Clone)]
pub struct Failure {
    pub message: String,
    pub log: Option<PathBuf>,
}

/// About how many files the engine writes, for the progress bar.
pub const FILES: usize = 9;

/// Starts installing on a thread: closes Fracst when updating, then runs the
/// engine silently. `choices` is `None` for an update.
pub fn start(
    args: &Args,
    existing: Option<Existing>,
    choices: Option<Choices>,
    ctx: eframe::egui::Context,
) -> Arc<Mutex<Job>> {
    let job = Arc::new(Mutex::new(Job::default()));
    let shared = job.clone();
    let engine_path = args.engine.clone();
    let all_users = args.all_users;
    std::thread::spawn(move || {
        let set = |f: &dyn Fn(&mut Job)| {
            f(&mut shared.lock().unwrap());
            ctx.request_repaint();
        };
        if let Some(existing) = &existing {
            set(&|j| j.stage = Some(Stage::Closing));
            close_fracst(&existing.dir);
        }
        set(&|j| j.stage = Some(Stage::Copying));
        let result = run_engine(engine_path, all_users, choices.as_ref(), |files| {
            set(&|j| {
                j.files = files;
                if files >= FILES - 1 {
                    j.stage = Some(Stage::Finishing);
                }
            })
        });
        let result = result.and_then(|log_dir| {
            let _ = std::fs::remove_dir_all(log_dir);
            find_existing().map(|e| e.dir).ok_or_else(|| Failure {
                message: "Setup finished, but Fracst isn't registered as installed.".into(),
                log: None,
            })
        });
        set(&|j| j.result = Some(result.clone()));
    });
    job
}

/// Writes the engine to a temporary folder and runs it, reporting how many
/// files it has written. Returns the temporary folder.
fn run_engine(
    engine_path: Option<PathBuf>,
    all_users: Option<bool>,
    choices: Option<&Choices>,
    progress: impl Fn(usize),
) -> Result<PathBuf, Failure> {
    let fail = |message: String, log: Option<PathBuf>| Failure { message, log };
    let temp = std::env::temp_dir().join(format!("fracst-setup-{}", std::process::id()));
    std::fs::create_dir_all(&temp).map_err(|e| fail(format!("Couldn't make a temporary folder: {e}"), None))?;
    let engine = match engine_path {
        Some(path) => path,
        None if ENGINE.is_empty() => {
            return Err(fail("This build of the installer doesn't contain Fracst. Use a release build.".into(), None));
        }
        None => {
            let path = temp.join("fracst-engine.exe");
            std::fs::write(&path, ENGINE).map_err(|e| fail(format!("Couldn't unpack the installer: {e}"), None))?;
            path
        }
    };
    let log = temp.join("setup.log");

    let mut cmd = Command::new(&engine);
    cmd.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/NOCANCEL", "/SP-"]);
    cmd.raw_arg(format!("/LOG=\"{}\"", log.display()));
    match all_users {
        Some(true) => {
            cmd.arg("/ALLUSERS");
        }
        Some(false) => {
            cmd.arg("/CURRENTUSER");
        }
        None => {}
    }
    if let Some(c) = choices {
        cmd.raw_arg(format!("/DIR=\"{}\"", c.dir.display()));
        // Every task named, so none falls back to its default.
        let tasks = [("desktopicon", c.desktop), ("addtopath", c.path), ("kfr", c.kfr)]
            .map(|(name, on)| if on { name.to_string() } else { format!("!{name}") })
            .join(",");
        cmd.raw_arg(format!("/MERGETASKS=\"{tasks}\""));
    }
    let mut child = cmd.spawn().map_err(|e| fail(format!("Couldn't start the installer: {e}"), None))?;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(fail(format!("Lost track of the installer: {e}"), Some(log))),
        }
        if let Ok(text) = std::fs::read_to_string(&log) {
            progress(text.matches("Dest filename:").count());
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    match status.code() {
        Some(0) => Ok(temp),
        Some(2 | 5) => Err(fail("Setup was cancelled, so nothing changed.".into(), None)),
        Some(code) => Err(fail(format!("Setup stopped before it finished (code {code})."), Some(log))),
        None => Err(fail("Setup stopped before it finished.".into(), Some(log))),
    }
}

/// Opens the installed Fracst, as the user running this installer.
pub fn launch(dir: &Path) {
    let _ = Command::new(dir.join("fracst.exe")).current_dir(dir).spawn();
}
