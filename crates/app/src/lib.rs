//! Open NTRIP Client application crate.
//!
//! The binary is a thin shell over this library so the integration tests and
//! the --selftest harness exercise the exact worker/logging/settings stack
//! the GUI runs. Module map:
//!
//! - [`settings`]: frozen-schema TOML model + legacy Settings.txt import
//! - [`profiles`]: profile CRUD invariants behind the Profiles Manager
//! - [`bus`]: worker -> UI events, the logger fan-out, repaint throttling
//! - [`state`]: UI-thread state, mutated only by draining the bus
//! - [`workers`]: ntrip connection worker (+ reconnect supervisor), serial
//! - [`logging`]: daily log files on a dedicated thread
//! - [`audio`]: wav alert + browser-open one-shots (hand FFI, no crates)
//! - [`ui`]: eframe/egui main window and dialogs
//! - [`selftest`]: headless CLI harness over the same workers

pub mod audio;
pub mod bus;
pub mod logging;
pub mod paths;
pub mod profiles;
pub mod selftest;
pub mod settings;
pub mod state;
pub mod ui;
pub mod workers;

pub const REPO_URL: &str = "https://github.com/intnsity/OpenNTRIPClient";
pub const RELEASES_URL: &str = "https://github.com/intnsity/OpenNTRIPClient/releases";

/// User-Agent sent on every request, `NTRIP <name>/<version>` per convention.
pub fn user_agent() -> String {
    format!("NTRIP OpenNtripClient/{}", env!("CARGO_PKG_VERSION"))
}

/// Panic hook writing `Logs\crash-YYYYMMDD-HHMMSS.txt` next to the exe, then
/// deferring to the default hook (which aborts under panic=abort). A GUI
/// app has no console to die into; the crash file is the only witness.
///
/// Hardened after a field abort left NO crash file: everything that can be
/// computed before a panic is computed at install time, because with
/// panic=abort a second panic inside the hook aborts the process before any
/// report exists. That is also why stamping the filename with the INSTALL
/// time loses nothing - the first panic is the last. The body writes to the
/// Logs dir, falls back beside the exe, always echoes to stderr, and drops
/// a marker file so the next run can announce the crash in its event log.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    let exe = paths::exe_dir();
    let dir = paths::logs_dir(&exe);
    let name = logging::crash_filename(&gnss::clock::now_local());
    let file = dir.join(&name);
    let fallback = exe.join(&name);
    let marker = paths::crash_marker(&exe);
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "Open NTRIP Client v{} crashed\n\n{info}\n\nbacktrace:\n{backtrace}\n",
            env!("CARGO_PKG_VERSION")
        );
        write_crash_report(&dir, &file, &fallback, &marker, &report);
        default_hook(info);
    }));
}

/// The hook's fallible tail, factored out of the closure so tests can drive
/// it without installing a process-global hook. Returns the path the report
/// landed at, if any write succeeded. No `?`, no unwrap, no panic sources:
/// every failure degrades to the next sink.
fn write_crash_report(
    dir: &std::path::Path,
    file: &std::path::Path,
    fallback: &std::path::Path,
    marker: &std::path::Path,
    report: &str,
) -> Option<std::path::PathBuf> {
    use std::io::Write as _;
    let _ = std::fs::create_dir_all(dir);
    let written = if std::fs::write(file, report).is_ok() {
        Some(file.to_path_buf())
    } else if std::fs::write(fallback, report).is_ok() {
        Some(fallback.to_path_buf())
    } else {
        None
    };
    if let Some(p) = &written {
        // Consumed (and deleted) by take_crash_notice on the next boot.
        let _ = std::fs::write(marker, p.display().to_string());
    }
    // Stderr is a no-op for the windowed exe but the whole story for a
    // console-launched run; never eprintln! here - it panics on a broken
    // pipe, which inside a panic hook under panic=abort kills the report.
    let _ = std::io::stderr().write_all(report.as_bytes());
    written
}

/// If the previous run crashed (the panic hook left its marker), consume
/// the marker and return the crash report's path for the event log. The
/// marker is deleted so the notice appears exactly once - a crash must be
/// loud, not a permanent banner.
pub fn take_crash_notice(base: &std::path::Path) -> Option<String> {
    let marker = paths::crash_marker(base);
    let path = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    Some(path.trim().to_string()).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{take_crash_notice, write_crash_report};

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ontc-lib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    /// Happy path: the Logs dir is created on demand, the report lands in
    /// it, and the marker round-trips through take_crash_notice exactly once.
    #[test]
    fn crash_report_and_marker_roundtrip() {
        let base = tempdir("crash-ok");
        let logs = base.join("Logs");
        let file = logs.join("crash-1.txt");
        let fallback = base.join("crash-1.txt");
        let marker = crate::paths::crash_marker(&base);
        let written = write_crash_report(&logs, &file, &fallback, &marker, "it broke\n");
        assert_eq!(written.as_deref(), Some(file.as_path()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "it broke\n");
        let notice = take_crash_notice(&base).expect("marker must surface the crash");
        assert_eq!(notice, file.display().to_string());
        // Consumed: the notice appears exactly once.
        assert!(take_crash_notice(&base).is_none());
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The field failure this hardening answers: the Logs dir cannot be
    /// created (here: its parent path is a FILE), yet a report must still
    /// land somewhere findable - beside the exe - and be announced.
    #[test]
    fn crash_report_falls_back_beside_the_exe() {
        let base = tempdir("crash-fb");
        // A file where the Logs dir should be makes create_dir_all fail.
        let obstruction = base.join("Logs");
        std::fs::write(&obstruction, b"not a directory").unwrap();
        let logs = obstruction.clone();
        let file = logs.join("crash-2.txt");
        let fallback = base.join("crash-2.txt");
        // The marker lives in the BASE dir precisely so this scenario can
        // still be announced on the next boot.
        let marker = crate::paths::crash_marker(&base);
        let written = write_crash_report(&logs, &file, &fallback, &marker, "still broke\n");
        assert_eq!(written.as_deref(), Some(fallback.as_path()));
        assert_eq!(std::fs::read_to_string(&fallback).unwrap(), "still broke\n");
        assert_eq!(
            take_crash_notice(&base).as_deref(),
            Some(fallback.display().to_string().as_str())
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// No marker (the overwhelmingly common boot): silence, no error.
    #[test]
    fn no_crash_marker_means_no_notice() {
        let base = tempdir("crash-none");
        assert!(take_crash_notice(&base).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
