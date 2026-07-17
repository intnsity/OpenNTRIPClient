//! Open NTRIP Client binary shell. All real logic lives in the library so
//! tests and --selftest exercise the identical stack.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use open_ntrip_client::{install_panic_hook, paths, settings, ui};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Headless mode runs before ANY eframe/GUI initialization.
    if args.iter().any(|a| a == "--selftest") {
        return ExitCode::from(open_ntrip_client::selftest::run(&args));
    }

    install_panic_hook();
    let base = paths::exe_dir();
    let (settings, boot_log) = settings::load_or_import(&base);

    // Window geometry restores from settings.toml; eframe's own persistence
    // stays off because it writes to AppData and this app is portable.
    // Minimum width matches the default (760): below it the NTRIP card's
    // fixed-width rows overflow their column and drag the right-anchored
    // status cluster and log buttons off-screen. The restored size is
    // clamped by hand too - verified on Windows that the initial
    // with_inner_size is NOT constrained by with_min_inner_size, so a
    // sub-minimum size in settings.toml would still open clipped.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Open NTRIP Client")
        .with_icon(window_icon())
        .with_inner_size(clamp_to_min(settings.window.size))
        .with_min_inner_size(MIN_INNER_SIZE);
    if let Some([x, y]) = settings.window.pos {
        viewport = viewport.with_position([x as f32, y as f32]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let result = eframe::run_native(
        "Open NTRIP Client",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, base, settings, boot_log)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fatal: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The smallest window the layout genuinely fits: the default width (the
/// NTRIP card's Host/Port/TLS row needs ~365 px of column) by the historical
/// height floor.
const MIN_INNER_SIZE: [f32; 2] = [760.0, 480.0];

/// Componentwise clamp of a restored window size up to [`MIN_INNER_SIZE`].
fn clamp_to_min(size: [f32; 2]) -> [f32; 2] {
    [
        size[0].max(MIN_INNER_SIZE[0]),
        size[1].max(MIN_INNER_SIZE[1]),
    ]
}

/// The runtime window/taskbar icon, embedded at compile time. The .ico that
/// build.rs compiles into the exe only covers Explorer and the exe file
/// itself; the live window needs the pixels handed to the windowing layer,
/// and a raw pre-decoded RGBA blob keeps image-format decoding out of the
/// dependency tree. Cross-platform for free: the same bytes serve the
/// Linux/macOS builds, which have no .ico at all.
fn window_icon() -> egui::IconData {
    const RGBA: &[u8] = include_bytes!("../../../assets/icon-rgba-64.bin");
    const SIDE: u32 = 64;
    // A wrong-sized blob would make eframe panic deep inside winit at
    // window creation; fail here, at the source of truth, instead.
    assert_eq!(
        RGBA.len(),
        (SIDE * SIDE * 4) as usize,
        "assets/icon-rgba-64.bin must be 64x64 RGBA"
    );
    egui::IconData {
        rgba: RGBA.to_vec(),
        width: SIDE,
        height: SIDE,
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_to_min, window_icon};

    /// A restored sub-minimum geometry (or a hand-edited settings.toml)
    /// must open at the layout's real minimum: winit does not apply the
    /// min-size constraint to the initial size on Windows.
    #[test]
    fn restored_window_size_clamps_to_layout_minimum() {
        assert_eq!(clamp_to_min([700.0, 560.0]), [760.0, 560.0]);
        assert_eq!(clamp_to_min([1024.0, 400.0]), [1024.0, 480.0]);
        assert_eq!(clamp_to_min([760.0, 480.0]), [760.0, 480.0]);
        assert_eq!(clamp_to_min([1920.0, 1080.0]), [1920.0, 1080.0]);
    }

    /// The embedded window icon must stay a well-formed 64x64 RGBA blob -
    /// a truncated or resized asset otherwise surfaces as a winit panic at
    /// startup on the user's machine.
    #[test]
    fn window_icon_is_valid_64x64_rgba() {
        let icon = window_icon();
        assert_eq!(icon.width, 64);
        assert_eq!(icon.height, 64);
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        // Not fully transparent: at least one visible pixel proves the blob
        // is image data rather than zeroes.
        assert!(icon.rgba.chunks_exact(4).any(|px| px[3] != 0));
    }
}
