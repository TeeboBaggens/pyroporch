//! A text buffer for passwords: masked on screen, wiped from memory when dropped or cleared.

use eframe::egui::{self, TextBuffer, TextEdit, Ui};
use std::ops::Range;
use zeroize::Zeroize;

/// Reserved up front so that typing doesn't leave old copies behind when the string has to grow.
const CAPACITY: usize = 4096;

pub struct SecretBuffer(String);

impl Default for SecretBuffer {
    fn default() -> Self {
        Self(String::with_capacity(CAPACITY))
    }
}

impl SecretBuffer {
    pub fn text(&self) -> &str {
        &self.0
    }
    pub fn wipe(&mut self) {
        self.0.zeroize();
        self.0.reserve(CAPACITY);
    }
}

impl Drop for SecretBuffer {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl TextBuffer for SecretBuffer {
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        &self.0
    }
    fn insert_text(&mut self, text: &str, char_index: egui::text::CharIndex) -> usize {
        self.0.insert_text(text, char_index)
    }
    fn delete_char_range(&mut self, char_range: Range<egui::text::CharIndex>) {
        self.0.delete_char_range(char_range);
    }
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
}

/// A masked single-line field with its label above it.
pub fn password_field(ui: &mut Ui, id: &str, buffer: &mut SecretBuffer, label: &str, width: f32) -> egui::Response {
    let label = ui.label(egui::RichText::new(label).small());
    labelled_password_field(ui, id, buffer, label.id, width)
}

/// A masked single-line field for a label that's already on screen.
pub fn labelled_password_field(ui: &mut Ui, id: &str, buffer: &mut SecretBuffer, label: egui::Id, width: f32) -> egui::Response {
    ui.add(TextEdit::singleline(buffer).id_salt(id).password(true).desired_width(width))
        .labelled_by(label)
}

/// Clears the text field's undo history, which keeps its own copies of what was typed.
pub fn forget_field(ui: &Ui, response_id: egui::Id) {
    egui::text_edit::TextEditState::default().store(ui.ctx(), response_id);
}
