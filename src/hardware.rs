//! The USB deck module. Runs on its own thread: it finds a deck, draws keys on it and reports
//! presses. Close the deck maker's own app first, because only one program can hold the device.

use pyroporch_core::engine::SaverFrame;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// What one physical key should show.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyVisual {
    pub row: u32,
    pub col: u32,
    pub title: String,
    pub icon: String,
    /// Full path of the picture filling the key, if any.
    pub background: String,
    pub color: String,
    pub text_color: String,
}

pub enum Command {
    Show(Vec<KeyVisual>),
    Brightness(u8),
    /// True turns every key and the backlight off. False puts them back.
    Asleep(bool),
    /// Shows the next frame of a screen saver.
    SaverFrame(Arc<SaverFrame>),
    /// Goes back to showing the keys.
    SaverStop,
}

pub enum Event {
    Connected { name: String, rows: u32, cols: u32 },
    Disconnected,
    Pressed { row: u32, col: u32 },
}

pub struct Handle {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
}

/// Starts looking for a deck. `wake` is called whenever there's news for the window.
pub fn start(brightness: u8, wake: impl Fn() + Send + 'static) -> Handle {
    let (commands, command_rx) = channel();
    let (event_tx, events) = channel();
    #[cfg(feature = "hardware")]
    std::thread::spawn(move || usb::run(brightness, command_rx, event_tx, wake));
    #[cfg(not(feature = "hardware"))]
    let _ = (brightness, command_rx, event_tx, wake);
    Handle { commands, events }
}

/// Lists connected decks and their key sizes, without touching what they show.
#[cfg(feature = "hardware")]
pub fn describe_connected() -> Vec<String> {
    let Ok(hid) = elgato_streamdeck::new_hidapi() else {
        return Vec::new();
    };
    elgato_streamdeck::list_devices(&hid)
        .into_iter()
        .map(|(kind, serial)| {
            let (width, height) = kind.key_image_format().size;
            format!(
                "{kind:?} ({serial}): {} rows, {} columns, {width} x {height} pixels per key",
                kind.row_count(),
                kind.column_count(),
            )
        })
        .collect()
}

#[cfg(not(feature = "hardware"))]
pub fn describe_connected() -> Vec<String> {
    Vec::new()
}

#[cfg(feature = "hardware")]
mod usb {
    use super::{Command, Event, KeyVisual, SaverFrame};
    use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
    use elgato_streamdeck::{StreamDeck, StreamDeckInput, list_devices, new_hidapi, refresh_device_list};
    use image::{DynamicImage, Rgba, RgbaImage, imageops};
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender};
    use std::time::Duration;

    const SCAN_EVERY: Duration = Duration::from_secs(2);
    const READ_WAIT: Duration = Duration::from_millis(40);
    /// Short while a screen saver plays, so that drawing it and noticing a press both stay quick.
    const SAVER_READ_WAIT: Duration = Duration::from_millis(5);

    pub fn run(brightness: u8, commands: Receiver<Command>, events: Sender<Event>, wake: impl Fn() + Send + 'static) {
        let Ok(mut hid) = new_hidapi() else {
            return;
        };
        let mut state = State {
            brightness,
            asleep: false,
            latest: Vec::new(),
            saver: None,
        };
        loop {
            // Keep the latest picture even with no deck plugged in.
            loop {
                match commands.try_recv() {
                    Ok(Command::Show(visuals)) => state.latest = visuals,
                    Ok(Command::Brightness(value)) => state.brightness = value,
                    Ok(Command::Asleep(asleep)) => state.asleep = asleep,
                    Ok(Command::SaverFrame(frame)) => state.saver = Some(frame),
                    Ok(Command::SaverStop) => state.saver = None,
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                }
            }
            let _ = refresh_device_list(&mut hid);
            let found = list_devices(&hid).into_iter().find(|(kind, _)| kind.key_count() > 0);
            let Some((kind, serial)) = found else {
                std::thread::sleep(SCAN_EVERY);
                continue;
            };
            let Ok(deck) = StreamDeck::connect(&hid, kind, &serial) else {
                std::thread::sleep(SCAN_EVERY);
                continue;
            };
            let _ = events.send(Event::Connected {
                name: deck.product().unwrap_or_else(|_| format!("{kind:?}")),
                rows: u32::from(kind.row_count()),
                cols: u32::from(kind.column_count()),
            });
            wake();
            let keep_going = drive(&deck, &mut state, &commands, &events, &wake);
            let _ = events.send(Event::Disconnected);
            wake();
            if !keep_going {
                let _ = deck.reset();
                return;
            }
        }
    }

    /// What should be on the deck, kept across it being unplugged.
    struct State {
        brightness: u8,
        asleep: bool,
        latest: Vec<KeyVisual>,
        saver: Option<Arc<SaverFrame>>,
    }

    /// A screen saver playing on this deck, keeping track of what's already on the keys so only
    /// the ones that change get sent.
    struct Playing {
        on_keys: Vec<Option<RgbaImage>>,
        light: Option<u8>,
    }

    impl Playing {
        fn new(keys: usize) -> Self {
            Self {
                on_keys: vec![None; keys],
                light: None,
            }
        }

        fn show(&mut self, deck: &StreamDeck, frame: &SaverFrame, cols: u32, (width, height): (u32, u32), brightness: u8) {
            let mut changed = false;
            for (index, on_key) in self.on_keys.iter_mut().enumerate() {
                let image = saver_key(frame, index as u32 / cols, index as u32 % cols, width, height);
                if on_key.as_ref() != Some(&image) {
                    let _ = deck.set_button_image(index as u8, DynamicImage::ImageRgba8(image.clone()));
                    *on_key = Some(image);
                    changed = true;
                }
            }
            if changed {
                let _ = deck.flush();
            }
            let light = (u32::from(brightness) * u32::from(frame.brightness_percent) / 100) as u8;
            if self.light != Some(light) {
                self.light = Some(light);
                let _ = deck.set_brightness(light);
            }
        }
    }

    /// Paints one key's share of a frame, at the size of this deck's keys.
    pub fn saver_key(frame: &SaverFrame, row: u32, col: u32, width: u32, height: u32) -> RgbaImage {
        let blocks = frame.blocks.max(1);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let down = y * blocks / height.max(1);
            for x in 0..width {
                let [r, g, b] = frame.color(row, col, x * blocks / width.max(1), down);
                pixels.extend_from_slice(&[r, g, b, 255]);
            }
        }
        RgbaImage::from_raw(width, height, pixels).unwrap_or_else(|| RgbaImage::new(width, height))
    }

    /// Every key blank, and no backlight behind them.
    fn go_dark(deck: &StreamDeck, shown: &mut [Option<KeyVisual>]) {
        let _ = deck.set_brightness(0);
        let _ = deck.clear_all_button_images();
        let _ = deck.flush();
        shown.iter_mut().for_each(|slot| *slot = None);
    }

    /// Serves one connected deck. Returns false when the app is closing, true if it was unplugged.
    fn drive(
        deck: &StreamDeck,
        state: &mut State,
        commands: &Receiver<Command>,
        events: &Sender<Event>,
        wake: &impl Fn(),
    ) -> bool {
        let kind = deck.kind();
        let cols = u32::from(kind.column_count()).max(1);
        let mut shown: Vec<Option<KeyVisual>> = vec![None; usize::from(kind.key_count())];
        let mut held = vec![false; usize::from(kind.key_count())];
        let (width, height) = kind.key_image_format().size;
        let size = (width as u32, height as u32);
        let keys = usize::from(kind.key_count());
        let mut playing = state.saver.as_ref().map(|_| Playing::new(keys));
        if state.asleep {
            go_dark(deck, &mut shown);
        } else {
            let _ = deck.set_brightness(state.brightness);
            draw(deck, &state.latest, &mut shown, cols, size);
        }
        loop {
            let mut new_frame: Option<Arc<SaverFrame>> = None;
            loop {
                match commands.try_recv() {
                    Ok(Command::Show(visuals)) => {
                        state.latest = visuals;
                        if !state.asleep && playing.is_none() {
                            draw(deck, &state.latest, &mut shown, cols, size);
                        }
                    }
                    Ok(Command::Brightness(value)) => {
                        state.brightness = value;
                        if !state.asleep {
                            let _ = deck.set_brightness(value);
                        }
                        // Picked up on the next frame, for a saver that's changing the light.
                        if let Some(active) = &mut playing {
                            active.light = None;
                        }
                    }
                    Ok(Command::Asleep(asleep)) if asleep != state.asleep => {
                        state.asleep = asleep;
                        if asleep {
                            go_dark(deck, &mut shown);
                        } else {
                            draw(deck, &state.latest, &mut shown, cols, size);
                            let _ = deck.set_brightness(state.brightness);
                        }
                    }
                    Ok(Command::Asleep(_)) => {}
                    // Only the newest frame gets drawn, so a slow deck can't fall behind.
                    Ok(Command::SaverFrame(frame)) => {
                        new_frame = Some(frame.clone());
                        state.saver = Some(frame);
                        if playing.is_none() {
                            playing = Some(Playing::new(keys));
                        }
                    }
                    Ok(Command::SaverStop) => {
                        state.saver = None;
                        new_frame = None;
                        if playing.take().is_some() {
                            shown.iter_mut().for_each(|slot| *slot = None);
                            let _ = deck.clear_all_button_images();
                            draw(deck, &state.latest, &mut shown, cols, size);
                            let _ = deck.set_brightness(state.brightness);
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return false,
                }
            }
            if let (Some(active), Some(frame), false) = (&mut playing, &new_frame, state.asleep) {
                active.show(deck, frame, cols, size, state.brightness);
            }
            let wait = if playing.is_some() { SAVER_READ_WAIT } else { READ_WAIT };
            match deck.read_input(Some(wait)) {
                Ok(StreamDeckInput::ButtonStateChange(states)) => {
                    for (index, (now, before)) in states.iter().zip(held.iter_mut()).enumerate() {
                        if *now && !*before {
                            let _ = events.send(Event::Pressed {
                                row: index as u32 / cols,
                                col: index as u32 % cols,
                            });
                            wake();
                        }
                        *before = *now;
                    }
                }
                Ok(_) => {}
                Err(_) => return true,
            }
        }
    }

    fn draw(deck: &StreamDeck, visuals: &[KeyVisual], shown: &mut [Option<KeyVisual>], cols: u32, size: (u32, u32)) {
        let mut changed = false;
        for (index, slot) in shown.iter_mut().enumerate() {
            let (row, col) = (index as u32 / cols, index as u32 % cols);
            let wanted = visuals.iter().find(|v| v.row == row && v.col == col).cloned();
            if *slot != wanted {
                let image = wanted
                    .as_ref()
                    .map_or_else(|| RgbaImage::from_pixel(size.0, size.1, Rgba([0, 0, 0, 255])), |v| render_key(v, size));
                let _ = deck.set_button_image(index as u8, DynamicImage::ImageRgba8(image));
                *slot = wanted;
                changed = true;
            }
        }
        if changed {
            let _ = deck.flush();
        }
    }

    fn parse(text: &str, fallback: [u8; 3]) -> Rgba<u8> {
        let hex = text.trim().strip_prefix('#').filter(|h| h.len() == 6 && h.is_ascii());
        let channel = |at: usize| hex.and_then(|h| u8::from_str_radix(&h[at..at + 2], 16).ok());
        match (channel(0), channel(2), channel(4)) {
            (Some(r), Some(g), Some(b)) => Rgba([r, g, b, 255]),
            _ => Rgba([fallback[0], fallback[1], fallback[2], 255]),
        }
    }

    /// Draws text centred, using the first font in the list that has each character.
    fn put_text(image: &mut RgbaImage, fonts: &[FontRef<'_>], text: &str, size: f32, center_y: f32, color: Rgba<u8>) {
        let scale = PxScale::from(size);
        let font_for = |c: char| fonts.iter().find(|font| font.glyph_id(c).0 != 0).or(fonts.first());
        let advance = |c: char| font_for(c).map_or(0.0, |font| font.as_scaled(scale).h_advance(font.glyph_id(c)));
        let width: f32 = text.chars().map(advance).sum();
        let mut x = (image.width() as f32 - width).max(0.0) / 2.0;
        for character in text.chars() {
            let Some(font) = font_for(character) else {
                continue;
            };
            let scaled = font.as_scaled(scale);
            let baseline = center_y + (scaled.ascent() + scaled.descent()) / 2.0;
            let id = font.glyph_id(character);
            let glyph = id.with_scale_and_position(scale, point(x, baseline));
            x += scaled.h_advance(id);
            let Some(outlined) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let (px, py) = (bounds.min.x as i64 + i64::from(gx), bounds.min.y as i64 + i64::from(gy));
                if px >= 0 && py >= 0 && px < i64::from(image.width()) && py < i64::from(image.height()) {
                    let pixel = image.get_pixel_mut(px as u32, py as u32);
                    for channel in 0..3 {
                        let (under, over) = (f32::from(pixel.0[channel]), f32::from(color.0[channel]));
                        pixel.0[channel] = (under + (over - under) * coverage) as u8;
                    }
                }
            });
        }
    }

    /// Draws one key: background colour, icon, and the title near the bottom.
    pub fn render_key(visual: &KeyVisual, (width, height): (u32, u32)) -> RgbaImage {
        let mut image = RgbaImage::from_pixel(width, height, parse(&visual.color, [0x26, 0x2B, 0x35]));
        let text_color = parse(&visual.text_color, [255, 255, 255]);
        let font = |bytes: &'static [u8]| FontRef::try_from_slice(bytes).ok();
        let (ubuntu, noto, icons) = (
            font(epaint_default_fonts::UBUNTU_LIGHT),
            font(epaint_default_fonts::NOTO_EMOJI_REGULAR),
            font(epaint_default_fonts::EMOJI_ICON),
        );
        let label: Vec<FontRef<'_>> = [ubuntu.clone(), noto.clone()].into_iter().flatten().collect();
        let symbols: Vec<FontRef<'_>> = [noto, icons, ubuntu].into_iter().flatten().collect();
        let has_title = !visual.title.is_empty();
        let on_picture = match image::open(&visual.background) {
            Ok(picture) => {
                let picture = picture.resize_exact(width, height, imageops::FilterType::Triangle).to_rgba8();
                imageops::overlay(&mut image, &picture, 0, 0);
                true
            }
            Err(_) => false,
        };
        let looks_like_path = visual.icon.contains(['/', '\\', '.']);
        if on_picture {
            // The picture is the key's look, so the small icon stays out of its way.
        } else if let Some(drawn) = crate::icon::named(&visual.icon, (height as f32 * if has_title { 0.56 } else { 0.72 }) as u32) {
            let x = i64::from(width.saturating_sub(drawn.width()) / 2);
            let y = if has_title { i64::from(height / 14) } else { i64::from(height.saturating_sub(drawn.height()) / 2) };
            imageops::overlay(&mut image, &drawn, x, y);
        } else if looks_like_path {
            if let Some(icon) = crate::icon::open_capped(&visual.icon) {
                let side = (height as f32 * 0.6) as u32;
                let icon = imageops::resize(&icon, side, side, imageops::FilterType::Triangle);
                let x = i64::from(width.saturating_sub(icon.width()) / 2);
                imageops::overlay(&mut image, &icon, x, i64::from(height / 12));
            }
        } else if !visual.icon.is_empty() {
            let center = if has_title { 0.4 } else { 0.5 };
            put_text(&mut image, &symbols, &visual.icon, height as f32 * 0.42, height as f32 * center, text_color);
        }
        if has_title {
            let title: String = visual.title.chars().take(12).collect();
            let (size, center) = (height as f32 * 0.17, height as f32 * 0.83);
            // A dark edge keeps the title readable on any picture.
            if on_picture {
                let shadow = Rgba([0, 0, 0, 255]);
                for offset in [-1.0, 1.0] {
                    put_text(&mut image, &label, &title, size, center + offset, shadow);
                }
            }
            put_text(&mut image, &label, &title, size, center, text_color);
        }
        image
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn draws_a_key_with_its_colour_and_text() {
            let visual = KeyVisual {
                row: 0,
                col: 0,
                title: "Hello".into(),
                icon: "🔗".into(),
                background: String::new(),
                color: "#102030".into(),
                text_color: String::new(),
            };
            let image = render_key(&visual, (96, 96));
            assert_eq!(image.dimensions(), (96, 96));
            assert_eq!(*image.get_pixel(0, 0), Rgba([0x10, 0x20, 0x30, 255]));
            let inked = image.pixels().filter(|p| p.0[0] > 0x60).count();
            assert!(inked > 50, "text and icon should be drawn, found {inked} bright pixels");
        }

        #[test]
        fn a_picture_fills_the_key_and_replaces_the_icon() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("key.png");
            RgbaImage::from_pixel(144, 144, Rgba([200, 20, 20, 255])).save(&path).unwrap();
            let visual = KeyVisual {
                row: 0,
                col: 0,
                title: String::new(),
                icon: "🔗".into(),
                background: path.to_string_lossy().into_owned(),
                color: "#102030".into(),
                text_color: String::new(),
            };
            // An XL key, whose own size is 96 pixels across.
            let image = render_key(&visual, (96, 96));
            assert!(image.pixels().all(|pixel| *pixel == Rgba([200, 20, 20, 255])), "the picture covers it all");
        }

        fn frame() -> SaverFrame {
            // One colour per square, at 2 squares a key.
            let (rows, cols, blocks) = (2u32, 2u32, 2u32);
            let mut rgb = Vec::new();
            for y in 0..rows * blocks {
                for x in 0..cols * blocks {
                    rgb.extend_from_slice(&[(x * 40) as u8, (y * 40) as u8, 200]);
                }
            }
            SaverFrame { rows, cols, blocks, brightness_percent: 100, rgb }
        }

        #[test]
        fn a_frame_is_sliced_into_keys_at_the_decks_size() {
            let frame = frame();
            let left = saver_key(&frame, 0, 0, 96, 96);
            let right = saver_key(&frame, 0, 1, 96, 96);
            assert_eq!(left.dimensions(), (96, 96));
            assert_eq!(left.get_pixel(0, 0).0, [0, 0, 200, 255]);
            assert_eq!(left.get_pixel(95, 0).0, [40, 0, 200, 255], "the next square across");
            assert_eq!(left.get_pixel(0, 95).0, [0, 40, 200, 255], "the next square down");
            assert_eq!(right.get_pixel(0, 0).0, [80, 0, 200, 255], "the key beside it carries on");
            assert!(left.pixels().all(|pixel| pixel.0[3] == 255), "no holes");
        }

        #[test]
        fn keys_beyond_a_smaller_frame_are_black() {
            let image = saver_key(&frame(), 0, 5, 72, 72);
            assert!(image.pixels().all(|pixel| pixel.0 == [0, 0, 0, 255]));
            let image = saver_key(&frame(), 7, 0, 72, 72);
            assert!(image.pixels().all(|pixel| pixel.0 == [0, 0, 0, 255]));
        }

        /// Everything but the USB write, for a whole deck. Run with --nocapture to see the time.
        #[test]
        fn a_whole_deck_of_screen_saver_keys_can_be_prepared() {
            let kind = elgato_streamdeck::info::Kind::XlV2;
            let (width, height) = kind.key_image_format().size;
            let frame = SaverFrame { rows: 4, cols: 8, blocks: 12, brightness_percent: 100, rgb: vec![90; 96 * 48 * 3] };
            let started = std::time::Instant::now();
            for index in 0..u32::from(kind.key_count()) {
                let image = saver_key(&frame, index / 8, index % 8, width as u32, height as u32);
                elgato_streamdeck::images::convert_image(kind, DynamicImage::ImageRgba8(image)).unwrap();
            }
            eprintln!("{:?} for one frame of 32 keys", started.elapsed());
        }

        #[test]
        fn the_drawn_sleep_icon_shows_on_a_key() {
            let visual = KeyVisual {
                row: 0,
                col: 0,
                title: "Sleep".into(),
                icon: pyroporch_core::engine::SLEEP_ICON.into(),
                background: String::new(),
                color: "#101010".into(),
                text_color: String::new(),
            };
            let image = render_key(&visual, (96, 96));
            let embers = image.pixels().filter(|p| p.0[0] > 180 && p.0[2] < 90).count();
            assert!(embers > 3, "{embers} ember pixels");
        }

        #[test]
        fn a_bad_colour_falls_back_instead_of_failing() {
            assert_eq!(parse("nope", [1, 2, 3]), Rgba([1, 2, 3, 255]));
        }
    }
}
