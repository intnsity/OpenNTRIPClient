//! The session band: the one place that answers "is it working?". It sits
//! directly under the caster card - the Connect the user just pressed - and
//! gathers what used to be scattered over five spots: the link state, the
//! activity bar with rate and session total, the stream summary, and the
//! next step to take.
//!
//! Two lines, fixed height, so connecting never shifts the surface:
//! 1. a state word in its semantic color, the target, and the activity
//!    cluster with the session total;
//! 2. what is in the stream once frames arrive (clickable to Stream detail),
//!    otherwise the next step or the one-sentence hint for the failure seen.
//!
//! A colored rule down the band's left edge repeats the state at a glance.

use std::time::{Duration, Instant};

use crate::bus::NtripStatus;
use crate::settings::BottomTab;
use crate::ui::{App, theme};

/// The band's state class; each maps to one token in `tone_color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Idle,
    Busy,
    Good,
    Warn,
    Bad,
}

fn tone_color(t: Tone) -> egui::Color32 {
    match t {
        Tone::Idle => theme::ink_secondary(),
        Tone::Busy => theme::accent(),
        Tone::Good => theme::success(),
        Tone::Warn => theme::warning(),
        Tone::Bad => theme::danger(),
    }
}

/// The state word for line 1, pure for tests. `stalled_for` is Some only
/// while a data-bearing connection has gone quiet past the stall threshold;
/// `not_rtcm` means bytes flow but none frame as RTCM3 - alive, yet green
/// would be a lie (an RT27 or CMR port looks exactly like this).
pub fn state_word(
    status: &NtripStatus,
    stalled_for: Option<Duration>,
    not_rtcm: bool,
) -> (String, Tone) {
    match status {
        NtripStatus::Idle => ("NOT CONNECTED".into(), Tone::Idle),
        NtripStatus::Connecting { attempt } if *attempt > 1 => {
            (format!("CONNECTING - attempt {attempt}"), Tone::Busy)
        }
        NtripStatus::Connecting { .. } => ("CONNECTING".into(), Tone::Busy),
        NtripStatus::WaitingForData | NtripStatus::Streaming if stalled_for.is_some() => {
            let s = stalled_for.unwrap_or_default().as_secs();
            (format!("NO DATA FOR {s} s"), Tone::Warn)
        }
        NtripStatus::WaitingForData => ("CONNECTED - WAITING FOR DATA".into(), Tone::Busy),
        NtripStatus::Streaming if not_rtcm => ("RECEIVING - NOT RTCM3".into(), Tone::Warn),
        NtripStatus::Streaming => ("STREAMING".into(), Tone::Good),
        NtripStatus::ReconnectWait { next_attempt } => {
            (format!("RECONNECTING - attempt {next_attempt}"), Tone::Warn)
        }
        NtripStatus::Stopped { failed: true, .. } => ("FAILED".into(), Tone::Bad),
        NtripStatus::Stopped { failed: false, .. } => ("DISCONNECTED".into(), Tone::Idle),
    }
}

/// Line 2 when there is no stream content to summarize: the next step for
/// an idle surface, or the explanation of what just happened. `hint` is the
/// GGA/geography hint for a stall or refusal, which outranks the generic
/// text because it names the likely fix.
pub fn next_step<'a>(
    status: &'a NtripStatus,
    stalled: bool,
    not_rtcm: bool,
    host_set: bool,
    hint: Option<&'a str>,
    last_close: Option<&'a str>,
) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    // A failure's own reason is the fact; a hint is a guess about it, so
    // the hint joins the reason rather than replacing it.
    if let (
        NtripStatus::Stopped {
            summary,
            failed: true,
        },
        Some(h),
    ) = (status, hint)
    {
        return Cow::Owned(format!("{summary} - {h}"));
    }
    if let Some(h) = hint {
        return Cow::Borrowed(h);
    }
    match status {
        NtripStatus::Idle if !host_set => {
            Cow::Borrowed("Enter a caster host above, or pick a profile.")
        }
        NtripStatus::Idle => Cow::Borrowed("Press Connect to start."),
        NtripStatus::Connecting { .. } => {
            Cow::Borrowed("Request sent - the exchange is in the Connection log.")
        }
        NtripStatus::WaitingForData | NtripStatus::Streaming if stalled => {
            Cow::Borrowed("The caster accepted the connection but is sending nothing.")
        }
        NtripStatus::WaitingForData => Cow::Borrowed("Waiting for the first correction bytes."),
        NtripStatus::Streaming if not_rtcm => {
            Cow::Borrowed("Data is arriving but it is not RTCM3 - see Stream detail.")
        }
        NtripStatus::Streaming => Cow::Borrowed("Bytes are arriving; decoding the first frames."),
        // The reason is the verdict; sending the tech to another tab for a
        // reason the app already holds would break "one verdict, one place".
        NtripStatus::ReconnectWait { .. } => match last_close {
            Some(why) => Cow::Owned(format!("{why} - retrying in 10 s")),
            None => Cow::Borrowed("Retrying in 10 s."),
        },
        NtripStatus::Stopped { summary, .. } => Cow::Borrowed(summary.as_str()),
    }
}

/// Line 2 after a sourcetable fetch.
fn table_line(streams: usize) -> String {
    match streams {
        0 => "The caster lists no streams.".to_string(),
        1 => "1 stream - double-click it in the Sourcetable tab to use it.".to_string(),
        k => format!("{k} streams - double-click one in the Sourcetable tab to use it."),
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let now = Instant::now();
    let n = &app.state.ntrip;
    let stalled_for = n.stalled(now).then(|| n.rx_age(now).unwrap_or_default());
    // A couple of kilobytes with no frame among them is a verdict, not a
    // warm-up: RTCM3 frames are at most ~1 kB.
    let not_rtcm = n.rtcm.is_empty() && n.rtcm_garbage_bytes > 2048;
    let p = app.settings.active();
    // A finished sourcetable fetch is a result, not a disconnection: name
    // what was fetched and the next step.
    let table_streams = match (&n.status, &n.sourcetable) {
        (NtripStatus::Stopped { failed: false, .. }, Some((h, port, t)))
            if n.rtcm.is_empty() && h == p.host.trim() && *port == p.port =>
        {
            Some(t.strs.len())
        }
        _ => None,
    };
    let (word, tone) = match table_streams {
        Some(_) => ("SOURCETABLE".to_string(), Tone::Good),
        None => state_word(&n.status, stalled_for, not_rtcm),
    };
    let ink = tone_color(tone);
    // The same "where it points" the Profiles list shows; nothing at all
    // until there is a host.
    let target = if p.host.trim().is_empty() {
        String::new()
    } else {
        p.summary()
    };
    let host_set = !p.host.trim().is_empty();
    let silent = n.rx_age(now).unwrap_or(Duration::ZERO);
    let mode = super::status_strip::classify_stream(&n.status, silent, app.rate_kbps);
    let total = super::plot_panel::fmt_total(n.total_bytes);
    // The hint belongs to a session: live, or the one that just failed -
    // never to a user's own Disconnect.
    let hint = match &n.status {
        NtripStatus::Idle | NtripStatus::Stopped { failed: false, .. } => None,
        _ => n.position_hint(),
    };

    let inner = theme::card(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(word).strong().color(ink));
            if !target.is_empty() {
                ui.label(
                    egui::RichText::new(&target)
                        .monospace()
                        .color(theme::ink_secondary()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                super::status_strip::activity_cluster(ui, &mode, app.activity);
                ui.label(
                    egui::RichText::new(total)
                        .small()
                        .monospace()
                        .color(theme::ink_secondary()),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            // The summary owns line 2 while frames are flowing; otherwise
            // the next step does, so the line is never blank.
            if !super::stream_summary::line(app, ui) {
                let status = &app.state.ntrip.status;
                let text = match table_streams {
                    Some(k) => std::borrow::Cow::Owned(table_line(k)),
                    None => next_step(
                        status,
                        stalled_for.is_some(),
                        not_rtcm,
                        host_set,
                        hint,
                        app.state.ntrip.last_close_reason(),
                    ),
                };
                let color = if hint.is_some() || not_rtcm {
                    theme::warning()
                } else {
                    theme::ink_secondary()
                };
                let resp = ui.label(egui::RichText::new(text.as_ref()).small().color(color));
                if hint.is_some() {
                    // The hint names the fix; one click puts the user there.
                    let resp = resp
                        .interact(egui::Sense::click())
                        .on_hover_text("Open GGA position reporting");
                    if resp.clicked() {
                        app.settings.window.gga_open = true;
                    }
                } else if matches!(status, NtripStatus::Connecting { .. }) {
                    let resp = resp
                        .interact(egui::Sense::click())
                        .on_hover_text("Open the Connection log");
                    if resp.clicked() {
                        app.settings.window.tab = BottomTab::Conn;
                    }
                }
            }
        });
    });
    // The state rule: a thick left edge in the state's color, painted over
    // the card's border so it reads as one shape.
    let r = inner.response.rect;
    let rule = egui::Rect::from_min_size(r.min, egui::vec2(4.0, r.height()));
    ui.painter().rect_filled(
        rule,
        egui::CornerRadius {
            nw: 8,
            sw: 8,
            ne: 0,
            se: 0,
        },
        ink,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_words_cover_every_status() {
        assert_eq!(
            state_word(&NtripStatus::Idle, None, false),
            ("NOT CONNECTED".into(), Tone::Idle)
        );
        assert_eq!(
            state_word(&NtripStatus::Connecting { attempt: 1 }, None, false),
            ("CONNECTING".into(), Tone::Busy)
        );
        assert_eq!(
            state_word(&NtripStatus::Connecting { attempt: 3 }, None, false),
            ("CONNECTING - attempt 3".into(), Tone::Busy)
        );
        assert_eq!(
            state_word(&NtripStatus::Streaming, None, false),
            ("STREAMING".into(), Tone::Good)
        );
        // Green must not outlive the data, in either data-bearing state.
        for s in [NtripStatus::Streaming, NtripStatus::WaitingForData] {
            assert_eq!(
                state_word(&s, Some(Duration::from_secs(8)), false),
                ("NO DATA FOR 8 s".into(), Tone::Warn)
            );
        }
        assert_eq!(
            state_word(&NtripStatus::ReconnectWait { next_attempt: 2 }, None, false,).1,
            Tone::Warn
        );
        let failed = NtripStatus::Stopped {
            summary: "401 Unauthorized".into(),
            failed: true,
        };
        assert_eq!(
            state_word(&failed, None, false),
            ("FAILED".into(), Tone::Bad)
        );
        let stopped = NtripStatus::Stopped {
            summary: "Disconnected".into(),
            failed: false,
        };
        assert_eq!(state_word(&stopped, None, false).1, Tone::Idle);
    }

    #[test]
    fn reconnecting_states_its_reason_in_the_band() {
        let wait = NtripStatus::ReconnectWait { next_attempt: 2 };
        let why = "resolve nohost.invalid:2013: No such host is known";
        assert_eq!(
            next_step(&wait, false, false, true, None, Some(why)),
            format!("{why} - retrying in 10 s")
        );
        assert_eq!(
            next_step(&wait, false, false, true, None, None),
            "Retrying in 10 s."
        );
    }

    #[test]
    fn a_fetched_sourcetable_names_its_next_step() {
        assert!(table_line(765).starts_with("765 streams - double-click one"));
        assert!(table_line(1).starts_with("1 stream - "));
        assert_eq!(table_line(0), "The caster lists no streams.");
    }

    #[test]
    fn a_live_stream_that_is_not_rtcm_is_not_green() {
        let (word, tone) = state_word(&NtripStatus::Streaming, None, true);
        assert_eq!((word.as_str(), tone), ("RECEIVING - NOT RTCM3", Tone::Warn));
        assert!(
            next_step(&NtripStatus::Streaming, false, true, true, None, None).contains("not RTCM3")
        );
    }

    #[test]
    fn next_step_teaches_and_hints_outrank_it() {
        let idle = NtripStatus::Idle;
        assert!(next_step(&idle, false, false, false, None, None).contains("Enter a caster host"));
        assert!(next_step(&idle, false, false, true, None, None).contains("Press Connect"));
        assert!(
            next_step(&NtripStatus::Streaming, true, false, true, None, None)
                .contains("sending nothing")
        );
        let h = "Many casters wait for your position.";
        assert_eq!(
            next_step(&NtripStatus::Streaming, true, false, true, Some(h), None),
            h
        );
        let stopped = NtripStatus::Stopped {
            summary: "Caster closed the connection".into(),
            failed: true,
        };
        assert_eq!(
            next_step(&stopped, false, false, true, None, None),
            "Caster closed the connection"
        );
        // With a hint, the reason stays visible ahead of it.
        assert_eq!(
            next_step(&stopped, false, false, true, Some(h), None),
            format!("Caster closed the connection - {h}")
        );
    }
}
