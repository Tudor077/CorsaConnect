//! Talking to the other app that wants the same telemetry.
//!
//! PicoPanel (`Projects\PicoPanel`) drives an RP2040 dashboard panel and reads
//! BeamNG through OutGauge - the same single-listener protocol we bind. Only
//! one process can have UDP 4444, so running both used to mean one of them sat
//! silent until you closed the other.
//!
//! PicoPanel already knows how to be the one that steps aside: its settings
//! have a `yield_outgauge` flag that skips its own OutGauge source, and it has
//! a "Corsa" source that listens on UDP 5051 for the packets we send the phone.
//! So the fix is entirely on our side of the fence: keep 4444, turn our mirror
//! on, flip that flag for it, and start it. That's what the launcher card does.
//!
//! Everything here is best effort and reversible. Nothing runs unless the user
//! presses a button, and the one file we write outside our own folder is
//! PicoPanel's settings.json - edited in place, one key, other keys untouched.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// Where PicoPanel's "Corsa" source listens. Matches `CorsaSource(port=5051)`
/// in `PicoPanel/pc/telemetry/sources.py`.
pub const DEFAULT_MIRROR: &str = "127.0.0.1:5051";

/// The setting PicoPanel reads at startup to decide whether to bind 4444.
const YIELD_KEY: &str = "yield_outgauge";

/// Don't flash a console window when we shell out (we're a windowed app).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Likely spots for `PicoPanel.exe`, best first. The build script drops it in
/// the project root, which is where it normally lives.
pub fn find_exe() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        candidates.push(PathBuf::from(&profile).join(r"Projects\PicoPanel\PicoPanel.exe"));
    }
    // Next to us, and one level up beside our own project folder.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("PicoPanel.exe"));
            if let Some(up) = dir.parent().and_then(|p| p.parent()) {
                candidates.push(up.join(r"PicoPanel\PicoPanel.exe"));
            }
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"PicoPanel\PicoPanel.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Whether a PicoPanel process is up right now - including one the user
/// started themselves, which is the case we most need to notice.
pub fn is_running() -> bool {
    let out = no_window(&mut Command::new("tasklist"))
        .args(["/FI", "IMAGENAME eq PicoPanel.exe", "/NH"])
        .output();
    match out {
        // tasklist with no match still exits 0, printing an INFO line instead.
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains("PicoPanel.exe"),
        Err(_) => false,
    }
}

/// Start PicoPanel. The window comes up so the user can see it took; it parks
/// itself in the tray from there.
pub fn start(exe: &Path) -> Result<Child, String> {
    Command::new(exe)
        // Its settings and logs are relative to nothing, but a sane working
        // directory still beats inheriting ours.
        .current_dir(exe.parent().unwrap_or_else(|| Path::new(".")))
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", exe.display()))
}

/// PicoPanel's settings file: `%LOCALAPPDATA%\PicoPanel\settings.json`.
pub fn settings_path() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA")
        .ok()
        .map(|d| PathBuf::from(d).join(r"PicoPanel\settings.json"))
}

/// Current value of the yield flag: `Some(true)` if PicoPanel is set to leave
/// 4444 alone, `None` if there's no settings file yet (it writes one when you
/// first change something in its window).
pub fn yields_outgauge() -> Option<bool> {
    let text = std::fs::read_to_string(settings_path()?).ok()?;
    let line = text.lines().find(|l| l.contains(YIELD_KEY))?;
    Some(line.contains("true"))
}

/// Set the yield flag, creating the file if PicoPanel has never saved one.
///
/// Hand-edited rather than parsed and re-serialised: it's someone else's file,
/// it may hold keys we know nothing about, and a settings file is exactly the
/// kind of thing you don't get to rewrite wholesale on a guess. One line
/// changes; anything unexpected is an error, not a repair.
pub fn set_yield_outgauge(on: bool) -> Result<PathBuf, String> {
    let path = settings_path().ok_or("LOCALAPPDATA is not set")?;
    let value = if on { "true" } else { "false" };

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let fresh = format!("{{\n  \"{YIELD_KEY}\": {value}\n}}\n");
            std::fs::write(&path, fresh).map_err(|e| format!("{}: {e}", path.display()))?;
            return Ok(path);
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };

    let updated = if let Some(i) = text.lines().position(|l| l.contains(YIELD_KEY)) {
        let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        let comma = if lines[i].trim_end().ends_with(',') { "," } else { "" };
        let indent: String = lines[i].chars().take_while(|c| c.is_whitespace()).collect();
        lines[i] = format!("{indent}\"{YIELD_KEY}\": {value}{comma}");
        lines.join("\n") + "\n"
    } else {
        // No such key yet: slide it in right after the opening brace, keeping
        // whatever follows exactly as it was.
        let open = text.find('{').ok_or_else(|| {
            format!("{} doesn't look like JSON - left alone", path.display())
        })?;
        let rest = text[open + 1..].trim_start_matches(['\r', '\n']);
        let comma = if rest.trim_start().starts_with('}') { "" } else { "," };
        format!(
            "{}{{\n  \"{YIELD_KEY}\": {value}{comma}\n{}",
            &text[..open],
            rest
        )
    };

    std::fs::write(&path, updated).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}
