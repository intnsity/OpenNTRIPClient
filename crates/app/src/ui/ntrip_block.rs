//! "NTRIP Caster" group: caster address and credentials, the editable
//! mountpoint combo fed by the cached sourcetable, protocol selectors,
//! connect/disconnect, status with reconnect attempt counter.

use crate::bus::NtripStatus;
use crate::settings::{Profile, ProtocolCfg};
use crate::ui::{App, theme};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    theme::card(ui.style()).show(ui, |ui| {
        ui.set_min_height(150.0);
        ui.strong("NTRIP Caster");
        ui.add_space(2.0);

        let busy = app.ntrip_busy();
        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("ntrip-grid")
                .num_columns(2)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    let p = app.settings.active_mut();

                    ui.label("Host");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut p.host)
                                .desired_width(150.0)
                                .hint_text("caster.example.com"),
                        );
                        ui.label("Port");
                        ui.add(egui::DragValue::new(&mut p.port).range(1..=65535).speed(1));
                        tls_checkbox(p, ui);
                    });
                    ui.end_row();

                    ui.label("User");
                    ui.add(egui::TextEdit::singleline(&mut p.username).desired_width(150.0));
                    ui.end_row();

                    ui.label("Password");
                    ui.horizontal(|ui| {
                        let reveal = app.reveal_password;
                        let p = app.settings.active_mut();
                        ui.add(
                            egui::TextEdit::singleline(&mut p.password)
                                .desired_width(150.0)
                                .password(!reveal),
                        );
                        ui.toggle_value(&mut app.reveal_password, "Show")
                            .on_hover_text("Reveal the password while held on");
                    });
                    ui.end_row();

                    ui.label("Mount");
                    ui.horizontal(|ui| {
                        mountpoint_combo(app, ui);
                    });
                    ui.end_row();

                    ui.label("Version");
                    ui.horizontal(|ui| {
                        let p = app.settings.active_mut();
                        ui.radio_value(&mut p.ntrip_version, 1u8, "v1");
                        ui.radio_value(&mut p.ntrip_version, 2u8, "v2");
                        ui.separator();
                        ui.radio_value(&mut p.protocol, ProtocolCfg::Ntrip, "NTRIP");
                        ui.radio_value(&mut p.protocol, ProtocolCfg::Tcp, "raw TCP");
                    });
                    ui.end_row();

                    // The diagnostic override only surfaces while TLS is on:
                    // it is meaningless otherwise, and hiding it keeps the
                    // dangerous control out of casual reach.
                    let p = app.settings.active_mut();
                    if p.tls {
                        ui.label("TLS");
                        insecure_certs_checkbox(p, ui);
                        ui.end_row();
                    } else {
                        // A profile cannot keep the override armed with TLS
                        // off; a stale true would silently spring back to
                        // life the next time TLS is enabled.
                        p.allow_invalid_certs = false;
                    }
                });
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let label = if busy { "Disconnect" } else { "Connect" };
            let host_ok = !app.settings.active().host.trim().is_empty();
            if ui
                .add_enabled(busy || host_ok, theme::accent_button(label))
                .clicked()
            {
                if busy {
                    // Stamp the click itself: the worker's close line says
                    // "Disconnected by user", and this line puts the request
                    // before it in the log so support reads cause -> effect.
                    app.hub.event("Disconnect requested");
                    app.disconnect_ntrip();
                } else {
                    app.connect_ntrip(false);
                }
            }
            if ui
                .add_enabled(!busy && host_ok, egui::Button::new("Get Sourcetable"))
                .on_hover_text("Download the caster's stream list")
                .clicked()
            {
                app.connect_ntrip(true);
            }
            if ui
                .button("Browse...")
                .on_hover_text("Open the Sourcetable tab (filter, sort, pick a mount)")
                .clicked()
            {
                app.settings.window.tab = crate::settings::BottomTab::Sourcetable;
            }
        });
        status_line(app, ui);
    });

    // GGA reporting used to hide behind a "Details..." window; One Surface
    // brings it onto the front page as a disclosure directly under the caster
    // card - the caster demanding NMEA and the control that answers it now
    // share a column.
    ui.add_space(4.0);
    super::gga_section::show(app, ui);
}

/// Live TLS toggle, bound straight to the profile the worker reads. Split
/// out (with the insecure-certs control) so the bindings stay unit-testable
/// without constructing an App.
fn tls_checkbox(p: &mut Profile, ui: &mut egui::Ui) -> egui::Response {
    ui.checkbox(&mut p.tls, "TLS").on_hover_text(
        "Encrypt the connection with TLS. Certificates verify against the \
         bundled webpki root store.",
    )
}

/// The loud diagnostic override for self-signed and bare-IP casters. Only
/// rendered while TLS is on; a persistent red banner shows whenever a
/// connection actually runs with verification disabled.
fn insecure_certs_checkbox(p: &mut Profile, ui: &mut egui::Ui) -> egui::Response {
    ui.checkbox(
        &mut p.allow_invalid_certs,
        "Accept invalid certificates (diagnostic)",
    )
    .on_hover_text(
        "DANGER: disables certificate verification for this profile - any \
         server can impersonate the caster. For reaching casters with \
         self-signed or bare-IP certificates only. A red warning banner \
         shows while connected this way.",
    )
}

/// The mountpoint control the original made famous: free text AND a dropdown
/// of the cached sourcetable's streams. Hand-rolled popup so the text stays
/// editable (egui's ComboBox is selection-only).
fn mountpoint_combo(app: &mut App, ui: &mut egui::Ui) {
    let edit = {
        let p = app.settings.active_mut();
        ui.add(
            egui::TextEdit::singleline(&mut p.mountpoint)
                .desired_width(150.0)
                .hint_text("empty = sourcetable"),
        )
    };
    let was_open = app.mount_popup_open;
    if ui
        .small_button("v")
        .on_hover_text("Pick from the cached sourcetable")
        .clicked()
    {
        app.mount_popup_open = !was_open;
    }
    if !app.mount_popup_open {
        return;
    }

    let (profile_host, profile_port) = {
        let p = app.settings.active();
        (p.host.trim().to_string(), p.port)
    };
    let table = app
        .state
        .ntrip
        .sourcetable
        .as_ref()
        .filter(|(h, po, _)| *h == profile_host && *po == profile_port)
        .map(|(_, _, t)| t.clone());

    let mut picked: Option<String> = None;
    let area = egui::Area::new(egui::Id::new("mount-popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(edit.rect.left_bottom() + egui::vec2(0.0, 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                match &table {
                    Some(t) if !t.strs.is_empty() => {
                        egui::ScrollArea::vertical()
                            .max_height(220.0)
                            .show(ui, |ui| {
                                for s in &t.strs {
                                    let text = if s.format.is_empty() {
                                        s.mountpoint.clone()
                                    } else {
                                        format!(
                                            "{}  ({}, {})",
                                            s.mountpoint, s.format, s.identifier
                                        )
                                    };
                                    if ui.selectable_label(false, text).clicked() {
                                        picked = Some(s.mountpoint.clone());
                                    }
                                }
                            });
                    }
                    _ => {
                        ui.label("No sourcetable cached for this caster.");
                        ui.label("Use [Get Sourcetable] first.");
                    }
                }
            });
        });
    if let Some(mount) = picked {
        app.settings.active_mut().mountpoint = mount;
        app.mount_popup_open = false;
    } else if was_open && area.response.clicked_elsewhere() {
        app.mount_popup_open = false;
    }
}

fn status_line(app: &App, ui: &mut egui::Ui) {
    let now = std::time::Instant::now();
    // The stall clock only matters while Streaming: a green "Streaming"
    // sitting next to the activity cluster's orange "no data N s" would let
    // the two adjacent indicators contradict each other for the whole
    // pre-kick starvation window.
    let stalled_for = app
        .state
        .ntrip
        .stalled(now)
        .then(|| app.state.ntrip.rx_age(now).unwrap_or_default());
    let (text, color) = status_text(
        &app.state.ntrip.status,
        stalled_for,
        app.settings.app.max_reconnect_attempts,
    );
    ui.label(egui::RichText::new(text).color(color).small());
}

/// Status text + ink, pure for testing. `stalled_for` is Some(silence age)
/// when a live connection has gone quiet past the stall threshold.
fn status_text(
    status: &NtripStatus,
    stalled_for: Option<std::time::Duration>,
    max: u32,
) -> (String, egui::Color32) {
    match status {
        NtripStatus::Idle => ("Not connected".to_string(), theme::INK_SECONDARY),
        NtripStatus::Connecting { attempt } => (
            format!("Connecting (attempt {attempt})..."),
            theme::INK_PRIMARY,
        ),
        NtripStatus::WaitingForData => (
            "Connected - waiting for data...".to_string(),
            theme::INK_PRIMARY,
        ),
        NtripStatus::Streaming => match stalled_for {
            // Success green must not outlive the data.
            Some(age) => (
                format!("Streaming - no data for {:.0} s", age.as_secs_f32()),
                theme::WARNING,
            ),
            None => ("Streaming".to_string(), theme::SUCCESS),
        },
        NtripStatus::ReconnectWait { next_attempt } => (
            format!("Reconnecting in 10 s (attempt {next_attempt} of {max})"),
            theme::WARNING,
        ),
        NtripStatus::Stopped { summary, failed } => (
            summary.clone(),
            if *failed {
                theme::DANGER
            } else {
                theme::INK_SECONDARY
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one headless egui frame rendering `render`, feeding pointer
    /// `events`, and return the rendered widget's rect.
    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut render: impl FnMut(&mut egui::Ui) -> egui::Response,
    ) -> egui::Rect {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run_ui(input, |ui| {
            rect = render(ui).rect;
        });
        rect
    }

    /// Click sequence at `pos`: move, press, release across frames the way a
    /// real pointer arrives.
    fn press(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn release(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }]
    }

    /// The status line and the activity cluster must agree during a stall:
    /// "Streaming" stays green only while data is actually arriving, and
    /// flips to the warning wording/ink for the silent pre-kick window.
    #[test]
    fn status_text_streaming_reflects_stall() {
        use std::time::Duration;
        let (text, color) = status_text(&NtripStatus::Streaming, None, 10_000);
        assert_eq!(text, "Streaming");
        assert_eq!(color, theme::SUCCESS);
        let (text, color) = status_text(
            &NtripStatus::Streaming,
            Some(Duration::from_secs(10)),
            10_000,
        );
        assert_eq!(text, "Streaming - no data for 10 s");
        assert_eq!(color, theme::WARNING);
        // Non-streaming states ignore the stall clock entirely.
        let (text, color) = status_text(&NtripStatus::Idle, Some(Duration::from_secs(10)), 10_000);
        assert_eq!(text, "Not connected");
        assert_eq!(color, theme::INK_SECONDARY);
    }

    /// M3 regression: the TLS checkbox was shipped permanently disabled with
    /// "lands in the next milestone" hover text. It must be a live control
    /// that writes the profile field the worker reads.
    #[test]
    fn tls_checkbox_is_live_and_toggles_the_profile() {
        let ctx = egui::Context::default();
        let mut p = Profile::default();
        assert!(!p.tls);
        let rect = frame(&ctx, Vec::new(), |ui| tls_checkbox(&mut p, ui));
        let center = rect.center();
        frame(&ctx, press(center), |ui| tls_checkbox(&mut p, ui));
        frame(&ctx, release(center), |ui| tls_checkbox(&mut p, ui));
        assert!(p.tls, "clicking the TLS checkbox must enable TLS");
    }

    /// The insecure override is reachable from the GUI (previously only via
    /// hand-editing settings.toml) and bound to allow_invalid_certs.
    #[test]
    fn insecure_certs_checkbox_toggles_the_override() {
        let ctx = egui::Context::default();
        let mut p = Profile {
            tls: true,
            ..Profile::default()
        };
        assert!(!p.allow_invalid_certs);
        let rect = frame(&ctx, Vec::new(), |ui| insecure_certs_checkbox(&mut p, ui));
        let center = rect.center();
        frame(&ctx, press(center), |ui| {
            insecure_certs_checkbox(&mut p, ui)
        });
        frame(&ctx, release(center), |ui| {
            insecure_certs_checkbox(&mut p, ui)
        });
        assert!(
            p.allow_invalid_certs,
            "clicking the override must arm allow_invalid_certs"
        );
    }
}
