//! The faint ant painted behind the journal when it has no history.
//!
//! egui cannot decode SVG and this build has no image loader (`egui_extras` is
//! `default-features = false`), so `logo.svg` is rasterised to raw RGBA once and
//! committed beside this file: building needs no SVG tool. Regenerate it with
//! `magick -background none logo.svg -resize 256x256 -depth 8 rgba:gui/assets/logo.rgba`.

use egui::{Color32, ColorImage, Rect, TextureHandle, TextureOptions};

/// The rasterised size, in pixels. Must match the committed asset.
const SIZE: [usize; 2] = [256, 256];
/// Raw RGBA, top row first, one byte per channel.
const RGBA: &[u8] = include_bytes!("../assets/logo.rgba");
/// A watermark, not a picture: this is all the opacity it gets. Premultiplied
/// white, so the logo keeps its own colour.
const TINT: Color32 = Color32::from_rgba_premultiplied(32, 32, 32, 32);

/// Uploads the logo once. Called from the first frame, when the texture limit is
/// known.
pub fn texture(ctx: &egui::Context) -> TextureHandle {
    let image = ColorImage::from_rgba_unmultiplied(SIZE, RGBA);
    ctx.load_texture("renameant-logo", image, TextureOptions::LINEAR)
}

/// The same pixels as the window icon. Wayland compositors ignore it and read
/// the desktop file instead; X11 sessions take it.
pub fn icon() -> egui::IconData {
    egui::IconData { rgba: RGBA.to_vec(), width: SIZE[0] as u32, height: SIZE[1] as u32 }
}

/// Paints the logo centred and faint in the area `ui` has left, before the
/// panel's own text draws over it.
pub fn paint(ui: &egui::Ui, texture: &TextureHandle) {
    let area = ui.available_rect_before_wrap();
    let rect =
        Rect::from_center_size(area.center(), egui::Vec2::splat(area.width().min(area.height())));
    ui.painter().image(
        texture.id(),
        rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        TINT,
    );
}

#[cfg(test)]
mod tests {
    /// The committed bytes and the declared size have to agree, or the texture
    /// upload is rejected at runtime.
    #[test]
    fn the_committed_pixels_match_the_declared_size() {
        assert_eq!(super::RGBA.len(), super::SIZE[0] * super::SIZE[1] * 4);
    }
}
