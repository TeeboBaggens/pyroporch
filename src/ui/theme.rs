//! Colours and spacing, driven by the `[ui]` section of the config.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle};
use pyroporch_core::config::UiSettings;

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
}

/// Reads `#rrggbb`.
pub fn parse_color(text: &str) -> Option<Color32> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some(Color32::from_rgb(channel(0)?, channel(2)?, channel(4)?))
}

pub fn color_hex(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

pub fn palette(settings: &UiSettings) -> Palette {
    // The same accent the config's default uses, so an unreadable one doesn't show a colour
    // from an older version.
    let accent = parse_color(&settings.accent)
        .or_else(|| parse_color(pyroporch_core::config::DEFAULT_ACCENT))
        .unwrap_or(Color32::from_rgb(0xE8, 0x74, 0x3B));
    if settings.theme == "light" {
        Palette {
            bg: Color32::from_rgb(0xF2, 0xF4, 0xF7),
            panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            raised: Color32::from_rgb(0xE7, 0xEA, 0xEF),
            border: Color32::from_rgb(0xD3, 0xD8, 0xE0),
            text: Color32::from_rgb(0x1C, 0x21, 0x2B),
            muted: Color32::from_rgb(0x5E, 0x67, 0x76),
            accent,
            ok: Color32::from_rgb(0x1F, 0x8A, 0x4C),
            warn: Color32::from_rgb(0xB2, 0x6B, 0x00),
            bad: Color32::from_rgb(0xC6, 0x30, 0x30),
        }
    } else {
        Palette {
            bg: Color32::from_rgb(0x14, 0x17, 0x1C),
            panel: Color32::from_rgb(0x1B, 0x1F, 0x26),
            raised: Color32::from_rgb(0x26, 0x2B, 0x35),
            border: Color32::from_rgb(0x31, 0x38, 0x44),
            text: Color32::from_rgb(0xE6, 0xE9, 0xEF),
            muted: Color32::from_rgb(0x96, 0x9E, 0xAE),
            accent,
            ok: Color32::from_rgb(0x3F, 0xB9, 0x72),
            warn: Color32::from_rgb(0xE5, 0xA9, 0x3C),
            bad: Color32::from_rgb(0xEC, 0x5F, 0x5F),
        }
    }
}

pub fn apply(ctx: &egui::Context, settings: &UiSettings) {
    let p = palette(settings);
    ctx.set_zoom_factor(settings.scale.clamp(0.5, 3.0));
    // Size comes from the config, and these shortcuts need to reach the hotkey recorder.
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    let mut style = (*ctx.global_style()).clone();
    let mut visuals = if settings.theme == "light" {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.panel;
    visuals.extreme_bg_color = p.bg;
    visuals.faint_bg_color = p.raised;
    visuals.window_stroke = Stroke::new(1.0, p.border);
    visuals.override_text_color = Some(p.text);
    visuals.selection.bg_fill = p.accent.linear_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, p.accent);
    visuals.hyperlink_color = p.accent;
    let radius = CornerRadius::same(6);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
    }
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.inactive.bg_fill = p.raised;
    visuals.widgets.inactive.weak_bg_fill = p.raised;
    visuals.window_corner_radius = CornerRadius::same(10);
    style.visuals = visuals;
    // This is an app, not a page: labels are for reading, not for selecting.
    style.interaction.selectable_labels = false;
    style.interaction.multi_widget_text_select = false;
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.text_styles.insert(TextStyle::Body, FontId::proportional(13.5));
    style.text_styles.insert(TextStyle::Button, FontId::proportional(13.5));
    style.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
    style.text_styles.insert(TextStyle::Heading, FontId::proportional(19.0));
    ctx.set_global_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_round_trip_and_bad_ones_are_refused() {
        let color = parse_color("#2F6FDB").unwrap();
        assert_eq!(color_hex(color), "#2F6FDB");
        assert!(parse_color("blue").is_none());
        assert!(parse_color("#12").is_none());
        assert!(parse_color("#ééé").is_none());
    }
}
