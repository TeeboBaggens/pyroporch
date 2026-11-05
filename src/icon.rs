//! The app's icons, drawn in code: a lantern hanging from a porch beam, with a flame inside. No
//! image files to ship.

use image::{Rgba, RgbaImage};
use std::sync::LazyLock;

/// How many samples across each pixel, for smooth edges.
const SAMPLES: u32 = 3;

type Rgb = [f32; 3];
type Color = [f32; 4];

const CLEAR: Color = [0.0; 4];
const WOOD_LIGHT: Rgb = [0.70, 0.42, 0.23];
const WOOD_DARK: Rgb = [0.43, 0.24, 0.13];
const BEAM_SHADOW: Rgb = [0.27, 0.15, 0.10];
const ASH_LIGHT: Rgb = [0.70, 0.69, 0.68];
const ASH_DARK: Rgb = [0.47, 0.46, 0.46];
const EMBER: Rgb = [0.96, 0.42, 0.10];
const FIRELIGHT: Rgb = [1.0, 0.62, 0.22];
const LANTERN_METAL: Rgb = [0.47, 0.36, 0.30];
const MARK_COLOR: Rgb = [0.80, 0.50, 0.27];

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn over(under: Color, top: Color) -> Color {
    let alpha = top[3] + under[3] * (1.0 - top[3]);
    if alpha <= 0.0 {
        return CLEAR;
    }
    let channel = |i: usize| (top[i] * top[3] + under[i] * under[3] * (1.0 - top[3])) / alpha;
    [channel(0), channel(1), channel(2), alpha]
}

fn put(color: &mut Color, when: bool, paint: Rgb) {
    if when {
        *color = [paint[0], paint[1], paint[2], 1.0];
    }
}

fn rect(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32) -> bool {
    (left..=right).contains(&x) && (top..=bottom).contains(&y)
}

fn circle(x: f32, y: f32, cx: f32, cy: f32, radius: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

fn polygon(x: f32, y: f32, points: &[[f32; 2]]) -> bool {
    let Some(mut previous) = points.last().copied() else {
        return false;
    };
    let mut inside = false;
    for &point in points {
        if (point[1] > y) != (previous[1] > y) && x < previous[0] + (y - previous[1]) / (point[1] - previous[1]) * (point[0] - previous[0]) {
            inside = !inside;
        }
        previous = point;
    }
    inside
}

fn in_tile(x: f32, y: f32) -> bool {
    let (left, top, right, bottom, radius) = (0.08, 0.08, 0.92, 0.92, 0.19);
    circle(x, y, x.clamp(left + radius, right - radius), y.clamp(top + radius, bottom - radius), radius)
}

/// How strongly a light at a point reaches `x`, `y`: 1 at the light, 0 at `radius`.
fn light(x: f32, y: f32, cx: f32, cy: f32, radius: f32) -> f32 {
    let (dx, dy) = (x - cx, y - cy);
    (1.0 - (dx * dx + dy * dy).sqrt() / radius).clamp(0.0, 1.0).powi(2)
}

/// A closed outline made of curves, flattened to points once.
fn flatten(start: [f32; 2], curves: &[[[f32; 2]; 3]]) -> Vec<[f32; 2]> {
    let mut points = vec![start];
    let mut from = start;
    for [a, b, to] in curves.iter().copied() {
        for step in 1..=16 {
            let t = step as f32 / 16.0;
            let u = 1.0 - t;
            let along = |i: usize| u * u * u * from[i] + 3.0 * u * u * t * a[i] + 3.0 * u * t * t * b[i] + t * t * t * to[i];
            points.push([along(0), along(1)]);
        }
        from = to;
    }
    points
}

/// A flame with a main tongue, leaning a little right, and a smaller one on each side.
static FLAME_OUTER: LazyLock<Vec<[f32; 2]>> = LazyLock::new(|| {
    flatten([0.50, 1.00], &[
        [[0.18, 1.00], [0.02, 0.80], [0.06, 0.58]],
        [[0.08, 0.46], [0.15, 0.38], [0.18, 0.22]],
        [[0.25, 0.28], [0.31, 0.37], [0.34, 0.44]],
        [[0.33, 0.27], [0.42, 0.11], [0.60, 0.00]],
        [[0.56, 0.16], [0.62, 0.28], [0.72, 0.38]],
        [[0.75, 0.33], [0.78, 0.28], [0.80, 0.20]],
        [[0.92, 0.36], [1.00, 0.56], [0.95, 0.72]],
        [[0.90, 0.89], [0.74, 1.00], [0.50, 1.00]],
    ])
});

static FLAME_MIDDLE: LazyLock<Vec<[f32; 2]>> = LazyLock::new(|| {
    flatten([0.50, 1.00], &[
        [[0.26, 1.00], [0.14, 0.85], [0.18, 0.68]],
        [[0.21, 0.58], [0.29, 0.53], [0.32, 0.45]],
        [[0.36, 0.50], [0.40, 0.55], [0.43, 0.59]],
        [[0.43, 0.46], [0.47, 0.36], [0.57, 0.27]],
        [[0.56, 0.40], [0.64, 0.50], [0.71, 0.58]],
        [[0.73, 0.55], [0.75, 0.52], [0.76, 0.49]],
        [[0.86, 0.64], [0.88, 0.76], [0.81, 0.86]],
        [[0.75, 0.95], [0.64, 1.00], [0.50, 1.00]],
    ])
});

static FLAME_CORE: LazyLock<Vec<[f32; 2]>> = LazyLock::new(|| {
    flatten([0.50, 1.00], &[
        [[0.34, 1.00], [0.27, 0.88], [0.31, 0.77]],
        [[0.35, 0.68], [0.46, 0.63], [0.52, 0.50]],
        [[0.55, 0.62], [0.67, 0.68], [0.70, 0.79]],
        [[0.73, 0.90], [0.64, 1.00], [0.50, 1.00]],
    ])
});

/// Paints a flame, centred on the middle of the picture, between `tip` and `base`.
fn flame(color: &mut Color, x: f32, y: f32, base: f32, tip: f32, width: f32) {
    let u = (x - 0.5 + width / 2.0) / width;
    let v = (y - tip) / (base - tip);
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) || !polygon(u, v, &FLAME_OUTER) {
        return;
    }
    let paint = if polygon(u, v, &FLAME_CORE) {
        mix([1.0, 0.86, 0.38], [1.0, 0.97, 0.72], (v - 0.5) * 2.0)
    } else if polygon(u, v, &FLAME_MIDDLE) {
        mix([0.99, 0.48, 0.10], [1.0, 0.72, 0.18], (v - 0.27) * 1.6)
    } else {
        mix([0.82, 0.13, 0.10], [0.96, 0.33, 0.08], v * 1.3)
    };
    put(color, true, paint);
}

/// A fire that's gone out: a few embers and a thread of smoke, thinning as it rises.
fn embers_and_smoke(color: &mut Color, x: f32, y: f32, base: f32, top: f32, size: f32) {
    if (top..=base).contains(&y) {
        let height = (base - y) / (base - top);
        let middle = 0.5 + size * 0.42 * (height * std::f32::consts::PI * 2.4).sin();
        if (x - middle).abs() <= size * 0.30 * (1.0 - height * 0.6) {
            *color = over(*color, [0.84, 0.85, 0.88, 0.9 * (1.0 - height * 0.7)]);
        }
    }
    for (dx, dy, radius) in [(-0.62, 0.0, 0.24), (0.0, -0.08, 0.30), (0.62, 0.0, 0.20)] {
        put(color, circle(x, y, 0.5 + dx * size, base + dy, radius * size), EMBER);
    }
}

/// The colour at one point, with `x` and `y` running 0 to 1 across the square. Lit, it's the app
/// icon on its tile. Unlit, the flame has gone out and the background stays clear, so a key's own
/// colour shows through.
fn lantern(x: f32, y: f32, lit: bool) -> Color {
    let mut color = CLEAR;
    if lit {
        if !in_tile(x, y) {
            return CLEAR;
        }
        let back = mix([0.15, 0.10, 0.11], [0.09, 0.09, 0.15], (y - 0.08) / 0.84);
        color = [back[0], back[1], back[2], 1.0];
        let glow = light(x, y, 0.5, 0.60, 0.50) * 0.65;
        color = over(color, [FIRELIGHT[0], FIRELIGHT[1], FIRELIGHT[2], glow]);
    }
    // The glass, and what's inside it.
    if rect(x, y, 0.35, 0.40, 0.65, 0.75) {
        color = over(color, if lit { [1.0, 0.72, 0.30, 0.34] } else { [0.8, 0.8, 0.82, 0.20] });
    }
    if lit {
        flame(&mut color, x, y, 0.74, 0.44, 0.215);
    } else {
        // Big enough to make out on a real key, where the icon is only about 50 pixels across.
        embers_and_smoke(&mut color, x, y, 0.72, 0.44, 0.09);
    }
    // The beam it hangs from.
    let beam = if lit { y <= 0.205 } else { rect(x, y, 0.20, 0.13, 0.80, 0.205) };
    let timber = if lit {
        mix(mix(WOOD_DARK, WOOD_LIGHT, 0.25), FIRELIGHT, light(x, y, 0.5, 0.40, 0.55) * 0.75)
    } else {
        mix(ASH_LIGHT, ASH_DARK, x)
    };
    put(&mut color, beam, timber);
    put(&mut color, lit && (0.205..=0.222).contains(&y), BEAM_SHADOW);
    // The hanger and the lantern's frame.
    let metal = if lit { LANTERN_METAL } else { ASH_LIGHT };
    let ring = circle(x, y, 0.5, 0.29, 0.042) && !circle(x, y, 0.5, 0.29, 0.02);
    put(&mut color, ring || rect(x, y, 0.488, 0.222, 0.512, 0.25), metal);
    put(&mut color, polygon(x, y, &[[0.42, 0.325], [0.58, 0.325], [0.685, 0.405], [0.315, 0.405]]), metal);
    put(&mut color, rect(x, y, 0.335, 0.40, 0.365, 0.76) || rect(x, y, 0.635, 0.40, 0.665, 0.76), metal);
    put(&mut color, rect(x, y, 0.315, 0.75, 0.685, 0.795), metal);
    put(&mut color, rect(x, y, 0.40, 0.795, 0.60, 0.82), metal);
    color
}

/// A simpler lantern for the menu bar, where it's tiny and may be a single flat colour.
fn lantern_mark(x: f32, y: f32) -> Color {
    let mut color = CLEAR;
    flame(&mut color, x, y, 0.83, 0.33, 0.36);
    let ring = circle(x, y, 0.5, 0.09, 0.08) && !circle(x, y, 0.5, 0.09, 0.035);
    put(&mut color, ring, MARK_COLOR);
    put(&mut color, polygon(x, y, &[[0.36, 0.13], [0.64, 0.13], [0.86, 0.27], [0.14, 0.27]]), MARK_COLOR);
    put(&mut color, rect(x, y, 0.18, 0.27, 0.28, 0.90) || rect(x, y, 0.72, 0.27, 0.82, 0.90), MARK_COLOR);
    put(&mut color, rect(x, y, 0.14, 0.88, 0.86, 0.98), MARK_COLOR);
    color
}

/// Draws a picture from a function giving the colour at each point, with smooth edges.
fn render_with(size: u32, color_at: impl Fn(f32, f32) -> Color) -> RgbaImage {
    let mut image = RgbaImage::new(size, size);
    let samples = (SAMPLES * SAMPLES) as f32;
    for (px, py, pixel) in image.enumerate_pixels_mut() {
        let mut total = [0.0f32; 4];
        for sy in 0..SAMPLES {
            for sx in 0..SAMPLES {
                let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                let color = color_at(x, y);
                total[0] += color[0] * color[3];
                total[1] += color[1] * color[3];
                total[2] += color[2] * color[3];
                total[3] += color[3];
            }
        }
        let alpha = total[3] / samples;
        let channel = |value: f32| {
            if total[3] > 0.0 { (value / total[3] * 255.0) as u8 } else { 0 }
        };
        *pixel = Rgba([channel(total[0]), channel(total[1]), channel(total[2]), (alpha * 255.0) as u8]);
    }
    image
}

/// Draws the icon at any size.
pub fn render(size: u32) -> RgbaImage {
    render_with(size, |x, y| lantern(x, y, true))
}

/// A built-in key icon by the name a key stores for it. Keys saved before the lantern hold the old
/// name, and get the new picture.
pub fn named(name: &str, size: u32) -> Option<RgbaImage> {
    use pyroporch_core::engine::{OLD_SLEEP_ICON, SLEEP_ICON};
    (name == SLEEP_ICON || name == OLD_SLEEP_ICON).then(|| render_with(size, |x, y| lantern(x, y, false)))
}

/// The lantern alone, for the menu bar, where a full square icon looks out of place.
pub fn render_mark(size: u32) -> RgbaImage {
    render_with(size, lantern_mark)
}

/// How big a picture a key may name may be, and how much room it may need to be read.
const MAX_SIDE: u32 = 16_384;
const MAX_ALLOC: u64 = 128 * 1024 * 1024;

/// Reads a picture named in a key's settings, with limits: a huge or malformed file must not
/// be able to take the app down. Returns a copy small enough to draw.
pub fn open_capped(path: &str) -> Option<RgbaImage> {
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    Some(picture.thumbnail(256, 256).to_rgba8())
}

/// The lantern in one flat colour, keeping its shape.
pub fn render_mark_tinted(size: u32, shade: u8) -> RgbaImage {
    let mut image = render_mark(size);
    for pixel in image.pixels_mut() {
        pixel.0[0] = shade;
        pixel.0[1] = shade;
        pixel.0[2] = shade;
    }
    image
}

/// The icon as a macOS `.icns` file: a small header, then a PNG at each size.
#[cfg(target_os = "macos")]
pub fn icns() -> Result<Vec<u8>, String> {
    let sizes: [(&[u8; 4], u32); 8] = [
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic07", 128),
        (b"ic13", 256),
        (b"ic08", 256),
        (b"ic14", 512),
        (b"ic09", 512),
        (b"ic10", 1024),
    ];
    // Drawn once at the largest size: drawing is the slow part.
    let largest = render(1024);
    let mut entries = Vec::new();
    for (kind, size) in sizes {
        let mut png = Vec::new();
        image::imageops::resize(&largest, size, size, image::imageops::FilterType::Lanczos3)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        entries.extend_from_slice(kind);
        entries.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
        entries.extend_from_slice(&png);
    }
    let mut file = Vec::with_capacity(entries.len() + 8);
    file.extend_from_slice(b"icns");
    file.extend_from_slice(&(entries.len() as u32 + 8).to_be_bytes());
    file.extend_from_slice(&entries);
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyroporch_core::engine::{OLD_SLEEP_ICON, SLEEP_ICON};

    #[test]
    fn the_icon_has_a_beam_a_lantern_a_flame_and_clear_corners() {
        let image = render(128);
        assert_eq!(image.get_pixel(1, 1).0[3], 0, "corners are clear");
        let flame = image.get_pixel(64, 90);
        assert!(flame.0[0] > 200 && flame.0[1] > 120 && flame.0[3] == 255, "{flame:?}");
        let beam = image.get_pixel(64, 19);
        assert!(beam.0[0] > beam.0[2] && beam.0[3] == 255, "{beam:?}");
        let frame = image.get_pixel(45, 77);
        assert_eq!(frame.0[3], 255, "{frame:?}");
    }

    #[test]
    fn the_menu_bar_lantern_can_be_white_or_black_in_the_same_shape() {
        let color = render_mark(44);
        for shade in [255, 0] {
            let tinted = render_mark_tinted(44, shade);
            assert!(tinted.pixels().all(|pixel| pixel.0[..3] == [shade; 3]));
            let shape = |image: &RgbaImage| image.pixels().map(|pixel| pixel.0[3]).collect::<Vec<_>>();
            assert_eq!(shape(&tinted), shape(&color));
        }
        assert!(color.pixels().any(|pixel| pixel.0[3] > 0), "there's a lantern to see");
    }

    #[test]
    fn the_sleep_icon_is_the_lantern_gone_out_on_a_clear_background() {
        let image = named(SLEEP_ICON, 128).unwrap();
        assert_eq!(image.get_pixel(3, 3).0[3], 0, "clear, so the key's colour shows");
        let ember = image.get_pixel(64, 82);
        assert!(ember.0[0] > 200 && ember.0[2] < 90, "{ember:?}");
        let frame = image.get_pixel(45, 77);
        assert!(frame.0[3] == 255 && frame.0[0].abs_diff(frame.0[2]) < 12, "ash grey: {frame:?}");
        assert!(image.enumerate_pixels().any(|(_, y, p)| (56..78).contains(&y) && p.0[3] > 40), "smoke");
        assert!(named("builtin:nothing", 64).is_none());
    }

    #[test]
    fn keys_saved_with_the_old_icon_name_get_the_new_picture() {
        assert_eq!(named(OLD_SLEEP_ICON, 32), named(SLEEP_ICON, 32));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_icon_file_is_well_formed() {
        let file = icns().unwrap();
        assert_eq!(&file[..4], b"icns");
        let length = u32::from_be_bytes([file[4], file[5], file[6], file[7]]) as usize;
        assert_eq!(length, file.len());
    }
}
