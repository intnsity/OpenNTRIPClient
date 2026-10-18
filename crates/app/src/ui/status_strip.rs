//! Fix-quality banner, the two configurable readout slots, and the
//! stream-activity cluster - the parity replacement for the original
//! client's green data bar, plus honest "needs receiver" states on the
//! receiver-derived readouts.

use std::time::{Duration, Instant};

use crate::bus::NtripStatus;
use crate::settings::DisplayId;
// STALL_AFTER lives in state.rs beside the rx_age/stalled predicate it
// parameterizes; re-exported here because it is part of this module's
// classification contract too.
pub use crate::state::STALL_AFTER;
use crate::state::{GnssState, RECEIVER_SILENT_AFTER};
use crate::ui::{App, theme};

/// Half-life of the activity envelope. A full pulse decays to ~0.18 between
/// 1 Hz RTCM bursts, so the bar visibly breathes at real correction rates
/// (0.5-1 kB/s) instead of sitting at a constant-looking level.
pub const ACTIVITY_HALF_LIFE: Duration = Duration::from_millis(400);

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let now = Instant::now();
    let receiver = ReceiverPresence {
        nmea_seen: app.state.gnss.nmea_seen,
        serial_connected: app.serial.is_some(),
    };
    let stream = StreamView {
        // No time-since-connect fallback here: a readout labeled
        // "Data Age" must not present time-since-connect as data age.
        last_rx_age_s: app
            .state
            .ntrip
            .rx_age(now)
            .and(app.state.ntrip.last_rx)
            .map(|t| now.duration_since(t).as_secs_f32()),
        rate_kbps: app
            .state
            .ntrip
            .rx_age(now)
            .is_some()
            .then_some(app.rate_kbps),
    };

    ui.horizontal(|ui| {
        // Everything up to the activity cluster is the receiver's word, not
        // the caster's - the most common misreading of this row.
        ui.label(theme::caption("From receiver"));
        let banner = fix_banner(&app.state.gnss, receiver.serial_connected, now);
        let resp = ui.label(
            egui::RichText::new(banner.text)
                .size(24.0)
                .strong()
                .color(banner.ink),
        );
        if let Some(note) = &banner.note {
            ui.label(theme::caption(note));
        }
        // The banner is the largest receiver-derived readout on screen: while
        // it has never seen a GGA, say where fix data would come from.
        if !app.state.gnss.has_fix_data {
            resp.on_hover_text(receiver.hint_text());
        } else if banner.note.is_some() {
            resp.on_hover_text("The receiver has stopped sending NMEA; this was its last fix.");
        }
        // Nothing else repaints a quiet app, yet a live fix must go stale the
        // moment the receiver does, and a stale note's age must keep counting.
        if app.state.gnss.has_fix_data {
            let next = match (&banner.note, app.state.gnss.nmea_age(now)) {
                (None, Some(age)) => {
                    RECEIVER_SILENT_AFTER.saturating_sub(age) + Duration::from_millis(50)
                }
                _ => Duration::from_secs(1),
            };
            ui.ctx().request_repaint_after(next);
        }
        ui.add_space(18.0);
        slot(
            ui,
            "slot-center",
            &mut app.settings.display.center,
            &app.state.gnss,
            &stream,
            receiver,
        );
        ui.add_space(18.0);
        slot(
            ui,
            "slot-right",
            &mut app.settings.display.right,
            &app.state.gnss,
            &stream,
            receiver,
        );
    });
}

// ---------------------------------------------------------------------
// Stream activity - pure logic
// ---------------------------------------------------------------------

/// What the activity cluster communicates this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamActivity {
    /// No live session: idle, stopped, or waiting out a reconnect delay.
    Idle,
    /// Connecting, or connected and inside the pre-data grace window.
    Waiting,
    /// Corrections flowing.
    Active { rate_kbps: f64 },
    /// Connected but silent past [`STALL_AFTER`].
    Stalled { silent_s: f32 },
}

/// Map the worker status plus stream silence onto the cluster mode.
/// `silent` is time since the last correction bytes (or since the status
/// change, before any data).
pub fn classify_stream(status: &NtripStatus, silent: Duration, rate_kbps: f64) -> StreamActivity {
    match status {
        NtripStatus::Streaming | NtripStatus::WaitingForData if silent > STALL_AFTER => {
            StreamActivity::Stalled {
                silent_s: silent.as_secs_f32(),
            }
        }
        NtripStatus::Streaming => StreamActivity::Active { rate_kbps },
        NtripStatus::WaitingForData | NtripStatus::Connecting { .. } => StreamActivity::Waiting,
        NtripStatus::Idle | NtripStatus::ReconnectWait { .. } | NtripStatus::Stopped { .. } => {
            StreamActivity::Idle
        }
    }
}

/// Impulse-plus-decay envelope step: silence halves the level every
/// [`ACTIVITY_HALF_LIFE`]. The caller snaps the level to 1.0 whenever new
/// bytes arrive; this rhythm - full on a burst, draining between bursts -
/// is what made the original's bar read as a live heartbeat, and it is
/// independent of the stream's byte rate.
pub fn decay_activity(level: f32, dt: Duration) -> f32 {
    let halves = dt.as_secs_f32() / ACTIVITY_HALF_LIFE.as_secs_f32();
    (level * 0.5_f32.powf(halves)).clamp(0.0, 1.0)
}

/// One frame's step of the caller-owned activity envelope: decay for the
/// elapsed time, then snap to full when the session byte total grew. The
/// ordering is load-bearing - snap AFTER decay, so a burst landing on this
/// very frame renders at full brightness instead of pre-dimmed. A total
/// that went backward is a new session's counter reset, not a burst: the
/// envelope only decays (the connect path also zeroes it explicitly).
pub fn advance_activity(level: f32, dt: Duration, last_total: u64, new_total: u64) -> f32 {
    let decayed = decay_activity(level, dt);
    if new_total > last_total { 1.0 } else { decayed }
}

/// Caption text and ink for the cluster. Stall is signal orange (warning is
/// never yellow on yellow paper); idle recedes to secondary ink.
pub fn cluster_caption(mode: &StreamActivity) -> (String, egui::Color32) {
    match mode {
        StreamActivity::Idle => ("-".to_string(), theme::ink_secondary()),
        StreamActivity::Waiting => ("waiting...".to_string(), theme::ink_secondary()),
        StreamActivity::Active { rate_kbps } => {
            (format!("{rate_kbps:.1} kB/s"), theme::ink_primary())
        }
        StreamActivity::Stalled { silent_s } => {
            (format!("no data {silent_s:.0} s"), theme::warning())
        }
    }
}

/// Well outline: opaque muted ink normally, warning orange while stalled -
/// the only state where the bar itself must demand attention. INK_MUTED here
/// is the token's legal non-text-boundary role (3:1+ on every paper): with
/// the old 14%-alpha hairline an idle bar measured ~1.3:1 against the panel,
/// invisible exactly when a user scans for where data will appear.
pub fn bar_stroke_color(mode: &StreamActivity) -> egui::Color32 {
    match mode {
        StreamActivity::Stalled { .. } => theme::warning(),
        _ => theme::ink_muted(),
    }
}

/// Fill fraction actually painted: the envelope while a session is live,
/// hard zero when disconnected so a stale envelope can never suggest data.
pub fn bar_fill_level(mode: &StreamActivity, level: f32) -> f32 {
    match mode {
        StreamActivity::Idle => 0.0,
        _ => level.clamp(0.0, 1.0),
    }
}

// ---------------------------------------------------------------------
// Stream activity - rendering
// ---------------------------------------------------------------------

const BAR_SIZE: egui::Vec2 = egui::vec2(90.0, 10.0);

/// The bar plus its caption, right-to-left; rendered by the session band.
pub fn activity_cluster(ui: &mut egui::Ui, mode: &StreamActivity, level: f32) {
    // Right-to-left row: the bar takes the right edge, caption sits left.
    let bar = activity_bar(ui, mode, level);
    let (caption, color) = cluster_caption(mode);
    let text = ui.label(
        egui::RichText::new(caption)
            .small()
            .monospace()
            .color(color),
    );
    let tip = "Correction data arriving from the caster. The bar pulses \
               with each burst; the running total is below the graph.";
    bar.on_hover_text(tip);
    text.on_hover_text(tip);
}

/// The recessed gauge itself: a PAPER_0 well with an emerald fill driven by
/// the activity envelope. Sense::hover only - it is a gauge, not a button.
fn activity_bar(ui: &mut egui::Ui, mode: &StreamActivity, level: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(BAR_SIZE, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 3, theme::paper_0());
        let inner = rect.shrink(1.0);
        let w = inner.width() * bar_fill_level(mode, level);
        if w >= 0.5 {
            p.rect_filled(
                egui::Rect::from_min_size(inner.min, egui::vec2(w, inner.height())),
                2,
                theme::success(),
            );
        }
        p.rect_stroke(
            rect,
            3,
            egui::Stroke::new(1.0, bar_stroke_color(mode)),
            egui::StrokeKind::Inside,
        );
    }
    resp
}

// ---------------------------------------------------------------------
// Fix banner
// ---------------------------------------------------------------------

/// What the fix banner shows: the big word, its ink, and an optional small
/// note beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct FixBanner {
    pub text: &'static str,
    pub ink: egui::Color32,
    pub note: Option<String>,
}

/// The banner's state machine. `has_fix_data` latches for the run, so a
/// receiver unplugged mid-RTK used to leave "RTK Fixed" glowing green; a
/// fix is live only while the port is open and NMEA keeps arriving.
/// Otherwise the last quality recedes to secondary ink with a note saying
/// why. Before any GGA the word names the missing link instead of an
/// ambiguous "no fix".
pub fn fix_banner(g: &GnssState, serial_open: bool, now: Instant) -> FixBanner {
    let talking = g.nmea_age(now).is_some_and(|a| a <= RECEIVER_SILENT_AFTER);
    if !g.has_fix_data {
        let text = match (serial_open, talking) {
            (false, _) => "No receiver",
            (true, false) => "Waiting for NMEA",
            (true, true) => "No GGA from receiver",
        };
        return FixBanner {
            text,
            ink: fix_quality_color(false, g.quality),
            note: None,
        };
    }
    let text = gnss::nmea::quality_name(g.quality);
    if serial_open && talking {
        return FixBanner {
            text,
            ink: fix_quality_color(true, g.quality),
            note: None,
        };
    }
    let note = match g.nmea_age(now) {
        Some(age) if !serial_open => format!("(last seen {})", super::text::age_label(age)),
        _ => "(receiver silent)".to_string(),
    };
    FixBanner {
        text,
        ink: theme::ink_secondary(),
        note: Some(note),
    }
}

/// Butter Paper fix-quality semantics. The original's red/yellow/orange/
/// green language maps onto the paper's cool semantic set: RTK Fixed is the
/// success state, RTK Float the warning (signal orange - warning is never
/// yellow on yellow paper), Invalid the danger. Ordinary GPS/DGPS/WAAS
/// fixes are unremarkable working states, so they read in plain ink; the
/// pre-data placeholder and the exotic qualities recede one step to
/// secondary ink - and no further: the placeholder is the banner's primary
/// readout when there is no fix, so it must stay comfortably above the
/// WCAG floor (INK_SECONDARY is ~5.6:1 on the base paper) even though it
/// renders large. Muted ink is reserved for disabled controls.
pub fn fix_quality_color(has_fix_data: bool, quality: u8) -> egui::Color32 {
    if !has_fix_data {
        return theme::ink_secondary();
    }
    match quality {
        0 => theme::danger(),              // Invalid
        4 => theme::success(),             // RTK Fixed
        5 => theme::warning(),             // RTK Float
        1 | 2 | 9 => theme::ink_primary(), // GPS / DGPS / WAAS
        _ => theme::ink_secondary(),       // PPS, Estimated, Manual, Simulation
    }
}

// ---------------------------------------------------------------------
// Readout slots
// ---------------------------------------------------------------------

/// Whether a GPS receiver is (or has been) supplying NMEA - the slots'
/// prerequisite. `nmea_seen` is any parsed sentence this app run, sticky:
/// once a receiver has spoken, a transiently missing field is normal
/// session behavior, not a setup error to hint about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReceiverPresence {
    pub nmea_seen: bool,
    pub serial_connected: bool,
}

impl ReceiverPresence {
    /// Tooltip explaining why a receiver-derived readout is empty. The
    /// nmea_seen arm exists for the fix banner: a receiver streaming RMC/GSV
    /// with GGA output disabled has perfectly good framing, and sending that
    /// user to debug baud rates is a false diagnosis. (The readout slots
    /// never reach it - slot_hint suppresses hints once NMEA is seen.)
    fn hint_text(&self) -> &'static str {
        if self.nmea_seen {
            "The receiver's NMEA is parsing, but no GGA sentence has \
             arrived - enable GGA output on the receiver to get fix data."
        } else if self.serial_connected {
            "The serial port is open but no NMEA has arrived yet - \
             check the baud rate and framing."
        } else {
            "This value comes from the GPS receiver's NMEA sentences on \
             the serial port, not from the caster stream. Connect a \
             receiver under Receiver (Serial) to populate it."
        }
    }
}

/// The caption (if any) under an empty receiver-derived slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlotHint {
    None,
    NeedsReceiver,
    WaitingForNmea,
}

/// Decide the hint for a slot showing `value`. Only the bare "-" of a
/// never-populated RECEIVER-derived readout hints; real values, the empty
/// Nothing slot, and the stream-side readouts (whose "-" means "no live
/// stream", which a receiver cannot fix) never do.
pub fn slot_hint(id: DisplayId, value: &str, r: ReceiverPresence) -> SlotHint {
    let stream_side = matches!(
        id,
        DisplayId::Nothing | DisplayId::DataAge | DisplayId::DataRate
    );
    if stream_side || value != "-" || r.nmea_seen {
        return SlotHint::None;
    }
    if r.serial_connected {
        SlotHint::WaitingForNmea
    } else {
        SlotHint::NeedsReceiver
    }
}

/// Stream-side readout inputs for the pickable Data Age / Data Rate slots:
/// None renders "-" - no live data-bearing connection (Data Rate) or no
/// bytes yet this session (Data Age).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamView {
    pub last_rx_age_s: Option<f32>,
    pub rate_kbps: Option<f64>,
}

/// Width of a readout slot: the picker and the value bed beneath it share it.
const SLOT_WIDTH: f32 = 150.0;

fn slot(
    ui: &mut egui::Ui,
    id: &str,
    choice: &mut DisplayId,
    g: &GnssState,
    stream: &StreamView,
    receiver: ReceiverPresence,
) {
    ui.vertical(|ui| {
        egui::ComboBox::from_id_salt(id)
            .width(SLOT_WIDTH)
            .selected_text(choice.label())
            .show_ui(ui, |ui| {
                for d in DisplayId::ALL {
                    ui.selectable_value(choice, *d, d.label());
                }
            });
        let value = slot_value(*choice, g, stream);
        let hint = slot_hint(*choice, &value, receiver);
        // The hint takes the value's own place, so it can only be read as
        // this slot's - a caption under it read as belonging to a neighbour.
        let text = match hint {
            SlotHint::None => egui::RichText::new(value).size(20.0).strong().monospace(),
            SlotHint::NeedsReceiver => hint_text("needs receiver"),
            SlotHint::WaitingForNmea => hint_text("waiting for NMEA"),
        };
        let resp = value_bed(ui, text);
        if hint != SlotHint::None {
            resp.on_hover_text(receiver.hint_text());
        }
    });
}

/// The live value in a fixed-width, right-aligned monospace bed. A support
/// tech watches this strip while values tick at 1-10 Hz: in a proportional,
/// left-aligned label "1615.876 ft" -> "1615.880 ft" changes width and shifts
/// every neighbour on the row. Monospace digits right-aligned in a bed as
/// wide as the picker keep the decimal point still and the row from
/// re-flowing (the 20 pt Plex Mono advance fits 12 characters - "29031.700 ft"
/// at Everest - inside SLOT_WIDTH).
fn hint_text(t: &str) -> egui::RichText {
    egui::RichText::new(t).color(theme::ink_secondary())
}

fn value_bed(ui: &mut egui::Ui, text: egui::RichText) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(SLOT_WIDTH, 0.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            // One 20 pt line tall either way, so a hint becoming a value
            // never moves the row.
            ui.set_min_size(egui::vec2(SLOT_WIDTH, 26.0));
            ui.label(text)
        },
    )
    .inner
}

/// Live value for a display id; "-" while its source (the receiver, or the
/// caster stream for the Data* ids) has not supplied it.
pub fn slot_value(id: DisplayId, g: &GnssState, s: &StreamView) -> String {
    use gnss::nmea::{knots_to_kmh, knots_to_mph, m_to_ft};
    let v = match id {
        DisplayId::Age => g.age_s.map(|a| format!("{a:.1} s")),
        DisplayId::Hdop => g.hdop.map(|v| format!("{v:.1}")),
        DisplayId::Vdop => g.vdop.map(|v| format!("{v:.1}")),
        DisplayId::Pdop => g.pdop.map(|v| format!("{v:.1}")),
        // Millimetre (thousandth-of-a-foot) resolution for survey work. f32
        // keeps ~7 significant digits, so a GGA height like 1615.876 m
        // survives parse and print intact (pinned by a test).
        DisplayId::ElevationFeet => g.alt_m.map(|m| format!("{:.3} ft", m_to_ft(m))),
        DisplayId::ElevationMeters => g.alt_m.map(|m| format!("{m:.3} m")),
        DisplayId::SpeedMph => g.speed_knots.map(|k| format!("{:.1} mph", knots_to_mph(k))),
        DisplayId::SpeedMphSmoothed => g.mph_smooth.get().map(|v| format!("{v:.1} mph")),
        DisplayId::SpeedKmh => g
            .speed_knots
            .map(|k| format!("{:.1} km/h", knots_to_kmh(k))),
        DisplayId::SpeedKmhSmoothed => g.kmh_smooth.get().map(|v| format!("{v:.1} km/h")),
        DisplayId::Heading => g.heading_deg.map(|h| format!("{h:.0} deg")),
        DisplayId::DataAge => s.last_rx_age_s.map(|a| format!("{a:.1} s")),
        DisplayId::DataRate => s.rate_kbps.map(|r| format!("{r:.1} kB/s")),
        DisplayId::Nothing => Some(String::new()),
    };
    v.unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parity-critical banner colors, pinned exactly: RTK Fixed is
    /// emerald, RTK Float signal orange, Invalid crimson, plain GPS/DGPS
    /// ink - and the placeholder before any GGA recedes to secondary ink
    /// (never muted: "No receiver" is a readout the user must be able to
    /// read, and secondary is the lightest AA-clearing ink) regardless of
    /// the stale quality byte.
    #[test]
    fn fix_quality_semantics() {
        assert_eq!(fix_quality_color(true, 4), theme::success()); // RTK Fixed
        assert_eq!(fix_quality_color(true, 5), theme::warning()); // RTK Float
        assert_eq!(fix_quality_color(true, 0), theme::danger()); // Invalid
        for gps_like in [1, 2, 9] {
            assert_eq!(fix_quality_color(true, gps_like), theme::ink_primary());
        }
        for exotic in [3, 6, 7, 8, 42] {
            assert_eq!(fix_quality_color(true, exotic), theme::ink_secondary());
        }
        // No GGA yet: readable placeholder even if quality still says fixed.
        assert_eq!(fix_quality_color(false, 4), theme::ink_secondary());
        assert_eq!(fix_quality_color(false, 0), theme::ink_secondary());
    }

    /// The banner state machine: the missing link named before any GGA, a
    /// live fix in its semantic ink, and a latched fix receding to secondary
    /// ink with a note once the port closes or the receiver goes quiet.
    #[test]
    fn fix_banner_states() {
        let t0 = Instant::now();
        let mut g = crate::state::AppState::new(t0).gnss;
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(fix_banner(&g, false, t0).text, "No receiver");
        assert_eq!(fix_banner(&g, true, t0).text, "Waiting for NMEA");
        g.nmea_seen = true;
        g.last_nmea = Some(t0);
        let no_gga = fix_banner(&g, true, at(1));
        assert_eq!(no_gga.text, "No GGA from receiver");
        assert_eq!(no_gga.ink, theme::ink_secondary());
        // A receiver that went quiet before any GGA is waited on again.
        assert_eq!(fix_banner(&g, true, at(10)).text, "Waiting for NMEA");

        g.has_fix_data = true;
        g.quality = 4;
        let live = fix_banner(&g, true, at(3));
        assert_eq!(
            live,
            FixBanner {
                text: "RTK Fixed",
                ink: theme::success(),
                note: None,
            }
        );
        let silent = fix_banner(&g, true, at(4));
        assert_eq!(silent.text, "RTK Fixed");
        assert_eq!(silent.ink, theme::ink_secondary(), "never green when stale");
        assert_eq!(silent.note.as_deref(), Some("(receiver silent)"));
        let unplugged = fix_banner(&g, false, at(1));
        assert_eq!(unplugged.ink, theme::ink_secondary());
        assert_eq!(unplugged.note.as_deref(), Some("(last seen just now)"));
        assert_eq!(
            fix_banner(&g, false, at(150)).note.as_deref(),
            Some("(last seen 2 min ago)")
        );
    }

    /// The envelope's exact rhythm: a burst snaps to 1.0 (caller side), one
    /// half-life halves it, silence never underflows, and an out-of-range
    /// input is clamped rather than trusted.
    #[test]
    fn activity_decay_half_life() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(close(decay_activity(1.0, ACTIVITY_HALF_LIFE), 0.5));
        assert!(close(decay_activity(1.0, 2 * ACTIVITY_HALF_LIFE), 0.25));
        assert!(close(decay_activity(0.5, ACTIVITY_HALF_LIFE), 0.25));
        // dt = 0 is the same frame twice: no change.
        assert!(close(decay_activity(0.7, Duration::ZERO), 0.7));
        // At the 1 Hz RTCM cadence the pulse stays clearly visible (~0.18).
        let between_bursts = decay_activity(1.0, Duration::from_secs(1));
        assert!((0.1..0.3).contains(&between_bursts), "{between_bursts}");
        // Long silence drains to (effectively) zero and never negative.
        let drained = decay_activity(1.0, Duration::from_secs(60));
        assert!((0.0..1e-6).contains(&drained));
        assert_eq!(decay_activity(5.0, Duration::ZERO), 1.0, "clamped");
        assert_eq!(decay_activity(-1.0, Duration::ZERO), 0.0, "clamped");
    }

    /// The full frame step the GUI's drain loop runs (previously inlined
    /// there, untested): decay happens first, a byte-total burst then snaps
    /// to full - never the other order - and a counter reset (new session)
    /// is not a burst.
    #[test]
    fn advance_activity_decays_then_snaps_on_growth() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        // No growth: pure decay.
        assert!(close(
            advance_activity(1.0, ACTIVITY_HALF_LIFE, 500, 500),
            0.5
        ));
        // Growth snaps to full AFTER the decay, so a burst on this very
        // frame renders at 1.0 no matter how faded the level was.
        assert_eq!(advance_activity(0.01, ACTIVITY_HALF_LIFE, 500, 900), 1.0);
        assert_eq!(advance_activity(0.0, Duration::from_secs(60), 0, 1), 1.0);
        // A total that went BACKWARD is a session reset, not a burst.
        assert!(close(
            advance_activity(1.0, ACTIVITY_HALF_LIFE, 10_000, 0),
            0.5
        ));
        // Same total twice on a zero-dt frame: unchanged.
        assert!(close(advance_activity(0.7, Duration::ZERO, 42, 42), 0.7));
    }

    /// The fix banner's hover diagnosis must track what the receiver is
    /// actually doing: an RMC/GSV-only receiver (GGA output disabled) has
    /// good framing, so the baud-rate hint would be a false trail.
    #[test]
    fn hint_text_distinguishes_no_gga_from_no_nmea() {
        let no_rx = ReceiverPresence {
            nmea_seen: false,
            serial_connected: false,
        };
        assert!(no_rx.hint_text().contains("Connect a receiver"));
        let port_open = ReceiverPresence {
            nmea_seen: false,
            serial_connected: true,
        };
        assert!(port_open.hint_text().contains("baud rate"));
        // NMEA parsing but no GGA yet: point at the receiver's GGA output,
        // never at baud/framing, whether or not the port is still open.
        for serial_connected in [true, false] {
            let talking = ReceiverPresence {
                nmea_seen: true,
                serial_connected,
            };
            let hint = talking.hint_text();
            assert!(hint.contains("enable GGA output"), "{hint}");
            assert!(!hint.contains("baud"), "{hint}");
        }
    }

    /// Status x silence -> cluster mode, the full truth table. The stall
    /// threshold is two missed 1 Hz epochs; disconnected states must map to
    /// Idle no matter how stale the silence clock is.
    #[test]
    fn stream_classification_truth_table() {
        let fresh = Duration::from_millis(500);
        let stale = Duration::from_secs(3);
        let rate = 0.7;
        assert_eq!(
            classify_stream(&NtripStatus::Streaming, fresh, rate),
            StreamActivity::Active { rate_kbps: rate }
        );
        assert_eq!(
            classify_stream(&NtripStatus::Streaming, stale, rate),
            StreamActivity::Stalled { silent_s: 3.0 }
        );
        // Exactly at the threshold is still healthy; stall needs > 2 s.
        assert_eq!(
            classify_stream(&NtripStatus::Streaming, STALL_AFTER, rate),
            StreamActivity::Active { rate_kbps: rate }
        );
        assert_eq!(
            classify_stream(&NtripStatus::WaitingForData, fresh, rate),
            StreamActivity::Waiting
        );
        assert_eq!(
            classify_stream(&NtripStatus::WaitingForData, stale, rate),
            StreamActivity::Stalled { silent_s: 3.0 }
        );
        // Connecting can take arbitrarily long without being a "stall".
        assert_eq!(
            classify_stream(&NtripStatus::Connecting { attempt: 3 }, stale, rate),
            StreamActivity::Waiting
        );
        for disconnected in [
            NtripStatus::Idle,
            NtripStatus::ReconnectWait { next_attempt: 2 },
            NtripStatus::Stopped {
                summary: "Disconnected".to_string(),
                failed: false,
            },
        ] {
            assert_eq!(
                classify_stream(&disconnected, stale, rate),
                StreamActivity::Idle
            );
        }
    }

    /// Mode -> token mapping, pinned like fix_quality_semantics: the fill is
    /// always emerald, the stalled well is the ONLY warning-colored outline,
    /// captions use readable inks only (never muted), and a disconnected bar
    /// paints no fill regardless of a stale envelope.
    #[test]
    fn cluster_tokens_and_fill() {
        let active = StreamActivity::Active { rate_kbps: 0.7 };
        let stalled = StreamActivity::Stalled { silent_s: 5.0 };
        // Opaque muted ink, not a translucent hairline: the empty idle well
        // must stay discoverable (3:1+ non-text boundary on every paper).
        assert_eq!(bar_stroke_color(&active), theme::ink_muted());
        assert_eq!(bar_stroke_color(&StreamActivity::Idle), theme::ink_muted());
        assert_eq!(
            bar_stroke_color(&StreamActivity::Waiting),
            theme::ink_muted()
        );
        assert_eq!(bar_stroke_color(&stalled), theme::warning());

        assert_eq!(
            cluster_caption(&active),
            ("0.7 kB/s".to_string(), theme::ink_primary())
        );
        assert_eq!(
            cluster_caption(&stalled),
            ("no data 5 s".to_string(), theme::warning())
        );
        assert_eq!(
            cluster_caption(&StreamActivity::Idle),
            ("-".to_string(), theme::ink_secondary())
        );
        assert_eq!(
            cluster_caption(&StreamActivity::Waiting),
            ("waiting...".to_string(), theme::ink_secondary())
        );

        assert_eq!(bar_fill_level(&StreamActivity::Idle, 0.9), 0.0);
        assert_eq!(bar_fill_level(&active, 0.9), 0.9);
        assert_eq!(bar_fill_level(&stalled, 0.1), 0.1);
        assert_eq!(bar_fill_level(&active, 7.0), 1.0, "clamped");
    }

    /// needs_receiver / waiting-for-NMEA truth table: only a bare "-" on a
    /// receiver-derived readout hints, and only until the first sentence.
    #[test]
    fn slot_hint_truth_table() {
        let no_rx = ReceiverPresence {
            nmea_seen: false,
            serial_connected: false,
        };
        let port_open = ReceiverPresence {
            nmea_seen: false,
            serial_connected: true,
        };
        let talking = ReceiverPresence {
            nmea_seen: true,
            serial_connected: true,
        };
        let unplugged = ReceiverPresence {
            nmea_seen: true,
            serial_connected: false,
        };
        assert_eq!(
            slot_hint(DisplayId::Age, "-", no_rx),
            SlotHint::NeedsReceiver
        );
        assert_eq!(
            slot_hint(DisplayId::Hdop, "-", port_open),
            SlotHint::WaitingForNmea
        );
        // Once NMEA has been seen, a missing field is normal, not a hint.
        assert_eq!(slot_hint(DisplayId::Age, "-", talking), SlotHint::None);
        assert_eq!(slot_hint(DisplayId::Age, "-", unplugged), SlotHint::None);
        // Populated values never hint.
        assert_eq!(slot_hint(DisplayId::Age, "1.0 s", no_rx), SlotHint::None);
        // The empty Nothing slot never hints.
        assert_eq!(slot_hint(DisplayId::Nothing, "", no_rx), SlotHint::None);
        assert_eq!(slot_hint(DisplayId::Nothing, "-", no_rx), SlotHint::None);
        // Stream-side readouts never hint: their "-" means "no live
        // stream", and connecting a receiver would not change that.
        assert_eq!(slot_hint(DisplayId::DataAge, "-", no_rx), SlotHint::None);
        assert_eq!(slot_hint(DisplayId::DataRate, "-", no_rx), SlotHint::None);
    }

    /// The pickable stream readouts: formatted from StreamView, "-" while
    /// the corresponding Option is None (disconnected / no data yet), and
    /// never touched by receiver state.
    #[test]
    fn stream_slot_values_format_from_stream_view() {
        let g = crate::state::AppState::new(Instant::now()).gnss;
        let live = StreamView {
            last_rx_age_s: Some(1.24),
            rate_kbps: Some(0.75),
        };
        assert_eq!(slot_value(DisplayId::DataAge, &g, &live), "1.2 s");
        assert_eq!(slot_value(DisplayId::DataRate, &g, &live), "0.8 kB/s");
        // Connected but no bytes yet: age honest-dashes, rate reads 0.
        let pre_data = StreamView {
            last_rx_age_s: None,
            rate_kbps: Some(0.0),
        };
        assert_eq!(slot_value(DisplayId::DataAge, &g, &pre_data), "-");
        assert_eq!(slot_value(DisplayId::DataRate, &g, &pre_data), "0.0 kB/s");
        let idle = StreamView {
            last_rx_age_s: None,
            rate_kbps: None,
        };
        assert_eq!(slot_value(DisplayId::DataAge, &g, &idle), "-");
        assert_eq!(slot_value(DisplayId::DataRate, &g, &idle), "-");
        // Receiver-derived ids ignore the stream view entirely.
        assert_eq!(slot_value(DisplayId::Age, &g, &live), "-");
    }

    /// Elevation reads to three decimals in both units - the user's own
    /// example, 1615.876 m - and stays an honest dash before any GGA.
    #[test]
    fn elevation_slots_read_three_decimals() {
        let idle = StreamView {
            last_rx_age_s: None,
            rate_kbps: None,
        };
        let mut g = crate::state::AppState::new(Instant::now()).gnss;
        assert_eq!(slot_value(DisplayId::ElevationMeters, &g, &idle), "-");
        assert_eq!(slot_value(DisplayId::ElevationFeet, &g, &idle), "-");
        g.alt_m = Some(1615.876);
        assert_eq!(
            slot_value(DisplayId::ElevationMeters, &g, &idle),
            "1615.876 m"
        );
        // 1615.876 m / 0.3048 = 5301.430 ft.
        assert_eq!(
            slot_value(DisplayId::ElevationFeet, &g, &idle),
            "5301.430 ft"
        );
        // End to end from the receiver's sentence: the f32 parse keeps the
        // third decimal (no type widening needed for a display change).
        let body = "GPGGA,123519,3500.000,N,10600.000,W,4,12,0.8,1615.876,M,-21.2,M,1.0,0000";
        let ck = body.bytes().fold(0u8, |a, b| a ^ b);
        let Ok(gnss::nmea::Sentence::Gga(gga)) = gnss::nmea::parse(&format!("${body}*{ck:02X}"))
        else {
            panic!("expected a GGA");
        };
        g.alt_m = gga.alt_m;
        assert_eq!(
            slot_value(DisplayId::ElevationMeters, &g, &idle),
            "1615.876 m"
        );
        // Below the ellipsoid/geoid is a real survey case, not an error.
        g.alt_m = Some(-12.5);
        assert_eq!(
            slot_value(DisplayId::ElevationMeters, &g, &idle),
            "-12.500 m"
        );
    }
}
