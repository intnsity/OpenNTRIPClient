//! Connection Log - the Conn bottom tab: the verbatim protocol exchange
//! (TX/RX lines, GGA uplinks, TLS results, reconnect decisions) over the 5 k
//! conn ring, plus a hex dump of the most recent unclassifiable caster
//! response.
//!
//! Rendering is virtualized (`show_rows`), so a full ring costs the same as
//! one screenful. The live filter selects row INDICES instead of copying
//! lines: the ring stays the single owner of the text, and stick-to-bottom
//! keeps following new arrivals until the user scrolls away (egui re-sticks
//! when they return to the bottom), which is the pause-on-scroll behavior a
//! diagnostic log needs.

use std::fmt::Write as _;

use crate::ui::text::contains_ignore_ascii_case;
use crate::ui::{App, theme};

/// Tab-local UI state; `main_window::App` holds one (`connlog_view`),
/// constructed via `Default`.
#[derive(Default)]
pub struct ViewState {
    /// Live ASCII-case-insensitive substring filter over the line view.
    pub filter: String,
    /// True while the hex dump replaces the line view.
    pub show_hex: bool,
}

/// Butter Paper exchange-direction colors: TX rides the cobalt accent
/// ("something WE did"), RX the emerald success token - both AA on every
/// paper step, pinned by theme.rs's `foregrounds_clear_aa_on_base_paper`.
/// Notice lines return `None` and keep the default ink - the verbatim
/// exchange is the star of this window, not the commentary.
fn direction_color(d: &Direction) -> Option<egui::Color32> {
    match d {
        Direction::Tx => Some(theme::ACCENT),
        Direction::Rx => Some(theme::SUCCESS),
        Direction::Note => None,
    }
}

pub fn tab(app: &mut App, ui: &mut egui::Ui) {
    // Disjoint field borrows: the view state mutates, the ring is read-only.
    let view = &mut app.connlog_view;
    let conn = &app.state.conn;
    let raw = app.state.ntrip.last_unknown_response.as_deref();

    // A captured response outliving the session is the whole point of the
    // hex view (forensics after the retry succeeded), so the toggle never
    // turns itself off; it is merely disabled until evidence exists.
    let hex_active = view.show_hex && raw.is_some();

    // Filtered row indices, recomputed per frame: 5 k substring probes are
    // well under a millisecond and a cache would have to watch both the
    // filter text and ring churn.
    let filtered: Option<Vec<usize>> = (!view.filter.is_empty() && !hex_active).then(|| {
        (0..conn.len())
            .filter(|&i| {
                conn.get(i)
                    .is_some_and(|l| contains_ignore_ascii_case(l, &view.filter))
            })
            .collect()
    });

    ui.horizontal(|ui| {
        ui.add_enabled(
            !hex_active,
            egui::TextEdit::singleline(&mut view.filter)
                .hint_text("filter")
                .desired_width(200.0),
        );
        let count = match (&filtered, raw) {
            _ if !hex_active => match &filtered {
                Some(idx) => format!("{} of {} lines", idx.len(), conn.len()),
                None => format!("{} lines", conn.len()),
            },
            (_, Some(r)) => format!("{} bytes", r.len()),
            _ => String::new(),
        };
        ui.label(egui::RichText::new(count).weak().small());

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let copy = ui.button("Copy all").on_hover_text(
                "Copy the current view (filtered lines only while a filter is active)",
            );
            if copy.clicked() {
                let text = match (hex_active, raw, &filtered) {
                    (true, Some(r), _) => hex_dump(r),
                    (_, _, Some(idx)) => join_lines(conn, idx),
                    _ => conn.join(),
                };
                ui.ctx().copy_text(text);
            }
            let toggle = ui
                .add_enabled(
                    raw.is_some(),
                    egui::Button::selectable(hex_active, "Hex dump"),
                )
                .on_hover_text("Raw bytes of the most recent unclassified caster response")
                .on_disabled_hover_text("No unclassified caster response captured yet");
            if toggle.clicked() {
                view.show_hex = !view.show_hex;
            }
        });
    });
    ui.separator();

    let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
    // Both views are monospace diagnostic beds, so they sit in the same
    // recessed paper-0 well the main window's event log uses - flat-on-card
    // log content breaks the theme's depth model.
    let well = theme::well_frame(ui.visuals());
    well.show(ui, |ui| match (hex_active, raw) {
        (true, Some(r)) => {
            // Virtualized like the line view: a caster that answers with a
            // captive-portal page can hand back a large body.
            egui::ScrollArea::both()
                .id_salt("connlog-hex")
                .auto_shrink([false, false])
                .show_rows(ui, row_h, hex_rows(r.len()), |ui, range| {
                    for row in range {
                        ui.add(
                            egui::Label::new(egui::RichText::new(hex_row(r, row)).monospace())
                                .extend(),
                        );
                    }
                });
        }
        _ => {
            let total = filtered.as_ref().map_or(conn.len(), Vec::len);
            egui::ScrollArea::both()
                .id_salt("connlog-lines")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show_rows(ui, row_h, total, |ui, range| {
                    for r in range {
                        let i = filtered.as_ref().map_or(r, |v| v[r]);
                        let Some(line) = conn.get(i) else { continue };
                        let text = egui::RichText::new(line).monospace();
                        let text = match direction_color(&direction(line)) {
                            Some(c) => text.color(c),
                            None => text,
                        };
                        ui.add(egui::Label::new(text).extend());
                    }
                });
        }
    });
}

/// Join the ring lines named by `idx`, one per line - the filtered
/// counterpart of `Ring::join`.
fn join_lines(conn: &crate::state::Ring, idx: &[usize]) -> String {
    let mut out = String::new();
    for &i in idx {
        if let Some(line) = conn.get(i) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

enum Direction {
    Tx,
    Rx,
    Note,
}

/// Classify a ring line by the worker's verbatim-exchange prefixes. Lines
/// arrive stamped "HH:MM:SS " by the hub; protocol lines then carry "> "
/// (client to caster) or "< " (caster to client), everything else (TLS
/// results, reconnect notices) is a Note. The stamp is skipped by shape
/// rather than assumed, so an unstamped line still classifies and a payload
/// that merely CONTAINS "> " never misclassifies.
fn direction(line: &str) -> Direction {
    let b = line.as_bytes();
    let rest = match b {
        [_, _, b':', _, _, b':', _, _, b' ', rest @ ..] => rest,
        _ => b,
    };
    match rest {
        [b'>', b' ', ..] => Direction::Tx,
        [b'<', b' ', ..] => Direction::Rx,
        _ => Direction::Note,
    }
}

const BYTES_PER_ROW: usize = 16;

/// Number of hex-dump rows for a payload of `len` bytes.
fn hex_rows(len: usize) -> usize {
    len.div_ceil(BYTES_PER_ROW)
}

/// One classic hexdump row: 8-digit offset, 16 hex bytes with a mid-row gap,
/// then an ASCII gutter ('.' for non-printables). A partial final row pads
/// the hex column to full width so the gutter stays aligned - that vertical
/// line is what makes an HTML error page recognizable at a glance.
fn hex_row(data: &[u8], row: usize) -> String {
    let start = row * BYTES_PER_ROW;
    let chunk = &data[start..data.len().min(start + BYTES_PER_ROW)];
    let mut out = String::with_capacity(80);
    let _ = write!(out, "{start:08X} ");
    for i in 0..BYTES_PER_ROW {
        let sep = if i == 8 { "  " } else { " " };
        match chunk.get(i) {
            Some(b) => {
                let _ = write!(out, "{sep}{b:02X}");
            }
            None => {
                out.push_str(sep);
                out.push_str("  ");
            }
        }
    }
    out.push_str("  |");
    for &b in chunk {
        out.push(if (0x20..=0x7E).contains(&b) {
            b as char
        } else {
            '.'
        });
    }
    out.push('|');
    out
}

/// The whole payload as hexdump text, for the clipboard.
fn hex_dump(data: &[u8]) -> String {
    let mut out = String::new();
    for row in 0..hex_rows(data.len()) {
        out.push_str(&hex_row(data, row));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_row_full_row_exact() {
        let data = b"HTTP/1.1 302 Fou";
        assert_eq!(
            hex_row(data, 0),
            "00000000  48 54 54 50 2F 31 2E 31  20 33 30 32 20 46 6F 75  |HTTP/1.1 302 Fou|"
        );
    }

    #[test]
    fn hex_row_final_partial_row_pads_hex_column() {
        // 20 bytes: the second row holds 4 and must align its ASCII gutter
        // with the full first row.
        let mut data = b"HTTP/1.1 302 Fou".to_vec();
        data.extend_from_slice(b"nd\r\n");
        let full = hex_row(&data, 0);
        let partial = hex_row(&data, 1);
        // 12 missing bytes pad 3 columns each, +1 for the mid-row gap, +2
        // before the gutter: 39 spaces between the last byte and the bar.
        assert_eq!(
            partial,
            format!("00000010  6E 64 0D 0A{}|nd..|", " ".repeat(39))
        );
        // The gutter opens at the same column in both rows.
        assert_eq!(full.find('|'), partial.find('|'));
        // Offset advances by one row of bytes.
        assert!(partial.starts_with("00000010 "));
    }

    #[test]
    fn hex_row_masks_non_printables() {
        let data = [0x00u8, 0x1F, 0x20, 0x7E, 0x7F, 0xFF];
        let row = hex_row(&data, 0);
        assert!(row.ends_with("|.. ~..|"), "{row}");
        assert!(row.contains("00 1F 20 7E 7F FF"), "{row}");
    }

    #[test]
    fn hex_rows_counts_partial_rows() {
        assert_eq!(hex_rows(0), 0);
        assert_eq!(hex_rows(1), 1);
        assert_eq!(hex_rows(16), 1);
        assert_eq!(hex_rows(17), 2);
        assert_eq!(hex_rows(32), 2);
    }

    #[test]
    fn hex_dump_joins_rows_with_newlines() {
        let data = [0x41u8; 17];
        let dump = hex_dump(&data);
        assert_eq!(dump.lines().count(), 2);
        assert!(dump.ends_with("|A|\n"), "{dump:?}");
    }

    /// TX/RX lines take theme tokens, never ad-hoc literals: ACCENT and
    /// SUCCESS are in theme.rs's AA-checked foreground set, so this mapping
    /// plus `foregrounds_clear_aa_on_base_paper` together guarantee the
    /// protocol exchange stays readable on the paper. Notes keep the
    /// default ink (`None`).
    #[test]
    fn direction_colors_are_theme_tokens() {
        assert_eq!(direction_color(&Direction::Tx), Some(theme::ACCENT));
        assert_eq!(direction_color(&Direction::Rx), Some(theme::SUCCESS));
        assert_eq!(direction_color(&Direction::Note), None);
    }

    #[test]
    fn direction_classifies_stamped_and_unstamped_lines() {
        assert!(matches!(
            direction("12:34:56 > GET /RTCM32 HTTP/1.1"),
            Direction::Tx
        ));
        assert!(matches!(direction("12:34:56 < ICY 200 OK"), Direction::Rx));
        assert!(matches!(
            direction("12:34:56 Reconnecting in 10 s (attempt 2 of 100)"),
            Direction::Note
        ));
        // Unstamped fallbacks still classify by prefix.
        assert!(matches!(direction("> $GPGGA,..."), Direction::Tx));
        assert!(matches!(
            direction("< Transfer-Encoding: chunked"),
            Direction::Rx
        ));
        // A payload merely containing "> " stays a Note.
        assert!(matches!(
            direction("12:34:56 TLS handshake failed: cert > expired"),
            Direction::Note
        ));
        assert!(matches!(direction(""), Direction::Note));
    }
}
