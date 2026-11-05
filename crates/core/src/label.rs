//! Where a key's title goes and how big it is, worked out the same way for the window and the deck.

/// Each line of a title takes this many times its size, top to bottom.
pub const LINE_HEIGHT: f32 = 1.15;

/// Which part of the key the title sits against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TitlePosition {
    Top,
    Middle,
    #[default]
    Bottom,
}

/// The parts of a key, each as a fraction of its height from the top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyLayout {
    pub label_top: f32,
    pub label_bottom: f32,
    pub icon_center: f32,
    /// The title sits on top of the icon, so the icon stands back.
    pub dim_icon: bool,
}

impl TitlePosition {
    pub const ALL: [TitlePosition; 3] = [TitlePosition::Top, TitlePosition::Middle, TitlePosition::Bottom];

    /// From a key's setting. Anything else counts as the bottom, where titles have always been.
    pub fn from_setting(text: &str) -> Self {
        match text {
            "top" => TitlePosition::Top,
            "middle" => TitlePosition::Middle,
            _ => TitlePosition::Bottom,
        }
    }

    /// What it's called in a key's setting.
    pub fn setting(self) -> &'static str {
        match self {
            TitlePosition::Top => "top",
            TitlePosition::Middle => "middle",
            TitlePosition::Bottom => "bottom",
        }
    }

    /// Without an icon to make room for, a title can have most of the key.
    pub fn layout(self, has_icon: bool) -> KeyLayout {
        let (label_top, label_bottom, icon_center) = match (self, has_icon) {
            (TitlePosition::Top, true) => (0.06, 0.38, 0.66),
            (TitlePosition::Middle, true) => (0.14, 0.86, 0.5),
            (TitlePosition::Bottom, true) => (0.62, 0.94, 0.36),
            (TitlePosition::Top, false) => (0.06, 0.72, 0.5),
            (TitlePosition::Middle, false) => (0.06, 0.94, 0.5),
            (TitlePosition::Bottom, false) => (0.28, 0.94, 0.5),
        };
        KeyLayout { label_top, label_bottom, icon_center, dim_icon: self == TitlePosition::Middle && has_icon }
    }

    /// Where the first line starts, given a title this tall, as a distance from the top of the key.
    pub fn first_line_at(self, layout: KeyLayout, key_height: f32, text_height: f32) -> f32 {
        let (top, bottom) = (layout.label_top * key_height, layout.label_bottom * key_height);
        match self {
            TitlePosition::Top => top,
            TitlePosition::Middle => (top + bottom - text_height) / 2.0,
            TitlePosition::Bottom => bottom - text_height,
        }
    }
}

/// A title's size, with the lines it takes up at that size.
#[derive(Debug, Clone, PartialEq)]
pub struct FittedLabel {
    pub size: f32,
    pub lines: Vec<String>,
}

/// Splits text into lines at its spaces, or says it can't fit: a word is too wide, or the lines too many.
fn wrap(text: &str, size: f32, width: f32, height: f32, width_of: &dyn Fn(&str, f32) -> f32) -> Option<Vec<String>> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        if width_of(word, size) > width {
            return None;
        }
        match lines.last_mut() {
            Some(line) if width_of(&format!("{line} {word}"), size) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    (lines.len() as f32 * size * LINE_HEIGHT <= height).then_some(lines)
}

/// At the smallest size, for text that still doesn't fit: breaks anywhere, and stops when out of room.
fn break_anywhere(text: &str, size: f32, width: f32, height: f32, width_of: &dyn Fn(&str, f32) -> f32) -> Vec<String> {
    let most_lines = ((height / (size * LINE_HEIGHT)) as usize).max(1);
    let mut lines = vec![String::new()];
    for letter in text.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        let line = lines.last_mut().map_or_else(String::new, |line| format!("{line}{letter}"));
        if width_of(&line, size) <= width {
            if let Some(last) = lines.last_mut() {
                *last = line;
            }
        } else if lines.len() < most_lines {
            lines.push(letter.to_string());
        } else {
            break;
        }
    }
    lines
}

/// The biggest size that fits this text in the box, without going over `largest` or under
/// `smallest`. `width_of` measures text at a size, in whatever font is being drawn with.
pub fn fit_label(text: &str, width: f32, height: f32, (smallest, largest): (f32, f32), width_of: &dyn Fn(&str, f32) -> f32) -> FittedLabel {
    let (mut low, mut high) = (smallest.ceil() as u32, (largest.min(height / LINE_HEIGHT).floor() as u32).max(smallest.ceil() as u32));
    let mut best = None;
    while low <= high {
        let middle = (low + high) / 2;
        match wrap(text, middle as f32, width, height, width_of) {
            Some(lines) => {
                best = Some(FittedLabel { size: middle as f32, lines });
                low = middle + 1;
            }
            None if middle == 0 => break,
            None => high = middle - 1,
        }
    }
    best.unwrap_or_else(|| FittedLabel { size: smallest, lines: break_anywhere(text, smallest, width, height, width_of) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font where every letter is half as wide as the text is tall.
    fn width_of(text: &str, size: f32) -> f32 {
        text.chars().count() as f32 * size * 0.5
    }

    fn fit(text: &str) -> FittedLabel {
        fit_label(text, 88.0, 64.0, (8.0, 200.0), &width_of)
    }

    #[test]
    fn a_short_title_is_big_and_a_long_one_is_small() {
        let (one_letter, long) = (fit("A"), fit("ireallylovepokemon"));
        assert_eq!(one_letter.lines, ["A"]);
        assert_eq!(one_letter.size, 55.0, "as tall as there's room for");
        assert_eq!(long.lines, ["ireallylovepokemon"], "kept whole, as it fits at this size");
        assert!(long.size < 10.0, "{}", long.size);
    }

    #[test]
    fn words_wrap_onto_lines_when_that_lets_them_be_bigger() {
        let wrapped = fit("All Systems Operational");
        assert_eq!(wrapped.lines, ["All Systems", "Operational"]);
        assert!(wrapped.size > 88.0 / 23.0 / 0.5, "bigger than on one line: {}", wrapped.size);
    }

    #[test]
    fn it_stays_inside_the_box_whatever_the_text() {
        for text in ["A", "Ab", "Play", "New Tab", "two words here", "ireallylovepokemon", &"long ".repeat(9)] {
            let fitted = fit(text);
            assert!(fitted.size >= 8.0, "{text}");
            assert!(fitted.lines.len() as f32 * fitted.size * LINE_HEIGHT <= 64.0, "{text}: {fitted:?}");
            for line in &fitted.lines {
                assert!(width_of(line, fitted.size) <= 88.0, "{text}: {fitted:?}");
            }
        }
    }

    #[test]
    fn text_too_long_for_the_smallest_size_breaks_up_and_stops_in_time() {
        let fitted = fit(&"x".repeat(500));
        assert_eq!(fitted.size, 8.0);
        assert_eq!(fitted.lines.len(), 6);
        assert!(fitted.lines.iter().all(|line| width_of(line, 8.0) <= 88.0));
    }

    #[test]
    fn nothing_much_to_fit_is_fine() {
        assert!(fit("").lines.is_empty());
        assert!(fit("   ").lines.is_empty());
        let tiny = fit_label("Hi", 1.0, 1.0, (8.0, 200.0), &width_of);
        assert_eq!(tiny.size, 8.0);
    }

    #[test]
    fn each_position_sits_where_it_says_and_makes_room_for_an_icon() {
        for position in TitlePosition::ALL {
            assert_eq!(TitlePosition::from_setting(position.setting()), position);
        }
        assert_eq!(TitlePosition::from_setting(""), TitlePosition::Bottom);
        assert_eq!(TitlePosition::from_setting("sideways"), TitlePosition::Bottom);

        let at = |position: TitlePosition, has_icon| position.first_line_at(position.layout(has_icon), 100.0, 20.0);
        assert!(at(TitlePosition::Top, false) < at(TitlePosition::Middle, false));
        assert!(at(TitlePosition::Middle, false) < at(TitlePosition::Bottom, false));
        let (top, bottom) = (TitlePosition::Top.layout(true), TitlePosition::Bottom.layout(true));
        assert!(top.label_bottom < top.icon_center, "the icon moves below a title at the top");
        assert!(bottom.icon_center < bottom.label_top, "and above one at the bottom");
        assert!(TitlePosition::Middle.layout(true).dim_icon);
        assert!(!TitlePosition::Middle.layout(false).dim_icon);
    }
}
