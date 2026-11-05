//! The screen for fitting a picture to a key: drag to move it, scroll or slide to resize it.

use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, Vec2};
use image::{RgbaImage, imageops};
use pyroporch_core::paths::Paths;
use pyroporch_core::transfer::store_image;

/// Pictures are stored at the size recommended for key artwork, which is above every deck's own
/// keys (72 to 120 pixels across). Checked against the deck maker's icon guide on 2026-11-02.
pub const KEY_IMAGE_SIZE: u32 = 144;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SIDE: u32 = 16_384;
/// How much room a picture may need to be decoded. Bigger than that is refused.
const MAX_ALLOC: u64 = 128 * 1024 * 1024;
/// Big enough to zoom into, small enough to stay quick and fit any graphics card.
const WORKING_SIDE: u32 = 2048;
const FRAME: f32 = 320.0;
const PREVIEW: f32 = 96.0;
const MAX_ZOOM: f32 = 6.0;

/// Where a picture sits in the square frame, in frame units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub scale: f32,
    pub pos: [f32; 2],
}

/// The scale at which the picture just covers the frame.
fn cover_scale(width: u32, height: u32) -> f32 {
    FRAME / width.min(height) as f32
}

/// The scale at which the whole picture just fits inside the frame.
fn contain_scale(width: u32, height: u32) -> f32 {
    FRAME / width.max(height) as f32
}

impl Placement {
    /// Fills the frame, centred.
    pub fn cover(width: u32, height: u32) -> Self {
        let scale = cover_scale(width, height);
        Self::centered(width, height, scale)
    }

    /// Shows all of the picture, centred.
    pub fn contain(width: u32, height: u32) -> Self {
        let scale = contain_scale(width, height);
        Self::centered(width, height, scale)
    }

    fn centered(width: u32, height: u32, scale: f32) -> Self {
        Self {
            scale,
            pos: [(FRAME - width as f32 * scale) / 2.0, (FRAME - height as f32 * scale) / 2.0],
        }
    }

    /// Keeps the picture from sliding away: a side larger than the frame always covers it, and a
    /// smaller side stays inside it.
    pub fn clamped(mut self, width: u32, height: u32) -> Self {
        let min = contain_scale(width, height);
        let max = cover_scale(width, height) * MAX_ZOOM;
        self.scale = self.scale.clamp(min, max);
        for (axis, size) in [(0, width), (1, height)] {
            let extent = size as f32 * self.scale;
            let (low, high) = if extent >= FRAME { (FRAME - extent, 0.0) } else { (0.0, FRAME - extent) };
            self.pos[axis] = self.pos[axis].clamp(low, high);
        }
        self
    }

    /// Changes the size while keeping the same part of the picture under `anchor`.
    pub fn zoomed(self, scale: f32, anchor: [f32; 2], width: u32, height: u32) -> Self {
        let ratio = scale / self.scale;
        Self {
            scale,
            pos: [
                anchor[0] - (anchor[0] - self.pos[0]) * ratio,
                anchor[1] - (anchor[1] - self.pos[1]) * ratio,
            ],
        }
        .clamped(width, height)
    }
}

/// Makes the key's picture. Anywhere the picture doesn't reach stays see-through, so the key's
/// colour shows there.
pub fn render(source: &RgbaImage, placement: Placement, output: u32) -> RgbaImage {
    let factor = output as f32 / FRAME;
    let scale = placement.scale.max(0.0001);
    // Only the part of the picture that lands inside the frame is needed. Taking that part
    // first keeps the resized copy no bigger than the frame, however far the picture is zoomed in.
    let visible = |start: f32, size: u32| -> (u32, u32) {
        let from = (-start / scale).floor().max(0.0) as u32;
        let to = ((FRAME - start) / scale).ceil().max(0.0) as u32;
        let from = from.min(size);
        (from, to.clamp(from, size) - from)
    };
    let (x0, wide) = visible(placement.pos[0], source.width());
    let (y0, tall) = visible(placement.pos[1], source.height());
    let mut canvas = RgbaImage::new(output, output);
    if wide == 0 || tall == 0 {
        return canvas;
    }
    let part = imageops::crop_imm(source, x0, y0, wide, tall).to_image();
    let width = ((wide as f32 * scale * factor).round() as u32).clamp(1, output.max(1));
    let height = ((tall as f32 * scale * factor).round() as u32).clamp(1, output.max(1));
    let resized = imageops::resize(&part, width, height, imageops::FilterType::Triangle);
    imageops::overlay(
        &mut canvas,
        &resized,
        ((placement.pos[0] + x0 as f32 * scale) * factor).round() as i64,
        ((placement.pos[1] + y0 as f32 * scale) * factor).round() as i64,
    );
    canvas
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// What opening a picture leads to.
pub enum Opened {
    /// Already fits: square and no bigger than a key picture.
    Ready(String),
    /// Needs fitting first.
    Fit(RgbaImage),
}

/// Reads a picture with limits, so a huge or malformed file can't take the app down.
pub fn open_picture(paths: &Paths, path: &std::path::Path) -> Result<Opened, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("couldn't read that file: {e}"))?.len();
    if size > MAX_FILE_BYTES {
        return Err("that picture is too big. Use one under 64 MB.".into());
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("couldn't read that file: {e}"))?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    // A picture that would need more room than this to be decoded is refused, rather than
    // taking the app's memory with it.
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| "that file isn't a PNG or JPEG picture".to_string())?;
    let picture = decoded.to_rgba8();
    let (width, height) = picture.dimensions();
    if width == height && width <= KEY_IMAGE_SIZE {
        return store_image(paths, &encode_png(&picture)?).map(Opened::Ready);
    }
    let working = if width.max(height) > WORKING_SIDE {
        decoded.thumbnail(WORKING_SIDE, WORKING_SIDE).to_rgba8()
    } else {
        picture
    };
    Ok(Opened::Fit(working))
}

pub struct CropDialog {
    source: RgbaImage,
    texture: TextureHandle,
    pub placement: Placement,
    /// The key this is for.
    pub slot: (u32, u32),
    pub error: String,
}

pub enum CropResult {
    Open,
    Cancelled,
    Done(RgbaImage),
}

impl CropDialog {
    pub fn new(ctx: &egui::Context, source: RgbaImage, slot: (u32, u32)) -> Self {
        let size = [source.width() as usize, source.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, source.as_raw());
        let texture = ctx.load_texture("crop-source", color, egui::TextureOptions::LINEAR);
        let placement = Placement::cover(source.width(), source.height());
        Self {
            source,
            texture,
            placement,
            slot,
            error: String::new(),
        }
    }

    fn paint(&self, ui: &egui::Ui, frame: Rect, zoom: f32, radius: u8, tint: Color32) {
        let painter = ui.painter_at(frame);
        painter.rect_filled(frame, CornerRadius::same(radius), tint);
        let scale = self.placement.scale * zoom;
        let min = frame.min + Vec2::new(self.placement.pos[0], self.placement.pos[1]) * zoom;
        let size = Vec2::new(self.source.width() as f32, self.source.height() as f32) * scale;
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(self.texture.id(), Rect::from_min_size(min, size), uv, Color32::WHITE);
    }

    pub fn show(&mut self, ctx: &egui::Context, accent: Color32, key_color: Color32, muted: Color32) -> CropResult {
        let (width, height) = self.source.dimensions();
        let mut result = CropResult::Open;
        egui::Modal::new(egui::Id::unique("crop_dialog")).show(ctx, |ui| {
            ui.set_width(FRAME + PREVIEW + 72.0);
            ui.heading("Fit the picture to the key");
            ui.label(RichText::new(format!("Your picture is {width} × {height}. Drag to move it, and scroll or use the slider to resize it.")).color(muted));
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                let (frame, response) = ui.allocate_exact_size(Vec2::splat(FRAME), Sense::drag());
                if response.dragged() {
                    let delta = response.drag_delta();
                    self.placement.pos[0] += delta.x;
                    self.placement.pos[1] += delta.y;
                    self.placement = self.placement.clamped(width, height);
                }
                let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                if response.hovered() && scroll != 0.0 {
                    let pointer = ui.input(|input| input.pointer.hover_pos()).unwrap_or(frame.center());
                    let anchor = pointer - frame.min;
                    let scale = self.placement.scale * (scroll * 0.003).exp();
                    self.placement = self.placement.zoomed(scale, [anchor.x, anchor.y], width, height);
                }
                self.paint(ui, frame, 1.0, 0, key_color);
                ui.painter().rect_stroke(frame, 0, Stroke::new(2.0, accent), StrokeKind::Outside);

                ui.vertical(|ui| {
                    ui.label(RichText::new("On the key").small().color(muted));
                    let (preview, _) = ui.allocate_exact_size(Vec2::splat(PREVIEW), Sense::hover());
                    self.paint(ui, preview, PREVIEW / FRAME, 14, key_color);
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("Saved at {KEY_IMAGE_SIZE} × {KEY_IMAGE_SIZE}, so it's sharp on every deck.")).small().color(muted));
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = ui.label("Size");
                let (min, max) = (contain_scale(width, height), cover_scale(width, height) * MAX_ZOOM);
                let mut scale = self.placement.scale;
                let slider = egui::Slider::new(&mut scale, min..=max).show_value(false).logarithmic(true);
                if ui.add_sized([180.0, 20.0], slider).labelled_by(label.id).changed() {
                    let center = FRAME / 2.0;
                    self.placement = self.placement.zoomed(scale, [center, center], width, height);
                }
                if ui.button("Fill the key").clicked() {
                    self.placement = Placement::cover(width, height);
                }
                if ui.button("Show it all").clicked() {
                    self.placement = Placement::contain(width, height);
                }
            });
            if !self.error.is_empty() {
                ui.colored_label(Color32::from_rgb(0xEC, 0x5F, 0x5F), &self.error);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Use this picture").color(Color32::WHITE)).fill(accent)).clicked() {
                    result = CropResult::Done(render(&self.source, self.placement, KEY_IMAGE_SIZE));
                }
                if ui.button("Cancel").clicked() {
                    result = CropResult::Cancelled;
                }
            });
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Left half red, right half blue.
    fn split_picture(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, _| {
            if x < width / 2 { Rgba([255, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) }
        })
    }

    #[test]
    fn a_wide_picture_fills_the_key_and_can_be_moved() {
        let source = split_picture(800, 400);
        let centered = Placement::cover(800, 400);
        let image = render(&source, centered, KEY_IMAGE_SIZE);
        assert_eq!(image.dimensions(), (KEY_IMAGE_SIZE, KEY_IMAGE_SIZE));
        assert_eq!(image.get_pixel(10, 72).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(134, 72).0, [0, 0, 255, 255]);

        // Dragged all the way right: only the left of the picture shows.
        let moved = Placement { pos: [10_000.0, 0.0], ..centered }.clamped(800, 400);
        assert_eq!(moved.pos[0], 0.0);
        let image = render(&source, moved, KEY_IMAGE_SIZE);
        assert_eq!(image.get_pixel(134, 72).0, [255, 0, 0, 255]);
    }

    #[test]
    fn showing_it_all_leaves_the_rest_see_through() {
        let source = split_picture(800, 400);
        let image = render(&source, Placement::contain(800, 400), KEY_IMAGE_SIZE);
        assert_eq!(image.get_pixel(72, 4).0[3], 0, "above the picture");
        assert_eq!(image.get_pixel(72, 72).0[3], 255, "the picture itself");
    }

    #[test]
    fn it_cannot_be_dragged_out_of_view_or_zoomed_without_limit() {
        let wild = Placement { scale: 1000.0, pos: [-99_999.0, 99_999.0] }.clamped(800, 400);
        assert!(wild.scale <= cover_scale(800, 400) * MAX_ZOOM);
        assert!(wild.pos[0] >= FRAME - 800.0 * wild.scale);
        assert_eq!(wild.pos[1], 0.0);
        let tiny = Placement { scale: 0.000_01, pos: [0.0, 0.0] }.clamped(800, 400);
        assert_eq!(tiny.scale, contain_scale(800, 400));
    }

    #[test]
    fn zooming_keeps_the_same_spot_under_the_pointer() {
        let start = Placement::cover(1000, 1000);
        let zoomed = start.zoomed(start.scale * 2.0, [160.0, 160.0], 1000, 1000);
        assert_eq!(zoomed.pos, [-160.0, -160.0]);
    }

    #[test]
    fn small_square_pictures_are_used_directly_and_big_ones_need_fitting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        paths.ensure().unwrap();
        let (small, big, junk) = (dir.path().join("s.png"), dir.path().join("b.png"), dir.path().join("j.png"));
        split_picture(96, 96).save(&small).unwrap();
        split_picture(600, 200).save(&big).unwrap();
        std::fs::write(&junk, "not a picture").unwrap();

        match open_picture(&paths, &small).unwrap() {
            Opened::Ready(name) => assert!(paths.image(&name).unwrap().exists()),
            Opened::Fit(_) => panic!("a 96 × 96 picture already fits"),
        }
        assert!(matches!(open_picture(&paths, &big).unwrap(), Opened::Fit(_)));
        assert!(open_picture(&paths, &junk).is_err());
    }
}
