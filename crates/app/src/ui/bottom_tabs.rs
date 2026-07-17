//! Bottom tab pane: the four always-present diagnostic surfaces - Event log,
//! Connection log, Stream detail, Sourcetable - filling the remaining
//! central height. One Surface law: these lived in floating windows; now
//! nothing about the session hides behind a window manager. All four tabs
//! are always offered (empty states teach the fetch path and the strip never
//! jumps width); the active tab IS `settings.window.tab`, persisted by the
//! unconditional exit save like the graph disclosure.
//!
//! The strip carries one affordance beyond selection: a warning dot on the
//! Conn tab while the worker has captured an unclassified caster response the
//! user has not yet looked at (`state.ntrip.unknown_response_gen` running
//! ahead of `App.conn_unknown_ack`). Opening the tab acks the generation and
//! clears the dot. Per-tab Copy/Clear stay inside each tab that owns them -
//! a strip-level cluster would only duplicate controls with different scopes.

use crate::settings::BottomTab;
use crate::ui::{App, theme};

/// Tab, strip label, and hover explainer, in strip order.
const TABS: [(BottomTab, &str, &str); 4] = [
    (BottomTab::Events, "Event log", "Application events"),
    (
        BottomTab::Conn,
        "Connection log",
        "Verbatim protocol exchange for the current session",
    ),
    (
        BottomTab::Stream,
        "Stream detail",
        "Live RTCM message statistics and decoded base data",
    ),
    (
        BottomTab::Sourcetable,
        "Sourcetable",
        "Browse the fetched sourcetable (filter, sort, pick a mount)",
    ),
];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // Computed before the visible-Conn ack below, so a fresh capture shows
    // its dot the same frame it lands (unless the Conn tab is already open).
    let unseen_unknown = app.state.ntrip.unknown_response_gen > app.conn_unknown_ack;
    ui.horizontal(|ui| {
        for (tab, label, hover) in TABS {
            let active = app.settings.window.tab == tab;
            let attention = tab == BottomTab::Conn && unseen_unknown && !active;
            let hover = if attention {
                "An unrecognized caster response was captured - open to inspect it"
            } else {
                hover
            };
            let resp = ui
                .add(egui::Button::selectable(active, label))
                .on_hover_text(hover);
            if attention {
                // A small warning dot on the tab's top-right corner: enough to
                // pull the eye, gone the moment the tab is opened.
                ui.painter().circle_filled(
                    resp.rect.right_top() + egui::vec2(-3.0, 3.0),
                    3.0,
                    theme::WARNING,
                );
            }
            if resp.clicked() {
                app.settings.window.tab = tab;
            }
        }
    });
    ui.add_space(2.0);
    // Seeing the Conn tab IS the acknowledgement: stamp the current
    // unknown-response generation on every frame it is visible, so the
    // badge predicate (builder-owned) has nothing left to point at.
    if app.settings.window.tab == BottomTab::Conn {
        app.conn_unknown_ack = app.state.ntrip.unknown_response_gen;
    }
    match app.settings.window.tab {
        BottomTab::Events => super::log_pane::tab(app, ui),
        BottomTab::Conn => super::connlog_window::tab(app, ui),
        BottomTab::Stream => super::rtcm_inspector::tab(app, ui),
        BottomTab::Sourcetable => super::sourcetable_browser::tab(app, ui),
    }
}
