//! One-shot OS effects: .wav alert playback and opening a URL in the
//! default browser. Both are fire-and-forget diagnostics conveniences -
//! failures are reported to the caller but never block or crash anything.
//!
//! Hand-rolled by design (no rodio, no webbrowser): on Windows a single
//! winmm/shell32 FFI call each; elsewhere a spawned system utility.

/// Play a .wav file asynchronously. Empty path = silent no-op (the settings
/// default). Returns Err with a human-readable reason for the event log.
pub fn play_wav(path: &str) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    // Async playback backends (SND_ASYNC, spawned players) cannot report a
    // missing file after they return; checking here is the only way the
    // Options [Test] button and the alert path get a real error.
    if !std::path::Path::new(path).is_file() {
        return Err(format!("audio file not found: {path}"));
    }
    play_wav_impl(path)
}

#[cfg(windows)]
fn play_wav_impl(path: &str) -> Result<(), String> {
    use std::sync::Mutex;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(psz_sound: *const u16, hmod: *const core::ffi::c_void, fdw_sound: u32)
        -> i32;
    }
    const SND_ASYNC: u32 = 0x0001;
    const SND_FILENAME: u32 = 0x0002_0000;
    const SND_NODEFAULT: u32 = 0x0002; // missing file -> error, not a beep

    // SND_ASYNC returns before playback finishes. MSDN only guarantees the
    // buffer may be released for SND_MEMORY-less calls after PlaySound
    // returns, but keeping the last path alive until the NEXT call (which
    // stops the previous sound) removes any doubt for zero cost.
    static LAST_PATH: Mutex<Vec<u16>> = Mutex::new(Vec::new());

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut held = LAST_PATH
        .lock()
        .map_err(|_| "audio mutex poisoned".to_string())?;
    *held = wide;
    // SAFETY: `held` is a NUL-terminated UTF-16 string that outlives the call
    // (and the playback, per the static above); winmm only reads it.
    let ok = unsafe {
        PlaySoundW(
            held.as_ptr(),
            std::ptr::null(),
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(format!("PlaySoundW failed for {path}"))
    }
}

#[cfg(not(windows))]
fn play_wav_impl(path: &str) -> Result<(), String> {
    // macOS ships afplay; Linux gets ALSA's aplay with PulseAudio's paplay as
    // the fallback. Spawned detached: alert latency must not block a worker.
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["afplay"]
    } else {
        &["aplay", "paplay"]
    };
    let mut last_err = String::new();
    for player in candidates {
        match std::process::Command::new(player)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(e) => last_err = format!("{player}: {e}"),
        }
    }
    Err(format!("no audio player available ({last_err})"))
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    open_url_impl(url)
}

#[cfg(windows)]
fn open_url_impl(url: &str) -> Result<(), String> {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *const core::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> isize;
    }
    const SW_SHOWNORMAL: i32 = 1;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let op = wide("open");
    let target = wide(url);
    // SAFETY: both strings are NUL-terminated and live across the call.
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null(),
            op.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Per the ShellExecuteW contract, values > 32 mean success.
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW returned {code} for {url}"))
    }
}

#[cfg(not(windows))]
fn open_url_impl(url: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{opener}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_path_is_silent_success() {
        assert_eq!(play_wav(""), Ok(()));
        assert_eq!(play_wav("   "), Ok(()));
    }

    #[test]
    fn missing_file_reports_error_not_silence() {
        let missing = std::env::temp_dir().join("open-ntrip-client-no-such-alert.wav");
        let _ = std::fs::remove_file(&missing);
        let r = play_wav(missing.to_string_lossy().as_ref());
        let err = r.expect_err("async playback cannot report this later; we must");
        assert!(err.contains("not found"), "{err}");
    }
}
