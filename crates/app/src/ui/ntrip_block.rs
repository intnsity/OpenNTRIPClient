//! The caster card (setup step 1): caster address and credentials, the
//! editable mountpoint combo fed by the loaded sourcetable, the protocol,
//! the one primary action, and status with reconnect attempt counter.

use std::sync::Arc;

use ntrip_core::sourcetable::SourceTable;

use crate::settings::{BottomTab, Profile, ProtocolCfg};
use crate::ui::{App, theme};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    theme::card(ui.style()).show(ui, |ui| {
        // Cards fill their column so the two steps read as one row.
        ui.set_width(ui.available_width());
        theme::section_heading(ui, "1", "Caster", false);
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
                        theme::number_field(
                            ui,
                            egui::DragValue::new(&mut p.port).range(1..=65535).speed(1),
                        );
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
                        ui.checkbox(&mut app.reveal_password, "show");
                    });
                    ui.end_row();

                    ui.label("Mountpoint");
                    // Raw TCP has no request to name a mount in, and no
                    // sourcetable to pick one from.
                    let raw = app.settings.active().protocol == ProtocolCfg::Tcp;
                    ui.add_enabled_ui(!raw, |ui| {
                        ui.horizontal(|ui| mountpoint_combo(app, ui));
                    })
                    .response
                    .on_disabled_hover_text("Not used with Raw TCP");
                    ui.end_row();

                    ui.label("Protocol");
                    ui.horizontal(|ui| {
                        let p = app.settings.active_mut();
                        let mut choice = Protocol::of(p);
                        for c in Protocol::ALL {
                            // Write back only on a click, so rendering never
                            // touches the profile (and never lights Save).
                            if ui.radio_value(&mut choice, c, c.label()).changed() {
                                choice.apply(p);
                            }
                        }
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
        if busy {
            ui.label(theme::caption("Fields are locked while connected."));
        }

        ui.add_space(4.0);
        let host_ok = !app.settings.active().host.trim().is_empty();
        ui.horizontal(|ui| {
            let label = {
                let p = app.settings.active();
                primary_label(busy, p.protocol, &p.mountpoint)
            };
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
            let raw = app.settings.active().protocol == ProtocolCfg::Tcp;
            if ui
                .add_enabled(!raw, egui::Button::new("Browse mounts"))
                .on_hover_text("Open the Sourcetable tab (filter, sort, pick a mountpoint)")
                .on_disabled_hover_text("Raw TCP has no sourcetable")
                .clicked()
            {
                // Land on a populated tab: fetch the list first unless one is
                // already loaded for this caster or a session holds the line.
                if !busy && host_ok && loaded_table(app).is_none() {
                    app.connect_ntrip(true);
                }
                app.settings.window.tab = BottomTab::Sourcetable;
            }
        });
        // A disabled primary must say why in plain sight, not only on hover.
        if !busy && !host_ok {
            ui.label(theme::caption("Enter a caster host"));
        }
    });

    // GGA reporting used to hide behind a "Details..." window; One Surface
    // brings it onto the front page as a disclosure directly under the caster
    // card - the caster demanding NMEA and the control that answers it now
    // share a column.
    ui.add_space(4.0);
    super::gga_section::show(app, ui);
}

/// The one protocol choice the user sees, over the two profile fields the
/// worker reads. Raw TCP has no NTRIP version, so choosing it leaves
/// `ntrip_version` alone and switching back restores the user's version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Ntrip1,
    Ntrip2,
    RawTcp,
}

impl Protocol {
    const ALL: [Protocol; 3] = [Protocol::Ntrip1, Protocol::Ntrip2, Protocol::RawTcp];

    fn of(p: &Profile) -> Protocol {
        match (p.protocol, p.ntrip_version) {
            (ProtocolCfg::Tcp, _) => Protocol::RawTcp,
            (ProtocolCfg::Ntrip, 2) => Protocol::Ntrip2,
            (ProtocolCfg::Ntrip, _) => Protocol::Ntrip1,
        }
    }

    fn apply(self, p: &mut Profile) {
        match self {
            Protocol::Ntrip1 => (p.protocol, p.ntrip_version) = (ProtocolCfg::Ntrip, 1),
            Protocol::Ntrip2 => (p.protocol, p.ntrip_version) = (ProtocolCfg::Ntrip, 2),
            Protocol::RawTcp => p.protocol = ProtocolCfg::Tcp,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Protocol::Ntrip1 => "NTRIP 1.0",
            Protocol::Ntrip2 => "NTRIP 2.0",
            Protocol::RawTcp => "Raw TCP",
        }
    }
}

/// The primary button names what a click will actually do: an NTRIP request
/// with no mountpoint IS a sourcetable fetch (see `App::connect_ntrip`).
fn primary_label(busy: bool, protocol: ProtocolCfg, mountpoint: &str) -> &'static str {
    if busy {
        "Disconnect"
    } else if protocol == ProtocolCfg::Ntrip && mountpoint.trim().is_empty() {
        "Get sourcetable"
    } else {
        "Connect"
    }
}

/// The sourcetable in memory, if it belongs to the active profile's caster.
fn loaded_table(app: &App) -> Option<Arc<SourceTable>> {
    let p = app.settings.active();
    let host = p.host.trim();
    app.state
        .ntrip
        .sourcetable
        .as_ref()
        .filter(|(h, po, _)| h == host && *po == p.port)
        .map(|(_, _, t)| t.clone())
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
/// of the loaded sourcetable's streams. Hand-rolled popup so the text stays
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
    let side = edit.rect.height();
    let arrow = ui
        .add(egui::Button::new("").min_size(egui::vec2(side, side)))
        .on_hover_text("Pick from the sourcetable");
    // The combo-box glyph, painted in the button's own state ink so it
    // follows hover and disabled like any label would.
    let tri = egui::Rect::from_center_size(arrow.rect.center(), egui::vec2(8.0, 5.0));
    ui.painter().add(egui::Shape::convex_polygon(
        vec![tri.left_top(), tri.right_top(), tri.center_bottom()],
        ui.style().interact(&arrow).fg_stroke.color,
        egui::Stroke::NONE,
    ));
    if arrow.clicked() {
        app.mount_popup_open = !was_open;
    }
    if !app.mount_popup_open {
        return;
    }

    let table = loaded_table(app);
    let can_fetch = !app.ntrip_busy() && !app.settings.active().host.trim().is_empty();
    let mut picked: Option<String> = None;
    let mut fetch = false;
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
                        ui.label("No sourcetable loaded for this caster.");
                        fetch = ui
                            .add_enabled(can_fetch, egui::Button::new("Get sourcetable"))
                            .clicked();
                    }
                }
            });
        });
    if let Some(mount) = picked {
        app.settings.active_mut().mountpoint = mount;
        app.mount_popup_open = false;
    } else if fetch {
        app.connect_ntrip(true);
        app.mount_popup_open = false;
    } else if was_open && area.response.clicked_elsewhere() {
        app.mount_popup_open = false;
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

    /// The single Protocol group round-trips onto the two profile fields,
    /// and Raw TCP keeps the NTRIP version for the trip back.
    #[test]
    fn protocol_choice_maps_onto_profile_fields() {
        let mut p = Profile::default();
        for c in Protocol::ALL {
            c.apply(&mut p);
            assert_eq!(Protocol::of(&p), c);
        }
        Protocol::Ntrip2.apply(&mut p);
        Protocol::RawTcp.apply(&mut p);
        assert_eq!((p.protocol, p.ntrip_version), (ProtocolCfg::Tcp, 2));
        Protocol::Ntrip1.apply(&mut p);
        assert_eq!((p.protocol, p.ntrip_version), (ProtocolCfg::Ntrip, 1));
    }

    /// The primary label must name what connect_ntrip will do.
    #[test]
    fn primary_label_is_state_honest() {
        assert_eq!(primary_label(true, ProtocolCfg::Ntrip, ""), "Disconnect");
        assert_eq!(
            primary_label(false, ProtocolCfg::Ntrip, "  "),
            "Get sourcetable"
        );
        assert_eq!(primary_label(false, ProtocolCfg::Ntrip, "RTCM3"), "Connect");
        assert_eq!(primary_label(false, ProtocolCfg::Tcp, ""), "Connect");
    }
}
