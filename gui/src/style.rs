//! The one place the window's look is defined: spacing, the accent, and the
//! two things that are always monospace (panel titles and text fields).
//!
//! egui's defaults are tight (3 pt between rows, 2 pt of button padding), and
//! its panel and group frames hard-code their own margins instead of reading
//! [`egui::Style::spacing`]. Every form widget, panel and card inherits what is
//! set once here, so the spacing stays consistent without per-panel tuning.

use egui::{
    Color32, Context, CursorIcon, FontFamily, Frame, Margin, Response, RichText, Stroke, Style,
    TextEdit, TextStyle, Ui, Vec2,
};

/// Space between widgets in a row or column.
const ITEM_SPACING: Vec2 = Vec2::new(10.0, 8.0);
/// Space inside a button, around its text.
pub const BUTTON_PADDING: Vec2 = Vec2::new(12.0, 6.0);
/// Minimum interactive size, so fields, buttons and drag values are tall enough.
const INTERACT_SIZE: Vec2 = Vec2::new(40.0, 26.0);
/// Margin inside a window, panel or card.
const MARGIN: i8 = 10;
/// Margin inside a menu.
const MENU_MARGIN: i8 = 8;
/// Inset on both sides of a table cell, so every column's content lines up.
pub const CELL_PADDING: i8 = 6;
/// Space inside a panel title bar, around its title.
const HEADER_PADDING: Margin = Margin::symmetric(12, 8);
/// The window's accent: orange (#d87129). Selection, links and the panel title
/// bars are all tinted from it.
pub const ACCENT: Color32 = Color32::from_rgb(216, 113, 41);
/// The outline on a focused or selected widget: the accent lifted for contrast.
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(240, 195, 150);
/// The accent lifted further and pushed to full chroma, for a rule that is on.
pub const ACCENT_BRIGHT: Color32 = Color32::from_rgb(255, 145, 65);
/// Width of a focus outline.
const FOCUS_WIDTH: f32 = 1.0;
/// Width of the line under a title bar.
const HEADER_LINE_WIDTH: f32 = 1.0;

/// Status colours for the dark theme: light enough for a near-black window.
const OK_DARK: Color32 = Color32::from_rgb(150, 224, 160);
const WARNING_DARK: Color32 = Color32::from_rgb(255, 205, 120);
const DANGER_DARK: Color32 = Color32::from_rgb(255, 185, 175);
/// The same meanings on a light window, darkened for APCA text contrast.
const OK_LIGHT: Color32 = Color32::from_rgb(27, 94, 32);
const WARNING_LIGHT: Color32 = Color32::from_rgb(122, 74, 0);
const DANGER_LIGHT: Color32 = Color32::from_rgb(164, 34, 26);
/// "no match" is still a status the user reads, so it gets a theme pair too.
const UNCHANGED_DARK: Color32 = Color32::from_rgb(192, 192, 192);
const UNCHANGED_LIGHT: Color32 = Color32::from_rgb(95, 99, 104);
/// Selected text: dark on the accent, which is light enough in either theme.
const SELECTED_TEXT: Color32 = Color32::from_rgb(43, 20, 21);

fn themed(ui: &Ui, light: Color32, dark: Color32) -> Color32 {
    if ui.visuals().dark_mode { dark } else { light }
}

pub(crate) fn ok(ui: &Ui) -> Color32 {
    themed(ui, OK_LIGHT, OK_DARK)
}

pub(crate) fn warning(ui: &Ui) -> Color32 {
    themed(ui, WARNING_LIGHT, WARNING_DARK)
}

pub(crate) fn danger(ui: &Ui) -> Color32 {
    themed(ui, DANGER_LIGHT, DANGER_DARK)
}

pub(crate) fn unchanged(ui: &Ui) -> Color32 {
    themed(ui, UNCHANGED_LIGHT, UNCHANGED_DARK)
}

/// Width of the outline on a hovered widget.
const HOVER_OUTLINE_WIDTH: f32 = 1.0;
/// Width of a hovered widget's own stroke: its text or drag handle.
const HOVER_STROKE_WIDTH: f32 = 1.5;

/// Applies the base spacing and accent to both themes, once, at startup.
pub fn apply(ctx: &Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = ITEM_SPACING;
        style.spacing.button_padding = BUTTON_PADDING;
        style.spacing.interact_size = INTERACT_SIZE;
        style.spacing.window_margin = Margin::same(MARGIN);
        style.spacing.menu_margin = Margin::same(MENU_MARGIN);
        style.visuals.selection.bg_fill =
            if style.visuals.dark_mode { ACCENT } else { ACCENT_LIGHT };
        style.visuals.selection.stroke = Stroke::new(FOCUS_WIDTH, SELECTED_TEXT);
        // A pressed widget fills with the accent; a hovered one takes an accent
        // outline and, for text and drag handles, an accent stroke.
        style.visuals.widgets.active.weak_bg_fill = ACCENT;
        style.visuals.widgets.active.bg_fill = ACCENT;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(HOVER_OUTLINE_WIDTH, ACCENT);
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(HOVER_STROKE_WIDTH, ACCENT_LIGHT);
        // Every button reads this, so a clickable control reads as one.
        style.visuals.interact_cursor = Some(CursorIcon::PointingHand);
    });
}

/// The frame for everything below a panel's title bar, padded like a window.
pub fn panel_body(style: &Style) -> Frame {
    Frame::new().inner_margin(style.spacing.window_margin)
}

/// The frame for a grouped card, padded like a window.
pub fn group_frame(style: &Style) -> Frame {
    Frame::group(style).inner_margin(style.spacing.window_margin)
}

/// A text field: monospace, so every field reads as one and no call site
/// styles its own.
pub fn field(ui: &mut Ui, text: &mut String, width: f32) -> Response {
    ui.add(TextEdit::singleline(text).desired_width(width).font(TextStyle::Monospace))
}

/// A menu's contents. egui's own menu style flattens the button padding to
/// (2, 0), which reads as no padding beside the window's controls.
pub fn menu(ui: &mut Ui) {
    ui.style_mut().spacing.button_padding = BUTTON_PADDING;
}

/// Give a clickable widget the hand cursor. egui's [`egui::Button`] reads
/// [`egui::Visuals::interact_cursor`] itself, but checkboxes and combo boxes do
/// not.
pub fn pointer(response: Response) -> Response {
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// The remove glyph's hover chip: #873731, so a destructive control is
/// unmistakable.
const REMOVE_HOVER: Color32 = Color32::from_rgb(0x87, 0x37, 0x31);
/// The chip hugs its glyph rather than its cell, so it fits a table row or a
/// journal line.
pub const REMOVE_CHIP: f32 = 18.0;
/// Rounding on the chip, close to the theme's button radius.
const REMOVE_CHIP_CORNER: u8 = 4;

/// A square remove chip: the same target and look in the table and the journal.
/// The caller supplies the hover text and the accessible name.
pub fn remove_chip(ui: &mut Ui) -> Response {
    let (_, rect) = ui.allocate_space(Vec2::splat(REMOVE_CHIP));
    // A real widget, not painted text, so the action can be focused and clicked
    // from the keyboard. A disabled `ui` also disables the click. The unique id,
    // not the scope id: sibling journal rows share a scope, and one id across
    // three chips makes any hover or click apply to all of them.
    let chip = ui.interact(rect, ui.unique_id().with("remove"), egui::Sense::click());
    if chip.hovered() {
        ui.painter().rect_filled(rect, egui::CornerRadius::same(REMOVE_CHIP_CORNER), REMOVE_HOVER);
    }
    let color = if chip.hovered() {
        Color32::WHITE
    } else if ui.is_enabled() {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "✕",
        TextStyle::Body.resolve(ui.style()),
        color,
    );
    chip.on_hover_cursor(CursorIcon::PointingHand)
}

/// A panel's full-bleed title bar: a faint neutral fill across the panel width,
/// with a line under it and the title padded inside. The title is monospace,
/// like the fields below it.
pub fn panel_heading(ui: &mut Ui, title: &str) {
    let (fill, line) = {
        let visuals = ui.visuals();
        (visuals.faint_bg_color, visuals.widgets.noninteractive.bg_stroke.color)
    };
    let bar = Frame::new().fill(fill).inner_margin(HEADER_PADDING).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.heading(RichText::new(title).family(FontFamily::Monospace));
    });
    let rect = bar.response.rect;
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(HEADER_LINE_WIDTH, line));
}
