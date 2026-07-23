//! RTCM Inspector - the Stream detail bottom tab: live per-message-type
//! stream statistics plus the latest diagnostic decodes - base position with
//! baseline distance to the user, antenna/receiver descriptors, 1029 text,
//! 1230 bias presence.
//!
//! Pure rendering over `AppState`: the ntrip worker posts `RtcmBatch` events
//! and `state.rs` folds them into `NtripUi`; nothing here mutates stream
//! state. Ages tick without traffic because the main window already requests
//! a 1 s heartbeat repaint every frame.

use std::time::Instant;

use egui_extras::{Column, TableBuilder};
use gnss::rtcm::decode::{Decoded, type_name};

use crate::ui::{App, theme};

pub fn tab(app: &mut App, ui: &mut egui::Ui) {
    let now = Instant::now();
    type_table(app, ui, now);
    ui.add_space(6.0);
    totals_strip(app, ui);
    ui.separator();
    decoded_panel(app, ui);
}

// ----------------------------------------------------------------------
// Per-type statistics table
// ----------------------------------------------------------------------

fn type_table(app: &App, ui: &mut egui::Ui, now: Instant) {
    if app.state.ntrip.rtcm.is_empty() {
        ui.label(egui::RichText::new("No RTCM frames received yet").weak());
        return;
    }
    TableBuilder::new(ui)
        .id_salt("rtcm-type-table")
        .striped(true)
        .vscroll(true)
        .max_scroll_height(280.0)
        .column(Column::exact(40.0))
        .column(Column::remainder().at_least(170.0).clip(true))
        .column(Column::exact(64.0))
        .column(Column::exact(60.0))
        .column(Column::exact(48.0))
        .column(Column::exact(56.0))
        .header(18.0, |mut header| {
            for title in ["Type", "Name", "Count", "Rate", "Size", "Age"] {
                header.col(|ui| {
                    theme::header_band(ui);
                    ui.label(egui::RichText::new(title).strong().small());
                });
            }
        })
        .body(|mut body| {
            // BTreeMap iteration gives a stable, numerically sorted table.
            for (ty, stat) in &app.state.ntrip.rtcm {
                body.row(16.0, |mut row| {
                    num_col(&mut row, ty.to_string());
                    row.col(|ui| {
                        ui.label(type_name(*ty).unwrap_or("-"));
                    });
                    num_col(&mut row, stat.count.to_string());
                    num_col(&mut row, fmt_hz(stat.rate.hz()));
                    num_col(&mut row, stat.last_frame_len.to_string());
                    num_col(
                        &mut row,
                        fmt_age(now.duration_since(stat.last_seen).as_secs_f64()),
                    );
                });
            }
        });
}

/// Right-aligned monospace cell: numbers column-align so rates and counts
/// can be compared by eye while they update.
fn num_col(row: &mut egui_extras::TableRow<'_, '_>, text: String) {
    row.col(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.monospace(text);
        });
    });
}

/// Totals plus the two stream-health counters. CRC failures and garbage
/// bytes are the top "is this stream actually RTCM3" signals, so nonzero
/// values take the theme's error/warning colors.
fn totals_strip(app: &App, ui: &mut egui::Ui) {
    let n = &app.state.ntrip;
    let total_frames: u64 = n.rtcm.values().map(|s| s.count).sum();
    ui.horizontal(|ui| {
        ui.label(format!("{} types, {} frames", n.rtcm.len(), total_frames));
        ui.add_space(12.0);
        let crc = format!("CRC failures: {}", n.rtcm_crc_failures);
        if n.rtcm_crc_failures > 0 {
            ui.label(egui::RichText::new(crc).color(ui.visuals().error_fg_color));
        } else {
            ui.label(crc);
        }
        ui.add_space(12.0);
        let garbage = format!("garbage bytes: {}", n.rtcm_garbage_bytes);
        if n.rtcm_garbage_bytes > 0 {
            ui.label(egui::RichText::new(garbage).color(ui.visuals().warn_fg_color));
        } else {
            ui.label(garbage);
        }
    });
}

// ----------------------------------------------------------------------
// Decoded panel
// ----------------------------------------------------------------------

fn decoded_panel(app: &App, ui: &mut egui::Ui) {
    egui::Grid::new("rtcm-decoded-grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            base_rows(app, ui);
            antenna_rows(app, ui);
            text_row(app, ui);
            biases_row(app, ui);
        });
}

fn none_yet(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).weak());
}

fn base_rows(app: &App, ui: &mut egui::Ui) {
    ui.label("Base station");
    let Some(Decoded::BasePosition {
        station_id,
        is_1006,
        ecef_x_m,
        ecef_y_m,
        ecef_z_m,
        antenna_height_m,
        lla,
    }) = &app.state.ntrip.base
    else {
        none_yet(ui, "no 1005/1006 seen");
        ui.end_row();
        return;
    };
    ui.label(format!(
        "{station_id} (msg {})",
        if *is_1006 { 1006 } else { 1005 }
    ));
    ui.end_row();

    ui.label("Position");
    ui.label(position_text(*lla));
    ui.end_row();

    // Only 1006 carries DF028; 1005 must not render a phantom zero height.
    if let Some(h) = antenna_height_m {
        ui.label("Antenna height");
        ui.label(format!("{h:.4} m"));
        ui.end_row();
    }

    ui.label("Baseline");
    let g = &app.state.gnss;
    // GGA altitude is orthometric (MSL), not ellipsoidal; the geoid offset
    // (tens of meters worldwide) is noise at the "am I on the right base"
    // resolution this readout serves, and far smaller than the error of
    // ignoring height entirely (a mountain-top base over a valley rover).
    let receiver = match (g.lat_deg, g.lon_deg) {
        (Some(lat), Some(lon)) => Some((lat, lon, g.alt_m.map_or(0.0, f64::from))),
        _ => None,
    };
    let p = app.settings.active();
    let user = user_position(receiver, p.manual_lat, p.manual_lon);
    // The base ECEF is authoritative (it is what 1005/1006 carries); the
    // all-zero "position not set" pattern is gated out via the normalized
    // `lla`, exactly like the Position row above.
    let base_ecef = lla.map(|_| (*ecef_x_m, *ecef_y_m, *ecef_z_m));
    match baseline_text(user, base_ecef) {
        Some(t) => {
            ui.label(t);
        }
        None => none_yet(ui, "-"),
    }
    ui.end_row();
}

fn antenna_rows(app: &App, ui: &mut egui::Ui) {
    let Some(Decoded::AntennaInfo {
        station_id,
        antenna,
        setup_id,
        antenna_serial,
        receiver,
        firmware,
        receiver_serial,
    }) = &app.state.ntrip.antenna
    else {
        ui.label("Antenna");
        none_yet(ui, "no 1008/1033 seen");
        ui.end_row();
        return;
    };
    ui.label("Antenna");
    ui.label(format!(
        "{} (station {station_id}, setup {setup_id})",
        if antenna.is_empty() {
            "-"
        } else {
            antenna.as_str()
        }
    ));
    ui.end_row();
    if let Some(s) = antenna_serial {
        ui.label("Antenna serial");
        ui.label(s);
        ui.end_row();
    }
    if receiver.is_some() || firmware.is_some() {
        ui.label("Receiver");
        ui.label(receiver_text(receiver.as_deref(), firmware.as_deref()));
        ui.end_row();
    }
    if let Some(s) = receiver_serial {
        ui.label("Receiver serial");
        ui.label(s);
        ui.end_row();
    }
}

fn text_row(app: &App, ui: &mut egui::Ui) {
    ui.label("Text (1029)");
    match &app.state.ntrip.text_1029 {
        Some((
            at,
            Decoded::TextMessage {
                station_id, text, ..
            },
        )) => {
            ui.label(format!("[{at}] station {station_id}: {text}"));
        }
        _ => none_yet(ui, "none received"),
    }
    ui.end_row();
}

fn biases_row(app: &App, ui: &mut egui::Ui) {
    ui.label("GLONASS biases (1230)");
    match &app.state.ntrip.biases_1230 {
        Some(Decoded::GlonassBiases {
            station_id,
            biases_m,
            ..
        }) => {
            ui.label(format!(
                "seen (station {station_id}, {} biases)",
                biases_m.len()
            ));
        }
        _ => none_yet(ui, "not seen"),
    }
    ui.end_row();
}

// ----------------------------------------------------------------------
// Display logic (pure, unit-tested)
// ----------------------------------------------------------------------

/// Rate display: "-" until the meter's first full one-second window.
fn fmt_hz(hz: Option<f32>) -> String {
    match hz {
        None => "-".to_string(),
        Some(v) => format!("{v:.1} Hz"),
    }
}

/// Age since a type was last seen. Sub-10 s keeps a decimal - most RTCM
/// types repeat every 1-10 s, so that is the range where resolution earns
/// its ink; whole seconds carry to 10 minutes, minutes beyond.
fn fmt_age(secs: f64) -> String {
    if secs < 9.95 {
        format!("{secs:.1} s")
    } else if secs < 599.5 {
        format!("{secs:.0} s")
    } else {
        format!("{:.0} min", secs / 60.0)
    }
}

/// Baseline distance: meters below 2 km (the "am I on the right base" range
/// support reads out loud), kilometers with one decimal beyond.
fn fmt_baseline(d_m: f64) -> String {
    if d_m < 2000.0 {
        format!("{d_m:.0} m")
    } else {
        format!("{:.1} km", d_m / 1000.0)
    }
}

/// Base geodetic position line. `None` is decode.rs's normalization of the
/// all-zero-ECEF pattern an un-surveyed base broadcasts; say so instead of
/// inventing a point.
fn position_text(lla: Option<(f64, f64, f64)>) -> String {
    match lla {
        None => "position not set (all-zero ECEF)".to_string(),
        Some((lat, lon, alt)) => format!("{lat:.7}, {lon:.7}  alt {alt:.1} m"),
    }
}

/// The user-side end of the baseline as (lat, lon, height): the receiver's
/// last known GGA position when one exists, else the profile's manual point
/// at height 0 (the manual location has no altitude field, and a truncated
/// vertical is still far closer than no vertical at all). "Is the manual
/// point real" is the worker's `manual_position_set` judgement - one source
/// of truth for the (0, 0)-means-unset sentinel and the garbage-coordinate
/// guard, here as at the GGA send site.
fn user_position(
    receiver: Option<(f64, f64, f64)>,
    manual_lat: f64,
    manual_lon: f64,
) -> Option<((f64, f64, f64), &'static str)> {
    if let Some(p) = receiver {
        return Some((p, "receiver"));
    }
    if !crate::workers::ntrip::manual_position_set(manual_lat, manual_lon) {
        return None;
    }
    Some(((manual_lat, manual_lon, 0.0), "manual"))
}

/// Baseline label, or `None` when either end of the line is unknown.
///
/// 3D ECEF Euclidean per plan.md, not 2D haversine: a mountain-top base
/// 900 m above a valley rover IS 900 m away, and reporting "12 m" would
/// mislead exactly the "am I on the right base" judgement this readout
/// exists for. `base` is the message's own ECEF; the user's geodetic
/// position converts through the same WGS-84 model that decoded the base.
fn baseline_text(
    user: Option<((f64, f64, f64), &'static str)>,
    base: Option<(f64, f64, f64)>,
) -> Option<String> {
    let ((ulat, ulon, uh), source) = user?;
    let base = base?;
    let d = gnss::geodesy::ecef_distance_m(gnss::geodesy::lla_to_ecef(ulat, ulon, uh), base);
    Some(format!("{} (from {source} position)", fmt_baseline(d)))
}

/// 1033 receiver descriptor + firmware on one line; either may be absent.
fn receiver_text(receiver: Option<&str>, firmware: Option<&str>) -> String {
    match (receiver, firmware) {
        (Some(r), Some(f)) => format!("{r} fw {f}"),
        (Some(r), None) => r.to_string(),
        (None, Some(f)) => format!("fw {f}"),
        (None, None) => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gnss::geodesy::{ecef_distance_m, lla_to_ecef};

    /// The 3D distance between a user LLA and a base ECEF, through the same
    /// path `baseline_text` takes.
    fn baseline_m(user: (f64, f64, f64), base_ecef: (f64, f64, f64)) -> f64 {
        ecef_distance_m(lla_to_ecef(user.0, user.1, user.2), base_ecef)
    }

    #[test]
    fn rate_display() {
        assert_eq!(fmt_hz(None), "-", "no reading before the first window");
        assert_eq!(fmt_hz(Some(1.0)), "1.0 Hz");
        assert_eq!(fmt_hz(Some(12.34)), "12.3 Hz");
        assert_eq!(fmt_hz(Some(0.04)), "0.0 Hz", "decayed-to-silence rate");
    }

    #[test]
    fn age_display_ranges() {
        assert_eq!(fmt_age(0.0), "0.0 s");
        assert_eq!(fmt_age(0.44), "0.4 s");
        assert_eq!(fmt_age(9.94), "9.9 s");
        assert_eq!(fmt_age(9.96), "10 s", "no '10.0 s' double-width flicker");
        assert_eq!(fmt_age(59.4), "59 s");
        assert_eq!(fmt_age(599.4), "599 s");
        assert_eq!(fmt_age(599.6), "10 min");
        assert_eq!(fmt_age(3600.0), "60 min");
    }

    /// Hand-computed horizontal chord: (45 N, 122 W) to a point 0.01 deg of
    /// longitude west, both at h=0. The WGS-84 prime-vertical radius at
    /// 45 N is N = a/sqrt(1 - e^2 sin^2(45)) = 6378137/0.998325 =
    /// 6388838.4 m, so the parallel's radius is N cos(45) = 4517590.9 m and
    /// 0.01 deg of it spans 4517590.9 * 1.7453293e-4 = 788.5 m of chord.
    #[test]
    fn baseline_formats_meters_under_two_km() {
        let d = baseline_m((45.0, -122.0, 0.0), lla_to_ecef(45.0, -122.01, 0.0));
        assert!((d - 788.5).abs() < 0.5, "hand-computed distance: {d}");
        assert_eq!(fmt_baseline(d), "788 m");
    }

    /// Same construction on the equator, where the parallel's radius is the
    /// full semi-major axis a = 6378137 m: 0.0179 deg = 1992.6 m (still
    /// meters), 0.02 deg = 2226.4 m (kilometers) - the two sides of the
    /// 2 km display threshold.
    #[test]
    fn baseline_formats_km_from_two_km_up() {
        let just_under = baseline_m((0.0, 0.0, 0.0), lla_to_ecef(0.0, 0.0179, 0.0));
        assert!((just_under - 1992.6).abs() < 0.5, "{just_under}");
        assert_eq!(fmt_baseline(just_under), "1993 m");

        let over = baseline_m((0.0, 0.0, 0.0), lla_to_ecef(0.0, 0.02, 0.0));
        assert!((over - 2226.4).abs() < 0.5, "{over}");
        assert_eq!(fmt_baseline(over), "2.2 km");

        // The exact boundary.
        assert_eq!(fmt_baseline(1999.9), "2000 m");
        assert_eq!(fmt_baseline(2000.0), "2.0 km");
    }

    /// The reason the baseline is 3D ECEF and not 2D haversine (plan.md):
    /// a base 900 m directly above the user lies exactly 900 m up the
    /// shared ellipsoid normal, and a small horizontal offset barely moves
    /// that. The old lat/lon-only math called this "12 m".
    #[test]
    fn baseline_is_three_dimensional() {
        // Pure vertical separation: distance IS the height difference.
        let d = baseline_m((45.0, -122.0, 0.0), lla_to_ecef(45.0, -122.0, 900.0));
        assert!((d - 900.0).abs() < 1e-6, "vertical baseline: {d}");

        // Mountain-top base, valley rover: ~7.9 m horizontal, 900 m up.
        let d = baseline_m((45.0, -122.0, 0.0), lla_to_ecef(45.0, -122.0001, 900.0));
        assert!((900.0..901.0).contains(&d), "slant baseline: {d}");
        assert_eq!(fmt_baseline(d), "900 m");
    }

    #[test]
    fn user_position_prefers_receiver_then_manual() {
        assert_eq!(
            user_position(Some((45.0, -122.0, 120.5)), 40.0, -100.0),
            Some(((45.0, -122.0, 120.5), "receiver"))
        );
        // The manual point has no altitude field; it pins to h=0.
        assert_eq!(
            user_position(None, 40.0, -100.0),
            Some(((40.0, -100.0, 0.0), "manual"))
        );
        // Receiver wins even when the manual point is the (0,0) default.
        assert_eq!(
            user_position(Some((1.0, 2.0, 0.0)), 0.0, 0.0),
            Some(((1.0, 2.0, 0.0), "receiver"))
        );
    }

    #[test]
    fn user_position_treats_default_manual_as_unset() {
        assert_eq!(user_position(None, 0.0, 0.0), None);
        // A single zero coordinate is a legitimate position (equator or
        // prime meridian); only the exact (0,0) pair is the default.
        assert!(user_position(None, 0.0, -122.0).is_some());
        assert!(user_position(None, 45.0, 0.0).is_some());
    }

    #[test]
    fn baseline_text_requires_both_ends() {
        let base = lla_to_ecef(45.0, -122.01, 0.0);
        assert_eq!(baseline_text(None, Some(base)), None);
        assert_eq!(
            baseline_text(Some(((45.0, -122.0, 0.0), "manual")), None),
            None
        );
        let t = baseline_text(Some(((45.0, -122.0, 0.0), "receiver")), Some(base))
            .expect("both ends known");
        assert_eq!(t, "788 m (from receiver position)");
        let t = baseline_text(
            Some(((0.0, 0.0, 0.0), "manual")),
            Some(lla_to_ecef(0.0, 0.02, 0.0)),
        )
        .expect("both ends known");
        assert_eq!(t, "2.2 km (from manual position)");
    }

    #[test]
    fn position_text_all_zero_ecef_reads_as_not_set() {
        assert_eq!(position_text(None), "position not set (all-zero ECEF)");
        assert_eq!(
            position_text(Some((38.8047594, -77.0647736, 114.56))),
            "38.8047594, -77.0647736  alt 114.6 m"
        );
    }

    #[test]
    fn receiver_descriptor_line() {
        assert_eq!(
            receiver_text(Some("JAVAD TRE_G3TH DELTA"), Some("3.6.7")),
            "JAVAD TRE_G3TH DELTA fw 3.6.7"
        );
        assert_eq!(receiver_text(Some("RCVR"), None), "RCVR");
        assert_eq!(receiver_text(None, Some("1.0")), "fw 1.0");
        assert_eq!(receiver_text(None, None), "-");
    }
}
