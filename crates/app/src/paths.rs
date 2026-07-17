//! Every file the app touches lives next to the exe - never the cwd, never
//! AppData - preserving the original's "copy the folder" portability. All
//! path helpers take an explicit base dir so tests can point them at a
//! tempdir; `exe_dir` is the one production anchor.

use std::path::{Path, PathBuf};

/// Directory containing the running executable. Falls back to "." only if
/// the OS cannot report the exe path at all (effectively never).
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn settings_file(base: &Path) -> PathBuf {
    base.join("settings.toml")
}

pub fn logs_dir(base: &Path) -> PathBuf {
    base.join("Logs")
}

pub fn nmea_dir(base: &Path) -> PathBuf {
    base.join("NMEA")
}

/// Raw correction captures: `Captures\YYYYMMDD_HHMMSS_{mount}.rtcm`.
pub fn captures_dir(base: &Path) -> PathBuf {
    base.join("Captures")
}

/// Marker the panic hook drops (holding the crash report's path) so the
/// next run can announce the crash in its event log, then delete it.
/// Deliberately in the base dir, NOT Logs: an uncreatable Logs dir is one
/// of the failure modes the marker must survive to report.
pub fn crash_marker(base: &Path) -> PathBuf {
    base.join("crash-pending.txt")
}
