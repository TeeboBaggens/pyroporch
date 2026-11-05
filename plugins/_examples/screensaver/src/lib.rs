//! The screen saver: a sample plugin, and the example of one that draws on the keys.
//!
//! It runs in the sandbox like any other plugin. Its action tells the app when to play, and asks
//! to be allowed to, because of the `deck = true` in its plugin.toml. The app then asks it for
//! frames through `paint`, and [`Saver`] works those out by answering one question: what colour is
//! this point right now? Points are in key units, so `(2.5, 1.0)` is halfway across the third key,
//! along the top of the second row.

use pyroporch_sdk::{
    ActionSpec, DeckControl, Field, FieldKind, HostHandle, Outcome, PaintRequest, Plugin, PluginInfo, Risk, RunRequest,
    SaverTiming, export_plugin,
};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const PLUGIN_ID: &str = "pyroporch.screensaver";
pub const ACTION_ID: &str = "pyroporch.screensaver/toggle";
const DEFAULT_WAIT_MINUTES: u32 = 5;
const LONGEST_WAIT_MINUTES: f64 = 1440.0;

/// How many game squares fit across one key.
const CELLS_PER_KEY: usize = 3;
const SNAKE_STEPS_PER_SECOND: f32 = 7.0;
const STARTING_SNAKE: usize = 3;
const LIFE_STEPS_PER_SECOND: f32 = 3.0;
const LIFE_MOST_GENERATIONS: u32 = 240;
const BREATH_SECONDS: f32 = 6.0;
/// Breathing never goes fully dark, so it still reads as on.
const DIMMEST_BREATH: f32 = 12.0;
const SPEEDS: [(&str, u16); 3] = [("slow", 50), ("normal", 100), ("fast", 200)];

/// What a screen saver looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaverAnimation {
    Breathing,
    RainbowSideways,
    RainbowUpDown,
    Snake,
    Campfire,
    DigitalRain,
    Stars,
    Life,
}

impl SaverAnimation {
    pub const ALL: &[SaverAnimation] = &[
        SaverAnimation::Breathing,
        SaverAnimation::RainbowSideways,
        SaverAnimation::RainbowUpDown,
        SaverAnimation::Snake,
        SaverAnimation::Campfire,
        SaverAnimation::DigitalRain,
        SaverAnimation::Stars,
        SaverAnimation::Life,
    ];

    /// The name in the settings form, which is also how a key's config stores it.
    pub fn label(self) -> &'static str {
        match self {
            SaverAnimation::Breathing => "Breathing",
            SaverAnimation::RainbowSideways => "Rainbow: side to side",
            SaverAnimation::RainbowUpDown => "Rainbow: up and down",
            SaverAnimation::Snake => "Snake",
            SaverAnimation::Campfire => "Campfire",
            SaverAnimation::DigitalRain => "Digital rain",
            SaverAnimation::Stars => "Twinkling stars",
            SaverAnimation::Life => "Game of Life",
        }
    }
}

/// What a key asks of the screen saver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaverConfig {
    pub animation: SaverAnimation,
    pub start_now: bool,
    pub wait_minutes: u32,
    pub speed_percent: u16,
}

#[derive(Default)]
pub struct ScreenSaverPlugin {
    /// The animation being played, kept between frames.
    playing: Mutex<Option<Saver>>,
}

impl Plugin for ScreenSaverPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.into(),
            name: "Screen Saver".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Animations on the keys, started by a key press or after a quiet spell.".into(),
            actions: vec![ActionSpec {
                id: "toggle".into(),
                name: "Screen Saver".into(),
                description: "Press to turn it on, press again to turn it off. While it plays, any key stops it.".into(),
                icon: "🌈".into(),
                risk: Risk::Low,
                refreshable: false,
                hidden: false,
                previews: true,
                fields: vec![
                    Field::new(
                        "animation",
                        "Animation",
                        FieldKind::Select {
                            options: SaverAnimation::ALL.iter().map(|a| a.label().to_string()).collect(),
                        },
                    )
                    .default(SaverAnimation::RainbowSideways.label()),
                    Field::new("start_now", "Start right away", FieldKind::Bool).default(false),
                    Field::new("wait_minutes", "Start after no key presses for (minutes)", FieldKind::Number)
                        .help("0 means it only plays when started right away, and turns itself off afterwards.")
                        .default(DEFAULT_WAIT_MINUTES),
                    Field::new(
                        "speed",
                        "Speed",
                        FieldKind::Select {
                            options: SPEEDS.iter().map(|(name, _)| (*name).to_string()).collect(),
                        },
                    )
                    .default("normal"),
                ],
            }],
        }
    }

    fn run(&self, request: &RunRequest, _host: &HostHandle) -> Result<Outcome, String> {
        let config = config_from(&request.settings);
        Ok(Outcome {
            deck: Some(DeckControl::ScreenSaver(SaverTiming {
                start_now: config.start_now,
                wait_minutes: config.wait_minutes,
            })),
            ..Outcome::default()
        })
    }

    fn paint(&self, request: &PaintRequest) -> Result<Vec<u8>, String> {
        let mut playing = self.playing.lock().map_err(|_| "the animation stopped".to_string())?;
        if request.restart || playing.is_none() {
            *playing = Some(Saver::new(config_from(&request.settings), request.rows, request.cols));
        }
        let saver = playing.as_mut().ok_or("the animation stopped")?;
        saver.set_time(request.millis as f32 / 1000.0);
        let blocks = request.blocks_per_key.max(1);
        let (across, down) = (request.cols * blocks, request.rows * blocks);
        let mut frame = Vec::with_capacity(1 + (across * down * 3) as usize);
        frame.push(saver.brightness_percent());
        for y in 0..down {
            for x in 0..across {
                let color = saver.color_at((x as f32 + 0.5) / blocks as f32, (y as f32 + 0.5) / blocks as f32);
                frame.extend_from_slice(&color);
            }
        }
        Ok(frame)
    }
}

export_plugin!(ScreenSaverPlugin::default());

/// Reads the settings form. Anything missing or strange falls back to the default.
pub fn config_from(settings: &Map<String, Value>) -> SaverConfig {
    let text = |key: &str| settings.get(key).and_then(Value::as_str).unwrap_or("");
    let animation = SaverAnimation::ALL
        .iter()
        .copied()
        .find(|a| a.label() == text("animation"))
        .unwrap_or(SaverAnimation::RainbowSideways);
    let speed_percent = SPEEDS
        .iter()
        .find(|(name, _)| *name == text("speed"))
        .map_or(100, |(_, percent)| *percent);
    let wait_minutes = settings
        .get("wait_minutes")
        .and_then(Value::as_f64)
        .map_or(DEFAULT_WAIT_MINUTES, |minutes| minutes.clamp(0.0, LONGEST_WAIT_MINUTES) as u32);
    let start_now = settings.get("start_now").and_then(Value::as_bool).unwrap_or(false);
    SaverConfig {
        animation,
        // With nothing to wait for, starting now is the only thing it can mean.
        start_now: start_now || wait_minutes == 0,
        wait_minutes,
        speed_percent,
    }
}

/// A small, repeatable source of randomness, so the deck and the window play the same game.
#[derive(Clone)]
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit.max(1) as u64) as usize
    }
}

/// The same value for the same inputs, spread evenly from 0 to 1.
fn hash(a: u32, b: u32) -> f32 {
    let mixed = (u64::from(a).wrapping_mul(0x9E37_79B1) ^ u64::from(b).wrapping_mul(0x85EB_CA77)).wrapping_add(0x1656_67B1);
    let mixed = (mixed ^ (mixed >> 15)).wrapping_mul(0x2545_F491_4F6C_DD1D);
    ((mixed >> 40) & 0xFFFF) as f32 / 65535.0
}

/// Smooth noise: random values at whole numbers, blended between them.
fn noise(x: f32, y: f32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |dx: i32, dy: i32| hash((ix as i32 + dx) as u32, (iy as i32 + dy) as u32);
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bottom - top) * sy
}

fn rainbow(position: f32) -> [u8; 3] {
    let hue = position.rem_euclid(1.0) * 6.0;
    let rise = hue.fract();
    let (r, g, b) = match hue as u32 {
        0 => (1.0, rise, 0.0),
        1 => (1.0 - rise, 1.0, 0.0),
        2 => (0.0, 1.0, rise),
        3 => (0.0, 1.0 - rise, 1.0),
        4 => (rise, 0.0, 1.0),
        _ => (1.0, 0.0, 1.0 - rise),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

/// Dark red to orange to pale yellow, the colours of a fire.
fn fire_color(heat: f32) -> [u8; 3] {
    let heat = heat.clamp(0.0, 1.0);
    let red = (heat * 2.6).min(1.0);
    let green = ((heat - 0.32) * 2.1).clamp(0.0, 1.0);
    let blue = ((heat - 0.78) * 3.0).clamp(0.0, 1.0);
    [(red * 255.0) as u8, (green * 215.0) as u8, (blue * 170.0) as u8]
}

fn scale(color: [u8; 3], amount: f32) -> [u8; 3] {
    color.map(|channel| (f32::from(channel) * amount.clamp(0.0, 1.0)) as u8)
}

/// A snake that plays itself: it heads for the food, and if it can't reach it, for wherever has
/// the most room. When it finally gets stuck, a new game starts.
#[derive(Clone)]
struct Snake {
    width: usize,
    height: usize,
    body: VecDeque<(usize, usize)>,
    food: (usize, usize),
    random: Random,
    best: usize,
}

impl Snake {
    fn new(width: usize, height: usize, seed: u64) -> Self {
        let mut snake = Self {
            width,
            height,
            body: VecDeque::new(),
            food: (0, 0),
            random: Random(seed | 1),
            best: 0,
        };
        snake.restart();
        snake
    }

    fn restart(&mut self) {
        self.best = self.best.max(self.body.len());
        self.body.clear();
        let (x, y) = (self.width / 2, self.height / 2);
        for back in 0..=x.min(STARTING_SNAKE - 1) {
            self.body.push_back((x - back, y));
        }
        self.place_food();
    }

    fn place_food(&mut self) {
        let free: Vec<(usize, usize)> = (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .filter(|cell| !self.body.contains(cell))
            .collect();
        match free.get(self.random.below(free.len())) {
            Some(cell) => self.food = *cell,
            // The snake has filled the board, which is as good as it gets.
            None => self.restart(),
        }
    }

    fn neighbours(&self, (x, y): (usize, usize)) -> Vec<(usize, usize)> {
        let mut around = Vec::with_capacity(4);
        if x > 0 {
            around.push((x - 1, y));
        }
        if y > 0 {
            around.push((x, y - 1));
        }
        if x + 1 < self.width {
            around.push((x + 1, y));
        }
        if y + 1 < self.height {
            around.push((x, y + 1));
        }
        around
    }

    /// Everything the snake can't move into. Its tail moves away, unless it's about to eat.
    fn blocked(&self) -> Vec<bool> {
        let mut blocked = vec![false; self.width * self.height];
        let moving: usize = usize::from(self.body.len() > 1);
        for (x, y) in self.body.iter().take(self.body.len() - moving) {
            blocked[y * self.width + x] = true;
        }
        blocked
    }

    /// The first move on a shortest way from `from` to `to`, if there is a way.
    fn step_towards(&self, from: (usize, usize), to: (usize, usize), blocked: &[bool]) -> Option<(usize, usize)> {
        let mut came_from: Vec<Option<(usize, usize)>> = vec![None; blocked.len()];
        let mut queue = VecDeque::from([to]);
        let mut seen = blocked.to_vec();
        seen[to.1 * self.width + to.0] = true;
        // Searches back from the target, so the answer is simply the head's next square.
        while let Some(cell) = queue.pop_front() {
            for next in self.neighbours(cell) {
                if next == from {
                    return Some(cell);
                }
                let index = next.1 * self.width + next.0;
                if !seen[index] {
                    seen[index] = true;
                    came_from[index] = Some(cell);
                    queue.push_back(next);
                }
            }
        }
        None
    }

    fn room_around(&self, start: (usize, usize), blocked: &[bool]) -> usize {
        let mut seen = blocked.to_vec();
        let mut stack = vec![start];
        let mut count = 0;
        while let Some(cell) = stack.pop() {
            let index = cell.1 * self.width + cell.0;
            if seen[index] {
                continue;
            }
            seen[index] = true;
            count += 1;
            stack.extend(self.neighbours(cell));
        }
        count
    }

    fn step(&mut self) {
        let Some(head) = self.body.front().copied() else {
            self.restart();
            return;
        };
        let blocked = self.blocked();
        let towards_food = self.step_towards(head, self.food, &blocked).filter(|next| {
            // Don't take food in a spot it couldn't then get out of.
            *next != self.food || self.room_around(*next, &blocked) > self.body.len() / 2
        });
        let next = towards_food.or_else(|| {
            self.neighbours(head)
                .into_iter()
                .filter(|cell| !blocked[cell.1 * self.width + cell.0])
                .max_by_key(|cell| self.room_around(*cell, &blocked))
        });
        let Some(next) = next else {
            self.restart();
            return;
        };
        self.body.push_front(next);
        if next == self.food {
            self.place_food();
        } else {
            self.body.pop_back();
        }
    }

    fn color_at(&self, cell: (usize, usize)) -> [u8; 3] {
        if cell == self.food {
            return [235, 60, 50];
        }
        match self.body.iter().position(|part| *part == cell) {
            Some(0) => [170, 255, 170],
            Some(part) => {
                // Fades toward the tail, so you can read which way it's going.
                let along = part as f32 / self.body.len().max(1) as f32;
                [20, (225.0 - along * 130.0) as u8, (85.0 - along * 40.0) as u8]
            }
            None => [6, 12, 9],
        }
    }
}

/// Conway's Game of Life, reseeded when it dies out or settles down.
#[derive(Clone)]
struct Life {
    width: usize,
    height: usize,
    cells: Vec<u16>,
    random: Random,
    generations: u32,
}

impl Life {
    fn new(width: usize, height: usize, seed: u64) -> Self {
        let mut life = Self {
            width,
            height,
            cells: vec![0; width * height],
            random: Random(seed | 1),
            generations: 0,
        };
        life.reseed();
        life
    }

    fn reseed(&mut self) {
        for cell in &mut self.cells {
            *cell = u16::from(self.random.below(100) < 32);
        }
        self.generations = 0;
    }

    fn neighbours(&self, x: usize, y: usize) -> usize {
        let mut count = 0;
        for dy in [self.height - 1, 0, 1] {
            for dx in [self.width - 1, 0, 1] {
                if (dx, dy) != (0, 0) {
                    let (nx, ny) = ((x + dx) % self.width, (y + dy) % self.height);
                    count += usize::from(self.cells[ny * self.width + nx] > 0);
                }
            }
        }
        count
    }

    fn step(&mut self) {
        let mut next = vec![0u16; self.cells.len()];
        for y in 0..self.height {
            for x in 0..self.width {
                let index = y * self.width + x;
                let around = self.neighbours(x, y);
                let age = self.cells[index];
                next[index] = match (age > 0, around) {
                    (true, 2 | 3) => age.saturating_add(1),
                    (false, 3) => 1,
                    _ => 0,
                };
            }
        }
        let settled = next.iter().map(|age| *age > 0).eq(self.cells.iter().map(|age| *age > 0));
        let extinct = next.iter().all(|age| *age == 0);
        self.cells = next;
        self.generations += 1;
        if settled || extinct || self.generations > LIFE_MOST_GENERATIONS {
            self.reseed();
        }
    }

    fn color_at(&self, x: usize, y: usize) -> [u8; 3] {
        match self.cells[y * self.width + x] {
            0 => [5, 7, 12],
            // New cells are bright, and old ones settle into a deeper blue.
            age => rainbow(0.45 + f32::from(age.min(14)) * 0.012),
        }
    }
}

enum Game {
    None,
    Snake(Snake),
    Life(Life),
}

/// One running screen saver, on a grid of `cols` by `rows` keys.
pub struct Saver {
    pub animation: SaverAnimation,
    rows: usize,
    cols: usize,
    speed: f32,
    time: f32,
    steps_done: u64,
    game: Game,
}

impl Saver {
    pub fn new(config: SaverConfig, rows: u32, cols: u32) -> Self {
        let (rows, cols) = ((rows as usize).max(1), (cols as usize).max(1));
        let (width, height) = (cols * CELLS_PER_KEY, rows * CELLS_PER_KEY);
        // Fixed seeds: a deck and the window started together play the same game.
        let game = match config.animation {
            SaverAnimation::Snake => Game::Snake(Snake::new(width, height, 0x5EED_0001)),
            SaverAnimation::Life => Game::Life(Life::new(width, height, 0x5EED_0002)),
            _ => Game::None,
        };
        Self {
            animation: config.animation,
            rows,
            cols,
            speed: f32::from(config.speed_percent) / 100.0,
            time: 0.0,
            steps_done: 0,
            game,
        }
    }

    /// Moves to this many seconds after starting.
    pub fn set_time(&mut self, seconds: f32) {
        self.time = seconds * self.speed;
        let rate = match self.game {
            Game::Snake(_) => SNAKE_STEPS_PER_SECOND,
            Game::Life(_) => LIFE_STEPS_PER_SECOND,
            Game::None => return,
        };
        // A slow frame catches up rather than slowing the game down, up to a limit.
        let due = (self.time * rate) as u64;
        let to_run = due.saturating_sub(self.steps_done).min(32);
        self.steps_done = due;
        for _ in 0..to_run {
            match &mut self.game {
                Game::Snake(snake) => snake.step(),
                Game::Life(life) => life.step(),
                Game::None => {}
            }
        }
    }

    /// How bright the deck's light should be, as a percent of the person's setting.
    pub fn brightness_percent(&self) -> u8 {
        if self.animation != SaverAnimation::Breathing {
            return 100;
        }
        let wave = 0.5 - 0.5 * (self.time / BREATH_SECONDS * std::f32::consts::TAU).cos();
        (DIMMEST_BREATH + (100.0 - DIMMEST_BREATH) * wave) as u8
    }

    /// The colour at a point on the keys.
    pub fn color_at(&self, x: f32, y: f32) -> [u8; 3] {
        let (cols, rows) = (self.cols as f32, self.rows as f32);
        match self.animation {
            SaverAnimation::Breathing => {
                // A soft ember glow. The deck's light does the breathing.
                let glow = 0.72 + 0.28 * (1.0 - (x / cols - 0.5).abs() - (y / rows - 0.5).abs());
                scale([255, 118, 40], glow)
            }
            SaverAnimation::RainbowSideways => rainbow(x / cols - self.time * 0.25),
            SaverAnimation::RainbowUpDown => rainbow(y / rows - self.time * 0.25),
            SaverAnimation::Snake | SaverAnimation::Life => {
                let cell_x = ((x * CELLS_PER_KEY as f32) as usize).min(self.cols * CELLS_PER_KEY - 1);
                let cell_y = ((y * CELLS_PER_KEY as f32) as usize).min(self.rows * CELLS_PER_KEY - 1);
                match &self.game {
                    Game::Snake(snake) => snake.color_at((cell_x, cell_y)),
                    Game::Life(life) => life.color_at(cell_x, cell_y),
                    Game::None => [0, 0, 0],
                }
            }
            SaverAnimation::Campfire => {
                let height = y / rows;
                let flicker = noise(x * 1.4, y * 1.4 - self.time * 1.6) * 0.6 + noise(x * 3.1, y * 3.1 - self.time * 3.0) * 0.4;
                // Hotter toward the bottom middle, the way a fire sits.
                let centered = 1.0 - (x / cols - 0.5).abs() * 0.9;
                fire_color((height * 1.25 - 0.28) * centered + (flicker - 0.5) * 0.85)
            }
            SaverAnimation::DigitalRain => {
                let column = (x * CELLS_PER_KEY as f32).floor();
                let speed = 0.9 + hash(column as u32, 7) * 1.8;
                let offset = hash(column as u32, 11) * rows * 3.0;
                let span = rows * 2.0;
                let head = (self.time * speed + offset).rem_euclid(span);
                let behind = (head - y).rem_euclid(span);
                if behind < 0.14 {
                    [205, 255, 215]
                } else {
                    let tail = 1.0 - behind / (rows * 0.95);
                    [0, (tail.clamp(0.0, 1.0) * 215.0) as u8 + 8, (tail.clamp(0.0, 1.0) * 70.0) as u8 + 4]
                }
            }
            SaverAnimation::Stars => {
                let (cell_x, cell_y) = ((x * 4.0).floor() as u32, (y * 4.0).floor() as u32);
                let is_star = hash(cell_x, cell_y) > 0.86;
                if !is_star {
                    return [3, 4, 10];
                }
                // Each star sits at its own spot in its square, twinkling at its own pace.
                let (spot_x, spot_y) = (hash(cell_x, cell_y + 91) * 0.5 + 0.25, hash(cell_x + 91, cell_y) * 0.5 + 0.25);
                let distance = ((x * 4.0).fract() - spot_x).hypot((y * 4.0).fract() - spot_y);
                let twinkle = 0.5 + 0.5 * (self.time * (0.8 + hash(cell_y, cell_x) * 2.2) + hash(cell_x, 3) * std::f32::consts::TAU).sin();
                let glow = (1.0 - distance * 4.2).clamp(0.0, 1.0) * twinkle;
                [(3.0 + glow * 225.0) as u8, (4.0 + glow * 228.0) as u8, (10.0 + glow * 245.0) as u8]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(animation: SaverAnimation) -> SaverConfig {
        SaverConfig {
            animation,
            start_now: true,
            wait_minutes: 5,
            speed_percent: 100,
        }
    }

    fn settings(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn the_form_turns_into_the_right_settings() {
        let read = config_from(&settings(json!({
            "animation": "Snake", "start_now": true, "wait_minutes": 12, "speed": "fast"
        })));
        assert_eq!(read, SaverConfig { animation: SaverAnimation::Snake, start_now: true, wait_minutes: 12, speed_percent: 200 });

        let defaults = config_from(&Map::new());
        assert_eq!(defaults.animation, SaverAnimation::RainbowSideways);
        assert!(!defaults.start_now);
        assert_eq!(defaults.wait_minutes, DEFAULT_WAIT_MINUTES);

        // Nothing to wait for means it can only start right away.
        assert!(config_from(&settings(json!({"wait_minutes": 0}))).start_now);
        assert_eq!(config_from(&settings(json!({"wait_minutes": 999_999}))).wait_minutes, 1440);
        assert_eq!(config_from(&settings(json!({"animation": "Nonsense"}))).animation, SaverAnimation::RainbowSideways);
    }

    #[test]
    fn the_action_offers_every_animation() {
        let info = ScreenSaverPlugin::default().info();
        let FieldKind::Select { options } = &info.actions[0].fields[0].kind else {
            panic!("the first field should be the animation list");
        };
        for wanted in ["Breathing", "Rainbow: side to side", "Rainbow: up and down", "Snake", "Campfire"] {
            assert!(options.iter().any(|o| o == wanted), "{wanted}");
        }
        assert_eq!(format!("{}/{}", info.id, info.actions[0].id), ACTION_ID);
    }

    #[test]
    fn the_side_to_side_wave_changes_across_and_moves() {
        let mut saver = Saver::new(config(SaverAnimation::RainbowSideways), 4, 8);
        saver.set_time(1.0);
        assert_ne!(saver.color_at(0.5, 1.0), saver.color_at(5.0, 1.0), "different across");
        assert_eq!(saver.color_at(2.0, 0.5), saver.color_at(2.0, 3.5), "the same up and down");
        let before = saver.color_at(2.0, 1.0);
        saver.set_time(2.0);
        assert_ne!(before, saver.color_at(2.0, 1.0), "it moves");
    }

    #[test]
    fn the_up_and_down_wave_changes_down_the_deck() {
        let mut saver = Saver::new(config(SaverAnimation::RainbowUpDown), 4, 8);
        saver.set_time(1.0);
        assert_ne!(saver.color_at(1.0, 0.5), saver.color_at(1.0, 2.5), "different down");
        assert_eq!(saver.color_at(0.5, 1.0), saver.color_at(7.5, 1.0), "the same across");
    }

    #[test]
    fn breathing_rises_and_falls_without_going_dark() {
        let mut saver = Saver::new(config(SaverAnimation::Breathing), 4, 8);
        let mut levels = Vec::new();
        for tenth in 0..=60 {
            saver.set_time(tenth as f32 / 10.0);
            levels.push(saver.brightness_percent());
        }
        let (low, high) = (*levels.iter().min().unwrap(), *levels.iter().max().unwrap());
        assert!(low >= DIMMEST_BREATH as u8 - 1 && low < 20, "{low}");
        assert!(high >= 98, "{high}");
        // Only breathing touches the deck's light.
        let rainbow = Saver::new(config(SaverAnimation::RainbowSideways), 4, 8);
        assert_eq!(rainbow.brightness_percent(), 100);
    }

    #[test]
    fn speed_changes_how_fast_it_plays() {
        let mut slow = Saver::new(SaverConfig { speed_percent: 50, ..config(SaverAnimation::RainbowSideways) }, 4, 8);
        let mut fast = Saver::new(SaverConfig { speed_percent: 200, ..config(SaverAnimation::RainbowSideways) }, 4, 8);
        slow.set_time(1.0);
        fast.set_time(1.0);
        assert_ne!(slow.color_at(1.0, 1.0), fast.color_at(1.0, 1.0));
    }

    #[test]
    fn the_snake_plays_by_the_rules_and_eats() {
        let mut snake = Snake::new(24, 12, 0x5EED_0001);
        let mut longest = 0;
        let mut previous_head = snake.body[0];
        for _ in 0..4000 {
            let length = snake.body.len();
            snake.step();
            let head = snake.body[0];
            if snake.body.len() >= length && length > 0 {
                // One square at a time, never through itself or off the board.
                let distance = head.0.abs_diff(previous_head.0) + head.1.abs_diff(previous_head.1);
                assert!(distance == 1 || snake.body.len() == 1, "jumped {distance}");
            }
            let mut cells: Vec<_> = snake.body.iter().copied().collect();
            cells.sort_unstable();
            cells.dedup();
            assert_eq!(cells.len(), snake.body.len(), "it ran through itself");
            assert!(snake.body.iter().all(|(x, y)| *x < 24 && *y < 12));
            assert!(!snake.body.contains(&snake.food), "food under the snake");
            longest = longest.max(snake.body.len());
            previous_head = head;
        }
        assert!(longest > 15, "it should get good at this, reached {longest}");
    }

    #[test]
    fn the_snake_starts_a_new_game_when_stuck() {
        // Too small a board to last, on purpose.
        let mut snake = Snake::new(3, 3, 7);
        for _ in 0..2000 {
            snake.step();
            assert!(!snake.body.is_empty());
        }
    }

    #[test]
    fn the_game_of_life_keeps_going() {
        let mut life = Life::new(24, 12, 3);
        for _ in 0..2000 {
            life.step();
            assert!(life.cells.iter().any(|age| *age > 0), "it should reseed instead of ending");
        }
    }

    #[test]
    fn a_blinker_follows_the_rules_of_life() {
        let mut life = Life::new(6, 6, 3);
        life.cells.fill(0);
        for x in 1..=3 {
            life.cells[2 * 6 + x] = 1;
        }
        life.step();
        let alive: Vec<usize> = (0..36).filter(|index| life.cells[*index] > 0).collect();
        assert_eq!(alive, vec![6 + 2, 2 * 6 + 2, 3 * 6 + 2]);
    }

    #[test]
    fn the_fire_is_hotter_at_the_bottom() {
        let mut saver = Saver::new(config(SaverAnimation::Campfire), 4, 8);
        saver.set_time(3.0);
        let total = |y: f32| -> u32 {
            (0..64).map(|i| saver.color_at(i as f32 / 8.0, y).iter().map(|c| u32::from(*c)).sum::<u32>()).sum()
        };
        assert!(total(3.8) > total(0.2) * 2, "{} and {}", total(3.8), total(0.2));
    }

    #[test]
    fn every_animation_draws_something_and_changes() {
        for animation in SaverAnimation::ALL {
            let mut saver = Saver::new(config(*animation), 4, 8);
            saver.set_time(2.0);
            let frame = |saver: &Saver| -> Vec<[u8; 3]> {
                (0..32).flat_map(|y| (0..64).map(move |x| (x, y))).map(|(x, y)| saver.color_at(x as f32 / 8.0, y as f32 / 8.0)).collect()
            };
            let first = frame(&saver);
            assert!(first.iter().any(|c| c.iter().any(|v| *v > 90)), "{animation:?} is blank");
            saver.set_time(5.0);
            let later = frame(&saver);
            let changed = animation == &SaverAnimation::Breathing && saver.brightness_percent() != 100;
            assert!(first != later || changed, "{animation:?} doesn't move");
        }
    }

    #[test]
    fn a_painted_frame_covers_the_whole_deck_and_says_how_bright() {
        let plugin = ScreenSaverPlugin::default();
        let request = |millis: u64, restart: bool, animation: &str| PaintRequest {
            settings: settings(json!({"animation": animation})),
            rows: 4,
            cols: 8,
            blocks_per_key: 6,
            millis,
            restart,
        };
        let frame = plugin.paint(&request(1000, true, "Rainbow: side to side")).unwrap();
        assert_eq!(frame.len(), 1 + 48 * 24 * 3);
        assert_eq!(frame[0], 100);
        assert_ne!(frame[1..4], frame[1 + 47 * 3..1 + 48 * 3], "the wave crosses the deck");

        let later = plugin.paint(&request(2000, false, "Rainbow: side to side")).unwrap();
        assert_ne!(frame, later, "it moves");

        let breathing = plugin.paint(&request(0, true, "Breathing")).unwrap();
        assert!(breathing[0] < 20, "a restart picks up the new animation: {}", breathing[0]);
    }

    #[test]
    fn it_copes_with_the_smallest_grid_and_points_at_the_edge() {
        for animation in SaverAnimation::ALL {
            let mut saver = Saver::new(config(*animation), 1, 1);
            for second in 0..40 {
                saver.set_time(second as f32);
                let _ = saver.color_at(0.0, 0.0);
                let _ = saver.color_at(1.0, 1.0);
                let _ = saver.color_at(0.999, 0.999);
            }
        }
    }
}
