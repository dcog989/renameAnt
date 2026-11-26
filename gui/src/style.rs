//! The one place the window's look is defined: the palette, spacing, and the
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
/// The rule card's icon and title sizes.
pub const RULE_ICON_SIZE: f32 = 30.0;
pub const RULE_TITLE_SIZE: f32 = 16.0;
/// The primary action's text, deliberately larger than every other button.
pub const ACTION_TEXT_SIZE: f32 = 20.0;
/// Border width of every rule card, on or off, so toggling cannot resize it.
const CARD_BORDER_WIDTH: f32 = 1.5;
/// The window's accent, palette hue 1. Selection, focus and a rule that is on
/// are all tinted from it.
pub const ACCENT: Color32 = Color32::from_rgb(0xd8, 0x71, 0x29);
/// The light window's selection fill: the accent lifted so selected text reads.
pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(240, 195, 150);
/// The accent lifted further and pushed to full chroma, for a rule that is on.
pub const ACCENT_BRIGHT: Color32 = Color32::from_rgb(255, 145, 65);
/// Width of a focus outline.
const FOCUS_WIDTH: f32 = 1.0;
/// Width of the line under a title bar.
const HEADER_LINE_WIDTH: f32 = 1.0;

/// Palette hue 2: danger. `role` tints it for text; fills take it raw.
pub(crate) const DANGER: Color32 = Color32::from_rgb(0xdf, 0x65, 0x68);
/// Palette hue 3: ok.
pub(crate) const OK: Color32 = Color32::from_rgb(0x57, 0xa7, 0x4a);
/// Palette hue 4: links.
const LINK: Color32 = Color32::from_rgb(0x00, 0x9e, 0xd3);
/// How far a role's hue moves toward the page's opposite for readable text:
/// toward white on the dark window, toward black on the light one. The raw
/// swatches sit near Lc 50 as text, so they are never used directly.
const ROLE_TINT: f32 = 0.55;
/// Selected text: dark on the accent, which is light enough in either theme.
const SELECTED_TEXT: Color32 = Color32::from_rgb(43, 20, 21);
/// The text ramp: the base colour, then every weaker tier as that colour at a
/// named alpha.
const DARK_BASE: Color32 = Color32::WHITE;
const DARK_MUTED: f32 = 0.75;
const DARK_DIM: f32 = 0.55;
const DARK_DISABLED: f32 = 0.40;
const LIGHT_BASE: Color32 = Color32::BLACK;
const LIGHT_MUTED: f32 = 0.75;
const LIGHT_DIM: f32 = 0.55;
const LIGHT_DISABLED: f32 = 0.40;

/// A palette hue moved toward the theme's page for text contrast.
fn role(dark: bool, hue: Color32) -> Color32 {
    let toward = if dark { Color32::WHITE } else { Color32::BLACK };
    hue.lerp_to_gamma(toward, ROLE_TINT)
}

/// A base colour at one of the ramp's alphas.
fn alpha(color: Color32, factor: f32) -> Color32 {
    let a = (factor * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

pub(crate) fn ok(ui: &Ui) -> Color32 {
    role(ui.visuals().dark_mode, OK)
}

pub(crate) fn warning(ui: &Ui) -> Color32 {
    role(ui.visuals().dark_mode, ACCENT)
}

pub(crate) fn danger(ui: &Ui) -> Color32 {
    role(ui.visuals().dark_mode, DANGER)
}

/// "No match" is still a status the user reads, so it keeps the normal text tier.
pub(crate) fn unchanged(ui: &Ui) -> Color32 {
    ui.visuals().text_color()
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
        // The base colour is the brightest text; muted, dim and disabled are the
        // same colour at a named alpha, so no widget invents its own.
        let (base, muted, dim, disabled) = if style.visuals.dark_mode {
            (DARK_BASE, DARK_MUTED, DARK_DIM, DARK_DISABLED)
        } else {
            (LIGHT_BASE, LIGHT_MUTED, LIGHT_DIM, LIGHT_DISABLED)
        };
        let muted_color = alpha(base, muted);
        style.visuals.widgets.noninteractive.fg_stroke.color = muted_color;
        style.visuals.widgets.inactive.fg_stroke.color = muted_color;
        style.visuals.widgets.open.fg_stroke.color = muted_color;
        // Active is also what `.strong()` reads, so it keeps the base colour.
        style.visuals.widgets.active.fg_stroke.color = base;
        style.visuals.weak_text_color = Some(alpha(base, dim));
        style.visuals.hyperlink_color = role(style.visuals.dark_mode, LINK);
        // egui multiplies `disabled_alpha` into the already-muted text colour,
        // so divide the muted step out to land on the disabled tier's own alpha.
        style.visuals.disabled_alpha = disabled / muted;
        // A pressed widget fills with the accent; a hovered one takes an accent
        // outline, while its text takes the base colour.
        style.visuals.widgets.active.weak_bg_fill = ACCENT;
        style.visuals.widgets.active.bg_fill = ACCENT;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(HOVER_OUTLINE_WIDTH, ACCENT);
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(HOVER_STROKE_WIDTH, base);
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

/// The accent a rule card's border takes when the rule is on: the bright tint
/// on the dark window, the plain accent on the light one.
fn card_border(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode { ACCENT_BRIGHT } else { ACCENT }
}

/// The same accent as text: a readable shade on the light window.
fn card_accent(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode { ACCENT_BRIGHT } else { role(false, ACCENT) }
}

/// A rule card: an accent border when the rule is on, the plain group border
/// when it is off.
pub fn card_frame(ui: &Ui, enabled: bool) -> Frame {
    let frame = group_frame(ui.style());
    let color = if enabled { card_border(ui) } else { frame.stroke.color };
    frame.stroke(Stroke::new(CARD_BORDER_WIDTH, color))
}

/// A rule card's icon and title: accent when the rule is on, weak when it is off.
pub fn card_text(ui: &Ui, enabled: bool) -> Color32 {
    if enabled { card_accent(ui) } else { ui.visuals().weak_text_color() }
}

/// A text field: monospace, so every field reads as one and no call site
/// styles its own.
pub fn field(ui: &mut Ui, text: &mut String, width: f32) -> Response {
    ui.add(TextEdit::singleline(text).desired_width(width).font(TextStyle::Monospace))
}

/// A dropdown: one shape for every combo in the window, with the pointer cursor
/// the combo itself does not set.
pub fn combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T,
    all: impl IntoIterator<Item = T>,
    label: impl Fn(T) -> &'static str,
) -> Response {
    let selected = label(*value);
    let response = egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for option in all {
                ui.selectable_value(value, option, label(option));
            }
        })
        .response;
    pointer(response)
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

/// How far the danger hue is darkened for the remove glyph's hover chip, so a
/// destructive control is unmistakable.
const REMOVE_HOVER_SHADE: f32 = 0.55;
/// The chip hugs its glyph rather than its cell, so it fits a table row or a
/// journal line.
pub const REMOVE_CHIP: f32 = 18.0;
/// Rounding on the chip, close to the theme's button radius.
const REMOVE_CHIP_CORNER: u8 = 4;

/// A square remove chip: the same target and look in the table, the journal and
/// the rule editor. `name` is what a screen reader announces; the caller
/// supplies the hover text.
pub fn remove(ui: &mut Ui, name: &str) -> Response {
    let (_, rect) = ui.allocate_space(Vec2::splat(REMOVE_CHIP));
    // A real widget, not painted text, so the action can be focused and clicked
    // from the keyboard. A disabled `ui` also disables the click. The unique id,
    // not the scope id: sibling journal rows share a scope, and one id across
    // three chips makes any hover or click apply to all of them.
    let chip = ui.interact(rect, ui.unique_id().with("remove"), egui::Sense::click());
    if chip.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(REMOVE_CHIP_CORNER),
            DANGER.gamma_multiply(REMOVE_HOVER_SHADE),
        );
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
    chip.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), name));
    pointer(chip)
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
        ui.heading(RichText::new(title).family(FontFamily::Monospace).strong());
    });
    let rect = bar.response.rect;
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(HEADER_LINE_WIDTH, line));
}
