//! When a screen saver starts and stops. The plugin says what it looks like: a helper thread asks
//! it for frames, which go to the deck and to the window.

use super::{App, KeyId};
use crate::hardware;
use eframe::egui::{self, Color32};
use pyroporch_core::engine::{Engine, SaverFrame};
use pyroporch_sdk::{PaintRequest, SaverTiming};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// How many squares a plugin draws across each key.
const BLOCKS_PER_KEY: u32 = 12;
const FRAME_EVERY: Duration = Duration::from_millis(66);

/// A key that turned a screen saver on, and what it asked for.
#[derive(Clone)]
pub struct Armed {
    pub id: KeyId,
    pub plugin: String,
    pub settings: Map<String, Value>,
    pub timing: SaverTiming,
}

/// Whether a key has turned a screen saver on, and whether one is playing right now.
pub struct SaverControl {
    pub armed: Option<Armed>,
    pub playing: Option<Playing>,
    pub last_press: Instant,
}

pub struct Playing {
    pub timing: SaverTiming,
    /// The latest frame, for drawing in the window.
    pub frame: Option<Arc<SaverFrame>>,
    /// Only a look: stopping it leaves everything else as it was.
    pub preview: bool,
    /// Dropping this is what stops the helper thread.
    frames: Receiver<Result<SaverFrame, String>>,
}

impl Default for SaverControl {
    fn default() -> Self {
        Self {
            armed: None,
            playing: None,
            last_press: Instant::now(),
        }
    }
}

/// Asks the plugin for frames until nobody's listening any more.
fn play(engine: Arc<Engine>, plugin: String, mut request: PaintRequest, wake: egui::Context) -> Receiver<Result<SaverFrame, String>> {
    let (sender, frames) = channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            let frame_started = Instant::now();
            request.millis = started.elapsed().as_millis() as u64;
            let painted = engine.paint(&plugin, &request);
            let failed = painted.is_err();
            request.restart = false;
            if sender.send(painted).is_err() || failed {
                return;
            }
            wake.request_repaint();
            std::thread::sleep(FRAME_EVERY.saturating_sub(frame_started.elapsed()));
        }
    });
    frames
}

impl App {
    fn start_playing(&mut self, ctx: &egui::Context, plugin: String, settings: Map<String, Value>, timing: SaverTiming, preview: bool) {
        // The deck's own grid if one's plugged in, so the animation fits it.
        let (rows, cols) = self.deck_shape.or_else(|| self.profile().map(|x| (x.rows, x.cols))).unwrap_or((3, 5));
        let request = PaintRequest {
            settings: settings.clone(),
            rows,
            cols,
            blocks_per_key: BLOCKS_PER_KEY,
            millis: 0,
            restart: true,
        };
        self.saver.playing = Some(Playing {
            timing,
            frame: None,
            preview,
            frames: play(self.engine.clone(), plugin, request, ctx.clone()),
        });
    }

    /// Pressing a screen saver key: on if it's off, and off if this key turned it on.
    pub fn toggle_saver(&mut self, ctx: &egui::Context, id: KeyId, plugin: String, settings: Map<String, Value>, timing: SaverTiming) {
        let this_key_is_on = self.saver.armed.as_ref().is_some_and(|armed| armed.id == id);
        self.saver.playing = None;
        if this_key_is_on {
            self.saver.armed = None;
            self.log(true, "screen saver off");
            return;
        }
        self.saver.armed = Some(Armed {
            id,
            plugin: plugin.clone(),
            settings: settings.clone(),
            timing,
        });
        self.saver.last_press = Instant::now();
        if timing.start_now {
            self.start_playing(ctx, plugin, settings, timing, false);
            self.log(true, "screen saver on");
        } else {
            self.log(true, format!("screen saver on: starts after {} quiet minute(s)", timing.wait_minutes));
        }
    }

    /// Stops it playing. It's still on if it's waiting to come back after a quiet spell.
    pub fn stop_playing(&mut self) {
        let Some(playing) = self.saver.playing.take() else {
            return;
        };
        self.saver.last_press = Instant::now();
        // Started once with nothing to wait for, so that was the whole job.
        if !playing.preview && playing.timing.wait_minutes == 0 {
            self.saver.armed = None;
        }
    }

    pub fn saver_off(&mut self) {
        self.saver.playing = None;
        self.saver.armed = None;
        self.log(true, "screen saver off");
    }

    pub fn preview_saver(&mut self, ctx: &egui::Context, plugin: String, settings: Map<String, Value>) {
        let timing = SaverTiming {
            start_now: true,
            wait_minutes: 0,
        };
        self.start_playing(ctx, plugin, settings, timing, true);
    }

    /// Starts it once the keys have been quiet long enough, and collects new frames.
    pub fn saver_tick(&mut self, ctx: &egui::Context) {
        let mut problem = None;
        if let Some(playing) = &mut self.saver.playing {
            while let Ok(painted) = playing.frames.try_recv() {
                match painted {
                    Ok(frame) => {
                        let frame = Arc::new(frame);
                        playing.frame = Some(frame.clone());
                        if let Some(handle) = &self.hardware {
                            let _ = handle.commands.send(hardware::Command::SaverFrame(frame));
                        }
                    }
                    Err(error) => problem = Some(error),
                }
            }
            ctx.request_repaint_after(FRAME_EVERY);
        }
        if let Some(error) = problem {
            // Its plugin may have been removed or turned off.
            self.saver.playing = None;
            self.saver.armed = None;
            self.log(false, format!("the screen saver stopped: {error}"));
            return;
        }
        if self.saver.playing.is_some() || self.asleep {
            return;
        }
        let Some(armed) = self.saver.armed.clone() else {
            return;
        };
        if armed.timing.wait_minutes == 0 {
            return;
        }
        if self.saver.last_press.elapsed() >= Duration::from_secs(u64::from(armed.timing.wait_minutes) * 60) {
            self.start_playing(ctx, armed.plugin, armed.settings, armed.timing, false);
        }
    }

    /// The grid the playing animation was drawn for.
    pub fn saver_shape(&self) -> Option<(u32, u32)> {
        let frame = self.saver.playing.as_ref()?.frame.as_ref()?;
        Some((frame.rows, frame.cols))
    }

    /// Draws one key of the animation in the window.
    pub fn paint_saver_key(&mut self, ui: &egui::Ui, rect: egui::Rect, row: u32, col: u32) {
        let painter = ui.painter_at(rect);
        let Some(frame) = self.saver.playing.as_ref().and_then(|playing| playing.frame.as_ref()) else {
            painter.rect_filled(rect, 0, Color32::BLACK);
            return;
        };
        let light = f32::from(frame.brightness_percent) / 100.0;
        let square = rect.width() / frame.blocks as f32;
        for down in 0..frame.blocks {
            for across in 0..frame.blocks {
                let [r, g, b] = frame.color(row, col, across, down).map(|channel| (f32::from(channel) * light) as u8);
                let min = rect.min + egui::vec2(across as f32 * square, down as f32 * square);
                // A touch oversized, so no seams show between squares.
                painter.rect_filled(egui::Rect::from_min_size(min, egui::Vec2::splat(square + 0.6)), 0, Color32::from_rgb(r, g, b));
            }
        }
    }
}
