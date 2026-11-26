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
/// The rule card's icon and title sizes.
pub const RULE_ICON_SIZE: f32 = 30.0;
pub const RULE_TITLE_SIZE: f32 = 16.0;
/// The primary action's text, deliberately larger than every other button.
pub const ACTION_TEXT_SIZE: f32 = 20.0;
/// Border width of every rule card, on or off, so toggling cannot resize it.
const CARD_BORDER_WIDTH: f32 = 1.5;
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
/// The three text tiers are one colour at a named opacity: bright at 100%,
/// dim and disabled below it, about 35 APCA Lc apart; bright clears Lc 75.
/// A saturated link blue cannot reach 75 on near-black, so links keep their
/// hue near 60.
const TEXT_DARK: Color32 = Color32::WHITE;
const TEXT_LIGHT: Color32 = Color32::from_gray(30);
/// Dim lands near gray 198 (dark) and 121 (light).
const DIM_ALPHA_DARK: f32 = 0.75;
const DIM_ALPHA_LIGHT: f32 = 0.58;
/// Disabled lands near gray 134 (dark) and 187 (light).
const DISABLED_ALPHA_DARK: f32 = 0.47;
const DISABLED_ALPHA_LIGHT: f32 = 0.28;
const LINK_DARK: Color32 = Color32::from_rgb(60, 190, 255);
const LINK_LIGHT: Color32 = Color32::from_rgb(0, 130, 230);

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
        // Text colours are the theme's, so every panel and the About page read
        // the same and meet APCA.
        let dark = style.visuals.dark_mode;
        let text = if dark { TEXT_DARK } else { TEXT_LIGHT };
        style.visuals.widgets.noninteractive.fg_stroke.color = text;
        let dim = if dark { DIM_ALPHA_DARK } else { DIM_ALPHA_LIGHT };
        style.visuals.weak_text_color = Some(text.gamma_multiply(dim));
        style.visuals.hyperlink_color = if dark { LINK_DARK } else { LINK_LIGHT };
        style.visuals.disabled_alpha =
            if dark { DISABLED_ALPHA_DARK } else { DISABLED_ALPHA_LIGHT };
        // A pressed widget fills with the accent; a hovered one takes an accent
        // outline, while its text keeps the theme's APCA-checked colour.
        style.visuals.widgets.active.weak_bg_fill = ACCENT;
        style.visuals.widgets.active.bg_fill = ACCENT;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(HOVER_OUTLINE_WIDTH, ACCENT);
        let hover_text = if dark { TEXT_DARK } else { TEXT_LIGHT };
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(HOVER_STROKE_WIDTH, hover_text);
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

/// A rule card: an accent border when the rule is on, the plain group border
/// when it is off.
pub fn card_frame(ui: &Ui, enabled: bool) -> Frame {
    let frame = group_frame(ui.style());
    let color = if enabled { ACCENT_BRIGHT } else { frame.stroke.color };
    frame.stroke(Stroke::new(CARD_BORDER_WIDTH, color))
}

/// A rule card's icon and title: accent when the rule is on, weak when it is off.
pub fn card_text(ui: &Ui, enabled: bool) -> Color32 {
    if enabled { ACCENT_BRIGHT } else { ui.visuals().weak_text_color() }
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

/// The remove glyph's hover chip: #873731, so a destructive control is
/// unmistakable.
const REMOVE_HOVER: Color32 = Color32::from_rgb(0x87, 0x37, 0x31);
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
        ui.heading(RichText::new(title).family(FontFamily::Monospace));
    });
    let rect = bar.response.rect;
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(HEADER_LINE_WIDTH, line));
}
