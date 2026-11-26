//! The current/new name table: virtualized, so a frame costs O(visible rows).
//! The sort is a view over the plan, which stays in apply order, and the
//! headers are the only control over it.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use egui::emath::GuiRounding as _;
use egui::{Align, Color32, Layout};
use egui_extras::{Column, TableBuilder};

use renameant_core::plan::Status;

use crate::format;
use crate::model::{PlanModel, Sort, SortKey};
use crate::style;

const HEADER_HEIGHT: f32 = 24.0;
const ROW_HEIGHT: f32 = 20.0;
const STATUS_WIDTH: f32 = 110.0;
/// The merged path column: room for a full path, with the name's tail last.
const PATH_WIDTH: f32 = 300.0;
const NAME_WIDTH: f32 = 150.0;
const NAME_MIN_WIDTH: f32 = 80.0;
const SIZE_WIDTH: f32 = 70.0;
const SIZE_MIN_WIDTH: f32 = 50.0;
const REMOVE_WIDTH: f32 = 70.0;
/// The sort marker beside a header: a painted triangle, not a glyph.
const ARROW_WIDTH: f32 = 10.0;
const ARROW_HEIGHT: f32 = 6.0;
/// How strong a row's status tint is on a striped and an unstriped row.
const ROW_FILL_ALPHA: [u8; 2] = [56, 36];

/// What a frame of the table changed: the path a row was removed by, if any.
/// The caller drops that path from its picked list too.
pub struct Change {
    pub removed: Option<PathBuf>,
}

/// Draws the table. A row's `X` or the Remove item in its context menu removes
/// it.
pub fn show(ui: &mut egui::Ui, model: &mut PlanModel) -> Change {
    if model.index.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label("Pick a folder or file to preview the rename.");
        });
        return Change { removed: None };
    }

    let view_len = model.view.len();
    let mut removed: Option<PathBuf> = None;
    // A label left selectable senses drag, which makes it the row's topmost
    // interactive widget and leaves the row's own hover highlight unpainted.
    ui.style_mut().interaction.selectable_labels = false;
    // The Modified column takes the remaining width, so a wide centre panel is
    // filled; the rest keep their widths and the horizontal scroll keeps them
    // reachable when the panel is narrow.
    let mut header_bottom = None;
    // The frame and the row bands span the columns, not the whole scroll viewport,
    // so neither leaves an unpainted strip after the last column.
    let mut content_width = 0.0;
    let row_gap = ui.spacing().item_spacing.y;
    let area = egui::ScrollArea::horizontal().auto_shrink([true, false]).show(ui, |ui| {
        // egui_extras draws a column's separator only when the column can be
        // dragged, so the fixed columns' separators are painted here instead.
        let mut fixed_edges: Vec<f32> = Vec::new();
        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            // Cells sense clicks so a row's context menu can open; they are not
            // keyboard focus stops, because the Remove chip is the row's action.
            .sense(egui::Sense::CLICK)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(STATUS_WIDTH).resizable(false).clip(true))
            .column(Column::initial(PATH_WIDTH).at_least(NAME_MIN_WIDTH).clip(true))
            .column(Column::initial(NAME_WIDTH).at_least(NAME_MIN_WIDTH).clip(true))
            .column(Column::initial(SIZE_WIDTH).at_least(SIZE_MIN_WIDTH).clip(true))
            .column(Column::remainder().resizable(false))
            .column(Column::exact(REMOVE_WIDTH).resizable(false))
            .header(HEADER_HEIGHT, |mut header| {
                header.col(|ui| {
                    header_bottom = Some(ui.max_rect().bottom());
                    fixed_edges.push(fixed_column_edge(ui));
                    sort_cell(ui, model, SortKey::Status, "Status", HeaderAlign::Left);
                });
                header.col(|ui| {
                    sort_cell(ui, model, SortKey::Path, "Path", HeaderAlign::Left);
                });
                header.col(|ui| {
                    sort_cell(ui, model, SortKey::After, "New Name", HeaderAlign::Left);
                });
                header.col(|ui| {
                    sort_cell(ui, model, SortKey::Size, "Size", HeaderAlign::Right);
                });
                header.col(|ui| {
                    fixed_edges.push(fixed_column_edge(ui));
                    sort_cell(ui, model, SortKey::Mtime, "Modified", HeaderAlign::Center);
                });
                header.col(|ui| {
                    cell(ui, |ui| {
                        let width = text_width(ui, "Remove");
                        center(ui, width, |ui| {
                            ui.strong("Remove");
                        });
                    });
                });
            });
        table.body(|mut body| {
            let spacing_x = body.ui_mut().spacing().item_spacing.x;
            // One band per row, across every column, not one stop per cell.
            content_width = {
                let widths = body.widths();
                widths.iter().sum::<f32>() + spacing_x * (widths.len() - 1) as f32
            };
            // Allow only the first row's band and stripe to rise into the header
            // gap, and clip the right to the columns, not the scroll viewport,
            // so neither bleeds past the last column.
            let spacing_y = body.ui_mut().spacing().item_spacing.y;
            let mut clip = body.ui_mut().clip_rect();
            clip.min.y -= spacing_y / 2.0;
            clip.max.x = (clip.left() + content_width).min(clip.max.x);
            body.ui_mut().set_clip_rect(clip);
            // `rows` consumes the body; keep a painter clipped to the columns so
            // the fills span the row without bleeding past the table's edge.
            let painter = body.ui_mut().painter().with_clip_rect(clip);
            body.rows(ROW_HEIGHT, view_len, |mut row| {
                let at = row.index();
                let plan_index = model.view[at];
                let entry = &model.plan.rows[plan_index];
                row.col(|ui| {
                    row_fill(&painter, ui, entry.status, at, content_width);
                    cell(ui, |ui| {
                        let label = format::status_label(entry.status);
                        ui.colored_label(status_color(ui, entry.status), label)
                            .on_hover_text(label);
                    });
                });
                row.col(|ui| {
                    cell(ui, |ui| {
                        ui.add(egui::Label::new(path_text(ui, &entry.src)).truncate());
                    });
                });
                row.col(|ui| {
                    cell(ui, |ui| {
                        let name = egui::RichText::new(format::file_text(&entry.dst)).strong();
                        ui.add(egui::Label::new(name).truncate());
                    });
                });
                let meta = model.meta.get(&entry.src);
                let size =
                    meta.and_then(|meta| meta.size).map_or_else(String::new, format::size_text);
                let modified = meta
                    .and_then(|meta| meta.mtime)
                    .map_or_else(String::new, |mtime| format::local_text(unix_seconds(mtime)));
                row.col(|ui| {
                    cell(ui, |ui| {
                        right(ui, |ui| {
                            ui.label(size);
                        });
                    });
                });
                row.col(|ui| {
                    cell(ui, |ui| {
                        let width = text_width(ui, &modified);
                        center(ui, width, |ui| {
                            // Extend, not wrap: the timestamp stays one line.
                            ui.add(egui::Label::new(egui::RichText::new(modified)).extend());
                        });
                    });
                });
                let removed_here = entry.src.clone();
                row.col(|ui| {
                    cell(ui, |ui| {
                        center(ui, style::REMOVE_CHIP, |ui| {
                            // The row's one action, focusable and announced.
                            let chip = style::remove(
                                ui,
                                &format!("remove {}", format::file_text(&removed_here)),
                            )
                            .on_hover_text("Remove this row from the list");
                            if chip.clicked() {
                                removed = Some(removed_here.clone());
                            }
                        });
                    });
                });
                row.response().context_menu(|ui| {
                    if ui.button("Remove").clicked() {
                        removed = Some(removed_here.clone());
                        ui.close();
                    }
                });
            });
        });
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let y = ui.min_rect().y_range();
        for x in fixed_edges {
            ui.painter().vline(x, y, stroke);
        }
    });
    // The frame and the rule under the titles are painted outside the scroll
    // area so they stay at the table's edges when the columns scroll. Both fit
    // the columns, so the frame never encloses an empty run. The rule sits half a
    // row gap into the header-body gap, level with the first row band's top.
    if let Some(header_bottom) = header_bottom {
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let viewport = area.inner_rect;
        let width = content_width.min(viewport.width());
        let rect = egui::Rect::from_min_size(viewport.min, egui::vec2(width, viewport.height()));
        // The rule stops at the frame's side lines, which are drawn over its
        // ends so it cannot poke out of the table.
        ui.painter()
            .hline(rect.x_range().shrink(stroke.width), header_bottom + row_gap / 2.0, stroke);
        ui.painter().rect_stroke(rect, egui::CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
    }
    Change { removed }
}

/// A row's full path, with its name drawn stronger so the row's own name stands
/// out from the folders above it.
fn path_text(ui: &egui::Ui, path: &Path) -> egui::text::LayoutJob {
    let (dir, name) = format::path_parts(path);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    let styled =
        |color: Color32| egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
    job.append(&dir, 0.0, styled(ui.visuals().text_color()));
    job.append(&name, 0.0, styled(ui.visuals().strong_text_color()));
    job
}

/// Paints a row's status tint across every column, with the table body's painter
/// so it spans the whole row instead of one cell. Half a row gap is added on each
/// side so it meets the row above and below; two strengths alternate.
fn row_fill(
    painter: &egui::Painter,
    ui: &egui::Ui,
    status: Status,
    row_index: usize,
    content_width: f32,
) {
    // The band is the raw palette hue, not the tinted text colour. "no match" is
    // the default, so it keeps the table's plain stripes.
    let hue = match status {
        Status::Ok => style::OK,
        Status::Conflict(_) => style::ACCENT,
        Status::Invalid(_) => style::DANGER,
        Status::Unchanged => return,
    };
    let cell = ui.max_rect();
    let half = 0.5 * ui.spacing().item_spacing.y;
    let rect = egui::Rect::from_x_y_ranges(
        cell.left()..=(cell.left() + content_width),
        (cell.top() - half)..=(cell.bottom() + half),
    )
    .round_ui();
    let fill = Color32::from_rgba_unmultiplied(
        hue.r(),
        hue.g(),
        hue.b(),
        ROW_FILL_ALPHA[row_index % 2],
    );
    painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
}

/// One table cell's content, inset by [`style::CELL_PADDING`] on both sides so
/// every column's content lines up. The horizontal scope fills the row height,
/// which centres the content vertically; the frame supplies the same horizontal
/// inset to every cell.
fn cell<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal_centered(|ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(style::CELL_PADDING, 0))
            .show(ui, content)
            .inner
    })
    .inner
}

/// A cell whose content packs against the column's right edge.
fn right<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.with_layout(Layout::right_to_left(Align::Center), content).inner
}

/// The width `text` has at the table's body style.
fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.painter().layout_no_wrap(text.to_owned(), font, ui.visuals().text_color()).size().x
}

/// A cell whose content is centred in the column, given its width. The leading
/// space keeps the content's own size, which justification would not: it would
/// stretch the `X`'s click target to the cell.
fn center<R>(ui: &mut egui::Ui, width: f32, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let lead = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(lead);
        content(ui)
    })
    .inner
}

/// The x of the divider after a fixed column. `ui` is that column's header cell;
/// the offset is the spacing egui_extras leaves between columns, so the line
/// lands where it puts a resizable column's separator.
fn fixed_column_edge(ui: &egui::Ui) -> f32 {
    ui.max_rect().right() + ui.spacing().item_spacing.x
}

/// The file's modification time as unix seconds. A pre-epoch time reads as 0,
/// so it shows the epoch in local time rather than failing.
fn unix_seconds(mtime: std::time::SystemTime) -> u64 {
    mtime.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

/// A header's alignment within its column.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderAlign {
    Left,
    Center,
    Right,
}

/// A sortable header cell. The whole cell sorts, not just the title: the cell's
/// padding and the space beside the title are part of the target. The align
/// decides where the title and its marker sit.
fn sort_cell(
    ui: &mut egui::Ui,
    model: &mut PlanModel,
    key: SortKey,
    title: &str,
    align: HeaderAlign,
) {
    let clicked = crate::style::pointer(ui.interact(
        ui.max_rect(),
        ui.scope_id().with("sort-cell"),
        egui::Sense::click(),
    ));
    cell(ui, |ui| match align {
        HeaderAlign::Left => sort_header(ui, model, key, title, align, clicked.clicked()),
        // The title itself is centred, so the direction marker can hang beside
        // it without pushing it off centre.
        HeaderAlign::Center => {
            let width = text_width(ui, title);
            center(ui, width, |ui| sort_header(ui, model, key, title, align, clicked.clicked()));
        }
        HeaderAlign::Right => {
            right(ui, |ui| sort_header(ui, model, key, title, align, clicked.clicked()))
        }
    });
}

/// A header that sorts the view by its column, and flips the direction when it
/// is already the sort. Only the column in force carries the marker.
fn sort_header(
    ui: &mut egui::Ui,
    model: &mut PlanModel,
    key: SortKey,
    title: &str,
    align: HeaderAlign,
    cell_clicked: bool,
) {
    // No button padding: the header text lines up with the cell content, which
    // the cell's own padding already insets. No minimum height either: the whole
    // header cell is clickable, and a taller button would hang past the header
    // row and centre its text a pixel low.
    ui.style_mut().spacing.button_padding = egui::Vec2::ZERO;
    ui.style_mut().spacing.interact_size.y = 0.0;
    let active = model.sort.key == key;
    let button = egui::Button::new(egui::RichText::new(title).strong()).frame(false);
    let response = ui.add(button).on_hover_text("Sort by this column");
    if active {
        match align {
            // Added after the title, so it lands on the title's right, or on
            // its left in a right-to-left cell, where that keeps the title's
            // edge aligned with the column's right-aligned values.
            HeaderAlign::Left | HeaderAlign::Right => direction(ui, model.sort.desc),
            // The title is the centred widget, so the marker hangs beside it
            // rather than pushing the title off the column's centre.
            HeaderAlign::Center => {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(
                        response.rect.right() + ui.spacing().item_spacing.x / 2.0,
                        response.rect.center().y - ARROW_HEIGHT / 2.0,
                    ),
                    egui::vec2(ARROW_WIDTH, ARROW_HEIGHT),
                );
                paint_direction(ui, rect, model.sort.desc);
            }
        }
    }
    if cell_clicked || response.clicked() {
        model.sort = Sort { key, desc: active && !model.sort.desc };
        model.rebuild_view();
    }
}

/// The direction marker, painted rather than typed: a header falls back only on
/// the bundled fonts and the system's, and neither is sure to carry an arrow.
fn direction(ui: &mut egui::Ui, descending: bool) {
    let size = egui::vec2(ARROW_WIDTH, ARROW_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    paint_direction(ui, rect, descending);
}

fn paint_direction(ui: &egui::Ui, rect: egui::Rect, descending: bool) {
    let (up, down) = (rect.center_top(), rect.center_bottom());
    let points = match descending {
        true => vec![rect.left_top(), rect.right_top(), down],
        false => vec![rect.left_bottom(), rect.right_bottom(), up],
    };
    let shape = egui::Shape::convex_polygon(points, ui.visuals().text_color(), egui::Stroke::NONE);
    ui.painter().add(shape);
}

fn status_color(ui: &egui::Ui, status: Status) -> Color32 {
    match status {
        Status::Ok => style::ok(ui),
        Status::Unchanged => style::unchanged(ui),
        Status::Conflict(_) => style::warning(ui),
        Status::Invalid(_) => style::danger(ui),
    }
}
