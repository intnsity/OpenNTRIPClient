//! About dialog: version, license, heritage, data attribution, and a
//! summary of every third-party license the shipped binary embeds.

use crate::ui::{App, theme};
use crate::{RELEASES_URL, REPO_URL};

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.show_about {
        return;
    }
    let mut open = app.show_about;
    egui::Window::new("About")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(400.0)
        .show(ctx, |ui| {
            ui.heading(format!("Open NTRIP Client v{}", env!("CARGO_PKG_VERSION")));
            ui.label(
                egui::RichText::new("Diagnostic client for NTRIP/RTK correction streams")
                    .color(theme::INK_SECONDARY),
            );
            ui.add_space(8.0);

            ui.label(
                "This program is free software, licensed under the GNU General \
Public License, version 3 or (at your option) any later version. It comes \
with ABSOLUTELY NO WARRANTY.",
            );
            ui.add_space(8.0);

            ui.label(
                "A clean-room, GPL-licensed successor to the Lefebure NTRIP \
Client by Lance Lefebure (lefebure.com), whose free tool kept RTK fieldwork \
running for two decades. This project reimplements its behavior from \
observation and documentation; it contains none of the original code.",
            );
            ui.add_space(8.0);

            ui.label(
                egui::RichText::new(
                    "Offline location data: city database (c) GeoNames, \
CC BY 4.0; ZIP centroids from the US Census Bureau (public domain).",
                )
                .small()
                .color(theme::INK_SECONDARY),
            );
            ui.add_space(8.0);

            link(app, ui, "Source code and issues", REPO_URL);
            if ui
                .link("Releases / check for updates")
                .on_hover_text(RELEASES_URL)
                .clicked()
            {
                app.check_updates_now();
            }
            ui.add_space(8.0);

            egui::CollapsingHeader::new("Third-party licenses")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "The binary statically embeds these open-source \
components. Full license texts ship in the repository.",
                        )
                        .small()
                        .color(theme::INK_SECONDARY),
                    );
                    ui.add_space(4.0);
                    egui::Grid::new("third-party-licenses")
                        .num_columns(2)
                        .spacing([12.0, 2.0])
                        .show(ui, |ui| {
                            for (component, license) in THIRD_PARTY {
                                ui.label(egui::RichText::new(*component).small());
                                ui.label(
                                    egui::RichText::new(*license)
                                        .small()
                                        .color(theme::INK_SECONDARY),
                                );
                                ui.end_row();
                            }
                        });
                });
        });
    app.show_about = open;
}

/// Direct dependencies of the shipped executable and their license
/// expressions, verified against each crate's manifest. Kept as data so the
/// dialog stays trivially auditable against Cargo.toml.
const THIRD_PARTY: &[(&str, &str)] = &[
    ("egui / eframe / egui_plot (GUI)", "MIT OR Apache-2.0"),
    ("serialport (receiver I/O)", "MPL-2.0"),
    ("rustls (TLS)", "Apache-2.0 OR ISC OR MIT"),
    ("ring (cryptography)", "Apache-2.0 AND ISC"),
    ("webpki-roots (CA roots)", "CDLA-Permissive-2.0"),
    ("serde / toml (settings)", "MIT OR Apache-2.0"),
    // The binary embeds egui's default fonts as unmodified data; CREDITS.md
    // promises these notices appear here, so keep the rows in lockstep.
    ("Hack / Noto Emoji fonts", "OFL-1.1"),
    ("Ubuntu-Light font", "UFL-1.0"),
    ("GeoNames city data", "CC BY 4.0"),
    ("US Census ZIP centroids", "Public domain"),
];

/// A cobalt link that reports browser failures to the event log instead of
/// failing silently.
fn link(app: &mut App, ui: &mut egui::Ui, text: &str, url: &str) {
    if ui.link(text).on_hover_text(url).clicked()
        && let Err(e) = crate::audio::open_url(url)
    {
        app.hub.event(format!("Could not open browser: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::THIRD_PARTY;

    /// CREDITS.md states its credits "also appear in the application's
    /// About screen"; the embedded font notices (Hack/Noto Emoji under
    /// OFL-1.1, Ubuntu-Light under UFL-1.0) are part of that claim, so the
    /// About table must carry them.
    #[test]
    fn about_lists_the_font_notices_credits_promises() {
        let has = |name: &str, license: &str| {
            THIRD_PARTY
                .iter()
                .any(|(c, l)| c.contains(name) && l.contains(license))
        };
        assert!(has("Hack", "OFL-1.1"));
        assert!(has("Noto Emoji", "OFL-1.1"));
        assert!(has("Ubuntu-Light", "UFL-1.0"));
    }
}
