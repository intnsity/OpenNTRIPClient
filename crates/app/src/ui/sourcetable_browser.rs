//! Sourcetable Browser - the Sourcetable bottom tab over the active profile's
//! cached sourcetable. Sub-tabs for STR / CAS / NET records (plus Unparsed
//! when the caster leaked non-record lines), a live text filter, click-to-sort
//! on the columns support techs actually scan (mount, format, country,
//! bitrate), and click-to-fill for the mountpoint - the browser upgrade of the
//! main window's small mount dropdown.
//!
//! Rendering is virtualized via `TableBuilder::body().rows()`: only visible
//! rows are laid out, so the EarthScope-scale table (1100+ STR records)
//! scrolls at full frame rate. Filtering and sorting recompute an index list
//! per frame instead of caching: at sourcetable scale that is microseconds,
//! and it keeps this module free of invalidation state.
//!
//! No colors or sizes are invented here: everything renders through egui's
//! semantic styles (strong/weak/monospace, striped rows, text-style heights)
//! plus the named theme tokens for the two table-specific bands the global
//! style cannot express (the recessed header, the hover edge bar).

use std::cmp::Ordering;
use std::sync::Arc;

use egui_extras::{Column, TableBuilder};
use ntrip_core::sourcetable::{SourceTable, StrRecord};

use crate::ui::text::contains_ignore_ascii_case;
use crate::ui::{App, theme};

/// Which record class the table area shows. `Unparsed` is only offered when
/// the parsed table actually has unparsed lines.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Str,
    Cas,
    Net,
    Unparsed,
}

/// Sortable STR columns. Everything else keeps caster order - which is
/// itself meaningful (operators group related streams), so no sort is the
/// default and a third click on a header restores it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SortKey {
    Mount,
    Format,
    Country,
    Bitrate,
}

/// Actions the tab emits; applied after rendering so a click never overlaps
/// the `&mut app.srctbl_view` borrow the render holds.
enum Pending {
    /// Fetch the sourcetable from the active profile's caster.
    Refresh,
    /// Fill the active profile's mountpoint field.
    UseMount(String),
}

/// Per-window UI state, owned by `App` (`srctbl_view`).
#[derive(Default)]
pub struct ViewState {
    tab: Tab,
    filter: String,
    /// Active sort: column + ascending? None = caster order.
    sort: Option<(SortKey, bool)>,
    /// Selected stream, identified by its mountpoint - the exact property
    /// the "Use" button applies. Identifying by semantic key instead of a
    /// row index guarded by table identity makes stale selection
    /// structurally impossible: the previous Arc-address scheme was
    /// ABA-prone (a new table allocation could reuse a freed table's
    /// address and falsely validate an old index). A mountpoint absent
    /// from the current table simply resolves to no selection.
    selected: Option<String>,
}

pub fn tab(app: &mut App, ui: &mut egui::Ui) {
    // Snapshot everything the render reads from outside its own ViewState, so
    // the render body holds the one `&mut app.srctbl_view` borrow and nothing
    // else - leaving `app` free to apply the deferred action afterwards.
    let (host, port) = {
        let p = app.settings.active();
        (p.host.trim().to_string(), p.port)
    };
    let table: Option<Arc<SourceTable>> = app
        .state
        .ntrip
        .sourcetable
        .as_ref()
        .filter(|(h, po, _)| *h == host && *po == port)
        .map(|(_, _, t)| t.clone());
    let busy = app.ntrip_busy();

    let mut pending: Option<Pending> = None;
    {
        let view = &mut app.srctbl_view;
        toolbar(ui, view, &host, port, busy, &mut pending);
        ui.separator();
        match &table {
            Some(t) => {
                sync_view(view, t);
                tabs(ui, view, t);
                // Reserve the footer strip before the table claims the
                // remaining height, then render bottom-up-free: table
                // shrinks, footer always stays visible.
                let footer_h =
                    ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
                let table_h =
                    (ui.available_height() - footer_h - ui.spacing().item_spacing.y * 2.0).max(0.0);
                body(ui, view, t, table_h, &mut pending);
                ui.separator();
                footer(ui, view, t, &mut pending);
            }
            None => {
                empty_hint(ui, &host, port);
            }
        }
    }

    match pending {
        Some(Pending::Refresh) => app.connect_ntrip(true),
        Some(Pending::UseMount(mount)) => {
            app.settings.active_mut().mountpoint = mount;
        }
        None => {}
    }
}

/// Drop state that no longer matches the (possibly refreshed) table. The
/// selection needs no invalidation here: it is a mountpoint, resolved
/// against the current table on every use via [`resolve_selection`].
fn sync_view(view: &mut ViewState, table: &Arc<SourceTable>) {
    if view.tab == Tab::Unparsed && table.unparsed.is_empty() {
        view.tab = Tab::Str;
    }
}

/// The selected mountpoint, if the current table actually carries a stream
/// by that name; otherwise there is no selection to act on.
fn resolve_selection<'a>(strs: &'a [StrRecord], selected: Option<&str>) -> Option<&'a str> {
    let sel = selected?;
    strs.iter()
        .find(|s| s.mountpoint == sel)
        .map(|s| s.mountpoint.as_str())
}

fn toolbar(
    ui: &mut egui::Ui,
    view: &mut ViewState,
    host: &str,
    port: u16,
    busy: bool,
    pending: &mut Option<Pending>,
) {
    ui.horizontal(|ui| {
        let refresh = ui
            .add_enabled(!busy && !host.is_empty(), egui::Button::new("Refresh"))
            .on_hover_text(format!("Download the sourcetable from {host}:{port}"))
            .on_disabled_hover_text(if host.is_empty() {
                "Configure a caster host first".to_string()
            } else {
                "Busy - disconnect first".to_string()
            });
        if refresh.clicked() {
            *pending = Some(Pending::Refresh);
        }
        if host.is_empty() {
            ui.weak("no caster configured");
        } else {
            ui.weak(format!("{host}:{port}"));
        }
        ui.separator();
        ui.label("Filter");
        ui.add(
            egui::TextEdit::singleline(&mut view.filter)
                .desired_width(200.0)
                .hint_text("mount / identifier / format / country"),
        );
        if !view.filter.is_empty() && ui.small_button("x").on_hover_text("Clear filter").clicked() {
            view.filter.clear();
        }
    });
}

fn tabs(ui: &mut egui::Ui, view: &mut ViewState, table: &SourceTable) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut view.tab, Tab::Str, "Streams (STR)");
        ui.selectable_value(&mut view.tab, Tab::Cas, "Casters (CAS)");
        ui.selectable_value(&mut view.tab, Tab::Net, "Networks (NET)");
        if !table.unparsed.is_empty() {
            ui.selectable_value(&mut view.tab, Tab::Unparsed, "Unparsed");
        }
    });
}

fn body(
    ui: &mut egui::Ui,
    view: &mut ViewState,
    table: &SourceTable,
    max_height: f32,
    pending: &mut Option<Pending>,
) {
    // Labels must not grab pointer input or row-click selection dies.
    ui.style_mut().interaction.selectable_labels = false;
    match view.tab {
        Tab::Str => str_table(ui, view, &table.strs, max_height, pending),
        Tab::Cas => cas_table(ui, view, table, max_height),
        Tab::Net => net_table(ui, view, table, max_height),
        Tab::Unparsed => unparsed_list(ui, view, table, max_height),
    }
}

/// Left-edge bar marking the hovered row (pure so the geometry is
/// testable): full row height, a few points wide - enough to read, thin
/// enough to stay out of the first column's text.
fn hover_bar_rect(row: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(row.min, egui::vec2(3.0, row.height()))
}

/// One sortable header cell: click cycles ascending -> descending -> off.
fn sort_header(ui: &mut egui::Ui, view: &mut ViewState, label: &str, key: SortKey) {
    theme::header_band(ui);
    let marker = match view.sort {
        Some((k, true)) if k == key => " ^",
        Some((k, false)) if k == key => " v",
        _ => "",
    };
    let text = egui::RichText::new(format!("{label}{marker}")).strong();
    if ui
        .add(egui::Button::new(text).frame(false))
        .on_hover_text("Sort")
        .clicked()
    {
        view.sort = match view.sort {
            Some((k, true)) if k == key => Some((key, false)),
            Some((k, false)) if k == key => None,
            _ => Some((key, true)),
        };
    }
}

fn plain_header(ui: &mut egui::Ui, label: &str) {
    theme::header_band(ui);
    ui.strong(label);
}

/// A clipped, non-interactive table cell.
fn cell(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) {
    ui.add(egui::Label::new(text).truncate());
}

fn str_table(
    ui: &mut egui::Ui,
    view: &mut ViewState,
    strs: &[StrRecord],
    max_height: f32,
    pending: &mut Option<Pending>,
) {
    let rows = str_rows(strs, view.filter.trim(), view.sort);
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    ui.push_id("srctbl-str", |ui| {
        egui::ScrollArea::horizontal().show(ui, |ui| {
            // Cloned before TableBuilder takes the &mut Ui; used to paint
            // the hover bar from inside the row closure.
            let painter = ui.painter().clone();
            let mut builder = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .min_scrolled_height(0.0)
                .max_scroll_height(max_height)
                .column(Column::auto().at_least(90.0)); // Mount
            for _ in 0..13 {
                builder = builder.column(Column::auto().at_least(24.0));
            }
            builder
                .column(Column::remainder().at_least(48.0)) // Bitrate
                .header(row_h, |mut header| {
                    header.col(|ui| sort_header(ui, view, "Mount", SortKey::Mount));
                    header.col(|ui| plain_header(ui, "Identifier"));
                    header.col(|ui| sort_header(ui, view, "Format", SortKey::Format));
                    header.col(|ui| plain_header(ui, "Carrier"));
                    header.col(|ui| plain_header(ui, "Nav"));
                    header.col(|ui| plain_header(ui, "Network"));
                    header.col(|ui| sort_header(ui, view, "Country", SortKey::Country));
                    header.col(|ui| plain_header(ui, "Lat"));
                    header.col(|ui| plain_header(ui, "Lon"));
                    header.col(|ui| plain_header(ui, "NMEA"));
                    header.col(|ui| plain_header(ui, "Solution"));
                    header.col(|ui| plain_header(ui, "Generator"));
                    header.col(|ui| plain_header(ui, "Auth"));
                    header.col(|ui| plain_header(ui, "Fee"));
                    header.col(|ui| sort_header(ui, view, "Bitrate", SortKey::Bitrate));
                })
                .body(|body| {
                    body.rows(row_h, rows.len(), |mut row| {
                        let idx = rows[row.index()];
                        let s = &strs[idx];
                        row.set_selected(view.selected.as_deref() == Some(s.mountpoint.as_str()));
                        row.col(|ui| cell(ui, egui::RichText::new(&s.mountpoint).monospace()));
                        row.col(|ui| cell(ui, &s.identifier));
                        row.col(|ui| cell(ui, &s.format));
                        row.col(|ui| cell(ui, s.carrier.to_string()));
                        row.col(|ui| cell(ui, &s.nav_system));
                        row.col(|ui| cell(ui, &s.network));
                        row.col(|ui| cell(ui, &s.country));
                        row.col(|ui| cell(ui, format!("{:.2}", s.lat)));
                        row.col(|ui| cell(ui, format!("{:.2}", s.lon)));
                        row.col(|ui| cell(ui, if s.nmea_required { "yes" } else { "" }));
                        row.col(|ui| cell(ui, s.solution.to_string()));
                        row.col(|ui| cell(ui, &s.generator));
                        row.col(|ui| cell(ui, s.auth.to_string().trim()));
                        row.col(|ui| cell(ui, if s.fee { "yes" } else { "" }));
                        row.col(|ui| cell(ui, s.bitrate.to_string()));
                        let resp = row.response();
                        if resp.double_clicked() {
                            view.selected = Some(s.mountpoint.clone());
                            *pending = Some(Pending::UseMount(s.mountpoint.clone()));
                        } else if resp.clicked() {
                            view.selected = Some(s.mountpoint.clone());
                        }
                        // Striped rows fill paper-0, which the paper-tint
                        // hover fill cannot clear (1.01:1 luminance), so
                        // hover also paints the cobalt "a click lands
                        // here" edge bar. Selection stays visible on its
                        // own via the cool accent-tint row fill.
                        if resp.hovered() {
                            painter.rect_filled(
                                hover_bar_rect(resp.rect),
                                egui::CornerRadius::ZERO,
                                theme::ACCENT,
                            );
                        }
                    });
                });
        });
    });
}

fn cas_table(ui: &mut egui::Ui, view: &ViewState, table: &SourceTable, max_height: f32) {
    let filter = view.filter.trim();
    let rows: Vec<usize> = table
        .casters
        .iter()
        .enumerate()
        .filter(|(_, c)| cas_matches(c, filter))
        .map(|(i, _)| i)
        .collect();
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    ui.push_id("srctbl-cas", |ui| {
        egui::ScrollArea::horizontal().show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .min_scrolled_height(0.0)
                .max_scroll_height(max_height)
                .column(Column::auto().at_least(110.0)) // Host
                .column(Column::auto().at_least(40.0)) // Port
                .column(Column::auto().at_least(80.0)) // Identifier
                .column(Column::auto().at_least(80.0)) // Operator
                .column(Column::auto().at_least(40.0)) // NMEA
                .column(Column::auto().at_least(50.0)) // Country
                .column(Column::auto().at_least(48.0)) // Lat
                .column(Column::auto().at_least(48.0)) // Lon
                .column(Column::remainder().at_least(60.0)) // Misc
                .header(row_h, |mut header| {
                    for label in [
                        "Host",
                        "Port",
                        "Identifier",
                        "Operator",
                        "NMEA",
                        "Country",
                        "Lat",
                        "Lon",
                        "Misc",
                    ] {
                        header.col(|ui| plain_header(ui, label));
                    }
                })
                .body(|body| {
                    body.rows(row_h, rows.len(), |mut row| {
                        let c = &table.casters[rows[row.index()]];
                        row.col(|ui| cell(ui, egui::RichText::new(&c.host).monospace()));
                        row.col(|ui| cell(ui, c.port.to_string()));
                        row.col(|ui| cell(ui, &c.identifier));
                        row.col(|ui| cell(ui, &c.operator));
                        row.col(|ui| cell(ui, &c.nmea));
                        row.col(|ui| cell(ui, &c.country));
                        row.col(|ui| cell(ui, format!("{:.2}", c.lat)));
                        row.col(|ui| cell(ui, format!("{:.2}", c.lon)));
                        row.col(|ui| cell(ui, &c.misc));
                    });
                });
        });
    });
}

fn net_table(ui: &mut egui::Ui, view: &ViewState, table: &SourceTable, max_height: f32) {
    let filter = view.filter.trim();
    let rows: Vec<usize> = table
        .networks
        .iter()
        .enumerate()
        .filter(|(_, n)| net_matches(n, filter))
        .map(|(i, _)| i)
        .collect();
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    ui.push_id("srctbl-net", |ui| {
        egui::ScrollArea::horizontal().show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .min_scrolled_height(0.0)
                .max_scroll_height(max_height)
                .column(Column::auto().at_least(80.0)) // Identifier
                .column(Column::auto().at_least(80.0)) // Operator
                .column(Column::auto().at_least(40.0)) // Auth
                .column(Column::auto().at_least(40.0)) // Fee
                .column(Column::auto().at_least(100.0)) // Web (network)
                .column(Column::auto().at_least(100.0)) // Web (streams)
                .column(Column::auto().at_least(100.0)) // Registration
                .column(Column::remainder().at_least(60.0)) // Misc
                .header(row_h, |mut header| {
                    for label in [
                        "Identifier",
                        "Operator",
                        "Auth",
                        "Fee",
                        "Web (network)",
                        "Web (streams)",
                        "Registration",
                        "Misc",
                    ] {
                        header.col(|ui| plain_header(ui, label));
                    }
                })
                .body(|body| {
                    body.rows(row_h, rows.len(), |mut row| {
                        let n = &table.networks[rows[row.index()]];
                        row.col(|ui| cell(ui, &n.identifier));
                        row.col(|ui| cell(ui, &n.operator));
                        row.col(|ui| cell(ui, &n.auth));
                        row.col(|ui| cell(ui, &n.fee));
                        row.col(|ui| cell(ui, &n.web_net));
                        row.col(|ui| cell(ui, &n.web_str));
                        row.col(|ui| cell(ui, &n.web_reg));
                        row.col(|ui| cell(ui, &n.misc));
                    });
                });
        });
    });
}

/// Raw non-record lines, verbatim and monospace - nothing a caster sends is
/// hidden, per the project's diagnostic charter.
fn unparsed_list(ui: &mut egui::Ui, view: &ViewState, table: &SourceTable, max_height: f32) {
    let filter = view.filter.trim();
    let rows: Vec<usize> = table
        .unparsed
        .iter()
        .enumerate()
        .filter(|(_, l)| contains_ignore_ascii_case(l, filter))
        .map(|(i, _)| i)
        .collect();
    let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
    ui.push_id("srctbl-unparsed", |ui| {
        // Monospace log content sits in the recessed paper-0 well, matching
        // the event log and Connection Log beds.
        let well = theme::well_frame(ui.visuals());
        well.show(ui, |ui| {
            egui::ScrollArea::both()
                .max_height(max_height)
                .auto_shrink([false, true])
                .show_rows(ui, row_h, rows.len(), |ui, range| {
                    for i in range {
                        let line = &table.unparsed[rows[i]];
                        ui.add(egui::Label::new(egui::RichText::new(line).monospace()).extend());
                    }
                });
        });
    });
}

fn footer(
    ui: &mut egui::Ui,
    view: &mut ViewState,
    table: &SourceTable,
    pending: &mut Option<Pending>,
) {
    ui.horizontal(|ui| {
        let mut counts = format!(
            "{} streams, {} casters, {} networks",
            table.strs.len(),
            table.casters.len(),
            table.networks.len()
        );
        if !table.unparsed.is_empty() {
            counts.push_str(&format!(", {} unparsed lines", table.unparsed.len()));
        }
        let filter = view.filter.trim();
        if !filter.is_empty() {
            let (shown, total) = match view.tab {
                Tab::Str => (str_rows(&table.strs, filter, None).len(), table.strs.len()),
                Tab::Cas => (
                    table
                        .casters
                        .iter()
                        .filter(|c| cas_matches(c, filter))
                        .count(),
                    table.casters.len(),
                ),
                Tab::Net => (
                    table
                        .networks
                        .iter()
                        .filter(|n| net_matches(n, filter))
                        .count(),
                    table.networks.len(),
                ),
                Tab::Unparsed => (
                    table
                        .unparsed
                        .iter()
                        .filter(|l| contains_ignore_ascii_case(l, filter))
                        .count(),
                    table.unparsed.len(),
                ),
            };
            counts.push_str(&format!(" - filter shows {shown} of {total}"));
        }
        ui.weak(counts);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let selected =
                resolve_selection(&table.strs, view.selected.as_deref()).map(str::to_string);
            let label = match &selected {
                Some(m) => format!("Use {m}"),
                None => "Use".to_string(),
            };
            let resp = ui
                .add_enabled(selected.is_some(), egui::Button::new(label))
                .on_hover_text("Set this stream as the profile's mountpoint")
                .on_disabled_hover_text("Select a stream in the STR tab first");
            if resp.clicked()
                && let Some(m) = selected
            {
                *pending = Some(Pending::UseMount(m));
            }
        });
    });
}

fn empty_hint(ui: &mut egui::Ui, host: &str, port: u16) {
    ui.add_space(ui.text_style_height(&egui::TextStyle::Body));
    if host.is_empty() {
        ui.label("No caster host configured. Enter one in the NTRIP Caster panel first.");
    } else {
        ui.label(format!("No sourcetable loaded for {host}:{port}."));
        ui.weak("Refresh fetches it from the caster (held in memory for this session).");
    }
}

// ----------------------------------------------------------------------
// Pure filter / sort predicates (unit-tested below).
// ----------------------------------------------------------------------

/// ASCII case-insensitive ordering without allocating.
fn cmp_ci(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// The live filter matches the fields a tech searches by: mountpoint,
/// identifier (city), format, and country.
fn str_matches(s: &StrRecord, filter: &str) -> bool {
    contains_ignore_ascii_case(&s.mountpoint, filter)
        || contains_ignore_ascii_case(&s.identifier, filter)
        || contains_ignore_ascii_case(&s.format, filter)
        || contains_ignore_ascii_case(&s.country, filter)
}

fn cas_matches(c: &ntrip_core::sourcetable::CasRecord, filter: &str) -> bool {
    contains_ignore_ascii_case(&c.host, filter)
        || contains_ignore_ascii_case(&c.identifier, filter)
        || contains_ignore_ascii_case(&c.operator, filter)
        || contains_ignore_ascii_case(&c.country, filter)
}

fn net_matches(n: &ntrip_core::sourcetable::NetRecord, filter: &str) -> bool {
    contains_ignore_ascii_case(&n.identifier, filter)
        || contains_ignore_ascii_case(&n.operator, filter)
}

/// Filtered (and optionally sorted) view of `strs` as indices. Sorting is
/// stable, with mountpoint as the secondary key so equal primary keys land
/// in a predictable, human-scannable order.
fn str_rows(strs: &[StrRecord], filter: &str, sort: Option<(SortKey, bool)>) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..strs.len())
        .filter(|&i| str_matches(&strs[i], filter))
        .collect();
    if let Some((key, ascending)) = sort {
        rows.sort_by(|&a, &b| {
            let (a, b) = (&strs[a], &strs[b]);
            let ord = match key {
                SortKey::Mount => cmp_ci(&a.mountpoint, &b.mountpoint),
                SortKey::Format => {
                    cmp_ci(&a.format, &b.format).then_with(|| cmp_ci(&a.mountpoint, &b.mountpoint))
                }
                SortKey::Country => cmp_ci(&a.country, &b.country)
                    .then_with(|| cmp_ci(&a.mountpoint, &b.mountpoint)),
                SortKey::Bitrate => a
                    .bitrate
                    .cmp(&b.bitrate)
                    .then_with(|| cmp_ci(&a.mountpoint, &b.mountpoint)),
            };
            if ascending { ord } else { ord.reverse() }
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture through the real parser, so field indexing stays honest.
    /// Bitrates are chosen to break lexicographic sorting (19200 < 2400 as
    /// strings), the classic mistake this table must not make.
    fn fixture() -> SourceTable {
        let raw = b"\
STR;P401_RTCM3;Portland;RTCM 3.2;1004(1),1005(10);2;GPS+GLO;PBO;USA;45.50;-122.60;1;0;TRIMBLE NETR9;none;B;N;2400;
STR;OGD1_RTCM3;Ogden;RTCM 3.3;MSM7;2;GPS+GLO+GAL;CHC;USA;41.20;-111.90;0;0;CHC N72;none;B;N;9600;
STR;BERN0;Bern;RTCM 3.1;1004;2;GPS;IGS;CHE;46.87;7.46;1;1;LEICA GR25;none;B;Y;500;
STR;zzz_low;Anywhere;CMR+;;0;GPS;;DEU;50.00;8.00;0;0;GENERIC;none;N;N;19200;
CAS;caster.example.com;2101;Example;ExampleOp;0;USA;45.00;-122.00;extra
NET;IGSNET;IGS;B;N;http://net;http://str;reg@example.com;extra
some stray vendor line
ENDSOURCETABLE
";
        ntrip_core::sourcetable::parse(raw)
    }

    fn mounts(strs: &[StrRecord], rows: &[usize]) -> Vec<String> {
        rows.iter().map(|&i| strs[i].mountpoint.clone()).collect()
    }

    #[test]
    fn fixture_parses_all_record_classes() {
        let t = fixture();
        assert_eq!(t.strs.len(), 4);
        assert_eq!(t.casters.len(), 1);
        assert_eq!(t.networks.len(), 1);
        assert_eq!(t.unparsed.len(), 1);
    }

    #[test]
    fn filter_matches_mount_identifier_format_and_country() {
        let t = fixture();
        // Empty filter: everything, in caster order.
        assert_eq!(str_rows(&t.strs, "", None).len(), 4);
        // Format (and mountpoint) hits.
        assert_eq!(
            mounts(&t.strs, &str_rows(&t.strs, "rtcm", None)),
            ["P401_RTCM3", "OGD1_RTCM3", "BERN0"]
        );
        // Country, case-insensitive.
        assert_eq!(mounts(&t.strs, &str_rows(&t.strs, "che", None)), ["BERN0"]);
        // Identifier, case-insensitive.
        assert_eq!(
            mounts(&t.strs, &str_rows(&t.strs, "OGDEN", None)),
            ["OGD1_RTCM3"]
        );
        assert!(str_rows(&t.strs, "no such thing", None).is_empty());
    }

    #[test]
    fn no_sort_preserves_caster_order() {
        let t = fixture();
        assert_eq!(str_rows(&t.strs, "", None), [0, 1, 2, 3]);
    }

    #[test]
    fn sort_by_mount_is_case_insensitive_both_directions() {
        let t = fixture();
        let asc = str_rows(&t.strs, "", Some((SortKey::Mount, true)));
        assert_eq!(
            mounts(&t.strs, &asc),
            ["BERN0", "OGD1_RTCM3", "P401_RTCM3", "zzz_low"]
        );
        let desc = str_rows(&t.strs, "", Some((SortKey::Mount, false)));
        assert_eq!(
            mounts(&t.strs, &desc),
            ["zzz_low", "P401_RTCM3", "OGD1_RTCM3", "BERN0"]
        );
    }

    #[test]
    fn sort_by_bitrate_is_numeric_not_lexicographic() {
        let t = fixture();
        let asc = str_rows(&t.strs, "", Some((SortKey::Bitrate, true)));
        let rates: Vec<u32> = asc.iter().map(|&i| t.strs[i].bitrate).collect();
        assert_eq!(rates, [500, 2400, 9600, 19200]);
    }

    #[test]
    fn sort_by_country_breaks_ties_by_mountpoint() {
        let t = fixture();
        let asc = str_rows(&t.strs, "", Some((SortKey::Country, true)));
        assert_eq!(
            mounts(&t.strs, &asc),
            ["BERN0", "zzz_low", "OGD1_RTCM3", "P401_RTCM3"]
        );
    }

    #[test]
    fn filter_and_sort_compose() {
        let t = fixture();
        let rows = str_rows(&t.strs, "rtcm", Some((SortKey::Bitrate, false)));
        assert_eq!(
            mounts(&t.strs, &rows),
            ["OGD1_RTCM3", "P401_RTCM3", "BERN0"]
        );
    }

    /// Selection is keyed by mountpoint, so a refreshed table - reordered,
    /// shrunk, or a fresh allocation at a recycled address - can never remap
    /// it to a different stream. The old index+Arc-address scheme could
    /// (ABA); a mountpoint either names the same stream or nothing.
    #[test]
    fn selection_resolves_by_mountpoint_not_row_position() {
        let t = fixture();
        assert_eq!(
            resolve_selection(&t.strs, Some("OGD1_RTCM3")),
            Some("OGD1_RTCM3")
        );
        // A refreshed table where the caster reordered and dropped streams:
        // the selection follows the stream, not the row index.
        let refreshed = ntrip_core::sourcetable::parse(
            b"\
STR;BERN0;Bern;RTCM 3.1;1004;2;GPS;IGS;CHE;46.87;7.46;1;1;LEICA GR25;none;B;Y;500;
STR;OGD1_RTCM3;Ogden;RTCM 3.3;MSM7;2;GPS+GLO+GAL;CHC;USA;41.20;-111.90;0;0;CHC N72;none;B;N;9600;
ENDSOURCETABLE
",
        );
        assert_eq!(
            resolve_selection(&refreshed.strs, Some("OGD1_RTCM3")),
            Some("OGD1_RTCM3")
        );
        // A stream the refreshed table no longer carries is no selection.
        assert_eq!(resolve_selection(&refreshed.strs, Some("P401_RTCM3")), None);
        assert_eq!(resolve_selection(&t.strs, None), None);
    }

    /// The hover cue hugs the row's left edge at full row height and stays
    /// narrow enough not to underline the first column's text.
    #[test]
    fn hover_bar_spans_row_height_at_left_edge() {
        let row = egui::Rect::from_min_size(egui::pos2(10.0, 40.0), egui::vec2(900.0, 18.0));
        let bar = hover_bar_rect(row);
        assert_eq!(bar.min, row.min);
        assert_eq!(bar.height(), row.height());
        assert!(bar.width() < 6.0);
    }

    #[test]
    fn cas_and_net_filters_cover_their_search_fields() {
        let t = fixture();
        let c = &t.casters[0];
        assert!(cas_matches(c, "example.COM"));
        assert!(cas_matches(c, "usa"));
        assert!(!cas_matches(c, "rtcm"));
        let n = &t.networks[0];
        assert!(net_matches(n, "igs"));
        assert!(!net_matches(n, "usa"));
    }
}
