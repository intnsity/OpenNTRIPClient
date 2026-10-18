//! The session band's second line while frames flow: what is in the data.
//! The base station's identity and position, how many message types at what
//! byte rate, and whether the RTCM framing is clean; a click drops the user
//! into the Stream detail tab for the full decode.
//!
//! It renders only while a live session has actually produced frames;
//! otherwise the band shows its next-step line in the same slot.

use gnss::rtcm::decode::Decoded;

use crate::settings::BottomTab;
use crate::ui::{App, theme};

/// Render the summary line if there is one; false leaves the slot to the
/// caller.
pub fn line(app: &mut App, ui: &mut egui::Ui) -> bool {
    // Only while a worker is live AND frames have arrived: `reset_stream_stats`
    // clears `rtcm` at each connect, and `ntrip_busy` falls at stop, so this
    // predicate is precisely "a session is streaming content right now".
    if !app.ntrip_busy() || app.state.ntrip.rtcm.is_empty() {
        return false;
    }

    let n = &app.state.ntrip;
    let base = match &n.base {
        Some(Decoded::BasePosition {
            station_id, lla, ..
        }) => Some((*station_id, lla.map(|(lat, lon, _)| (lat, lon)))),
        _ => None,
    };
    let frames: u64 = n.rtcm.values().map(|s| s.count).sum();
    let (head, chip, health) = summary_line(
        base,
        n.rtcm.len(),
        frames,
        app.rate_kbps,
        n.rtcm_crc_failures,
        n.rtcm_garbage_bytes,
    );

    // The neutral headline reads in secondary ink; only the framing-health
    // chip takes a semantic color, so a clean stream stays calm and CRC
    // trouble is the one thing that jumps. Built as one LayoutJob so the whole
    // line is a single clickable target.
    let mut job = egui::text::LayoutJob::default();
    let font = egui::TextStyle::Small.resolve(ui.style());
    job.append(
        &format!("{head}   -   "),
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: theme::ink_secondary(),
            ..Default::default()
        },
    );
    job.append(
        &chip,
        0.0,
        egui::TextFormat {
            font_id: font,
            color: match health {
                Health::Clean => theme::success(),
                Health::Issues => theme::warning(),
            },
            ..Default::default()
        },
    );

    // A left-aligned, content-width clickable line - the same subtle
    // "reads as text, lights up on hover, acts as a link" affordance the
    // disclosure headers use - not a centered full-width button.
    let resp = ui
        .selectable_label(false, job)
        .on_hover_text("Open the Stream detail tab");
    if resp.clicked() {
        app.settings.window.tab = BottomTab::Stream;
    }
    true
}

/// Whether the RTCM framing looks clean. Any CRC failure or stray byte is a
/// red flag that the "stream" may not be RTCM3 at all, so it is the one part
/// of the summary worth a color.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Health {
    Clean,
    Issues,
}

/// The summary's two text halves - the neutral headline and the framing-health
/// chip - plus the chip's health class. Pure over primitives so the wording
/// is unit-tested without standing up an `NtripUi`. `base` is
/// `(station_id, Some((lat, lon)))`, or `(id, None)` for an un-surveyed base
/// broadcasting all-zero ECEF.
fn summary_line(
    base: Option<(u16, Option<(f64, f64)>)>,
    types: usize,
    frames: u64,
    rate_kbps: f64,
    crc_failures: u64,
    garbage_bytes: u64,
) -> (String, String, Health) {
    let mut head = match base {
        Some((id, Some((lat, lon)))) => format!("station {id}  {lat:.5}, {lon:.5}"),
        Some((id, None)) => format!("station {id}  (position not set)"),
        None => "no base station yet".to_string(),
    };
    head.push_str(&format!(
        "   -   {types} types, {frames} frames   {rate_kbps:.1} kB/s"
    ));

    if crc_failures == 0 && garbage_bytes == 0 {
        (head, "framing clean".to_string(), Health::Clean)
    } else {
        let mut chip = format!("{crc_failures} CRC fails");
        if garbage_bytes > 0 {
            chip.push_str(&format!(", {garbage_bytes} garbage bytes"));
        }
        (head, chip, Health::Issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_stream_with_base_position() {
        let (head, chip, health) = summary_line(
            Some((1234, Some((45.52021, -122.67419)))),
            8,
            4210,
            1.23,
            0,
            0,
        );
        assert_eq!(
            head,
            "station 1234  45.52021, -122.67419   -   8 types, 4210 frames   1.2 kB/s"
        );
        assert_eq!(chip, "framing clean");
        assert_eq!(health, Health::Clean);
    }

    #[test]
    fn base_without_surveyed_position() {
        let (head, _, _) = summary_line(Some((7, None)), 3, 90, 0.4, 0, 0);
        assert!(head.starts_with("station 7  (position not set)"), "{head}");
    }

    #[test]
    fn no_base_yet_still_summarizes_frames() {
        let (head, chip, health) = summary_line(None, 2, 12, 0.8, 0, 0);
        assert_eq!(
            head,
            "no base station yet   -   2 types, 12 frames   0.8 kB/s"
        );
        assert_eq!(chip, "framing clean");
        assert_eq!(health, Health::Clean);
    }

    /// CRC failures alone, and CRC plus garbage, both read as Issues and name
    /// the counts - the "this may not be RTCM3" signal the color exists for.
    #[test]
    fn framing_trouble_is_flagged() {
        let (_, chip, health) = summary_line(Some((1, Some((0.0, 0.0)))), 4, 100, 1.0, 3, 0);
        assert_eq!(chip, "3 CRC fails");
        assert_eq!(health, Health::Issues);

        let (_, chip, health) = summary_line(None, 4, 100, 1.0, 3, 512);
        assert_eq!(chip, "3 CRC fails, 512 garbage bytes");
        assert_eq!(health, Health::Issues);
    }
}
