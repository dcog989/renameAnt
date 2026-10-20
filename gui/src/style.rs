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
/// Space inside a panel title bar, around its title.
const HEADER_PADDING: Margin = Margin::symmetric(12, 8);
/// The window's accent: teal (#307f86). Selection, links and the panel title
/// bars are all tinted from it.
pub const ACCENT: Color32 = Color32::from_rgb(48, 127, 134);
/// The outline on a focused or selected widget: the accent lifted for contrast.
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(150, 210, 215);
/// Width of a focus outline.
const FOCUS_WIDTH: f32 = 1.0;
/// The panel title bars: the accent at low strength, so it reads as a tint
/// rather than a highlight.
const HEADER_FILL: Color32 = Color32::from_rgba_unmultiplied_const(48, 127, 134, 40);
/// The line under a title bar: the accent, a little stronger.
const HEADER_LINE_COLOR: Color32 = Color32::from_rgba_unmultiplied_const(48, 127, 134, 130);
/// Width of the line under a title bar.
const HEADER_LINE_WIDTH: f32 = 1.0;

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
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke = Stroke::new(FOCUS_WIDTH, ACCENT_LIGHT);
        style.visuals.hyperlink_color = ACCENT;
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

/// The frame for a panel: no margin, so a title bar reaches the panel's top and
/// side edges. It has no fill, so the window's opaque clear colour shows through.
pub fn panel_frame() -> Frame {
    Frame::new()
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

/// A panel's full-bleed title bar: the accent fill across the panel width, with
/// a line under it and the title padded inside. The title is monospace, like the
/// fields below it.
pub fn panel_heading(ui: &mut Ui, title: &str) {
    let bar = Frame::new().fill(HEADER_FILL).inner_margin(HEADER_PADDING).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.heading(RichText::new(title).family(FontFamily::Monospace));
    });
    let rect = bar.response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(HEADER_LINE_WIDTH, HEADER_LINE_COLOR),
    );
}
