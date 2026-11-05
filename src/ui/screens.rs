//! The other screens: plugins, the vault, the assistant connection and settings, plus dialogs.

use super::secret_field::{forget_field, labelled_password_field, password_field};
use super::theme::{Palette, color_hex, parse_color};
use super::{App, ImportDialog, SecretDialog, SecretTarget, View};
use eframe::egui::{self, Color32, RichText, Ui};
use pyroporch_core::policy::{Grants, Pending};
use pyroporch_core::transfer::{self, FILE_EXTENSION};
use pyroporch_core::vault::{MIN_PASSPHRASE_CHARS, VaultState};
use serde_json::Value;

const SCREEN_WIDTH: f32 = 720.0;

fn screen(ui: &mut Ui, p: Palette, title: &str, intro: &str, contents: impl FnOnce(&mut Ui)) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(p.bg).inner_margin(24))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.allocate_ui(egui::vec2(SCREEN_WIDTH, ui.available_height()), |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.set_max_width(SCREEN_WIDTH);
                            ui.heading(title);
                            ui.label(RichText::new(intro).color(p.muted));
                            ui.add_space(12.0);
                            contents(ui);
                        });
                    });
                });
            });
        });
}

fn card(ui: &mut Ui, p: Palette, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(p.panel)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(8)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
    ui.add_space(10.0);
}

fn primary(ui: &mut Ui, p: Palette, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(p.accent))
}

impl App {
    /// Reads the plugins folder again, and says what it found.
    pub fn refresh_plugin_folder(&mut self) {
        self.engine.refresh_plugins();
        let statuses = self.engine.plugin_statuses();
        let added: Vec<_> = statuses.iter().filter(|s| !s.builtin).collect();
        let waiting = added.iter().filter(|s| !s.enabled && !s.sha256.is_empty()).count();
        self.plugin_message = if added.is_empty() {
            "The plugins folder is empty.".to_string()
        } else {
            format!("Found {} item(s) in the plugins folder, {waiting} waiting for your approval.", added.len())
        };
    }

    pub fn restore_samples(&mut self) {
        match pyroporch_core::sample_plugin::restore(&mut self.ws) {
            Ok(()) => self.refresh_plugin_folder(),
            Err(error) => self.plugin_message = format!("Couldn't put the samples back: {error}"),
        }
    }

    pub fn remove_plugin(&mut self, id: &str, folder: &str) {
        self.confirm_remove_plugin = None;
        match self.engine.remove_plugin(id, folder) {
            Ok(()) => self.plugin_message = format!("Removed {id}."),
            Err(error) => self.plugin_message = error,
        }
    }

    pub fn plugins_view(&mut self, ui: &mut Ui) {
        let p = self.palette();
        let statuses = self.engine.plugin_statuses();
        let plugins_dir = self.ws.paths.plugins_dir();
        let message = self.plugin_message.clone();
        let mut approve = None;
        let mut disable = None;
        let mut revoke = None;
        let mut open_folder = false;
        let mut open_guide = false;
        let mut refresh = false;
        let mut restore_samples = false;
        let mut remove = None;
        let mut ask_remove = None;
        let samples_missing = !pyroporch_core::sample_plugin::missing(&self.ws.paths).is_empty();
        let confirming = self.confirm_remove_plugin.clone();
        screen(ui, p, "Plugins", "Plugins add new actions that keys can use.", |ui| {
            let mut plugin_card = |ui: &mut Ui, status: &pyroporch_core::engine::PluginStatus| {
                card(ui, p, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&status.name).strong().size(15.0));
                        ui.label(RichText::new(&status.version).small().color(p.muted));
                        let can_approve = !status.sha256.is_empty();
                        let (tag, color) = match (status.builtin, status.enabled, can_approve) {
                            (true, _, _) => ("built in", p.muted),
                            (false, true, _) if status.sample => ("sample, on, in sandbox", p.ok),
                            (false, true, _) => ("on, in sandbox", p.ok),
                            (false, false, true) => ("off", p.warn),
                            (false, false, false) => ("not a plugin yet", p.warn),
                        };
                        ui.label(RichText::new(tag).small().color(color));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !status.builtin {
                                // By folder, because a copied plugin has the same id as the original.
                                let asking = confirming.as_deref() == Some(status.folder.as_str());
                                let label = if asking { "Click again to remove" } else { "Remove" };
                                if ui.button(label).on_hover_text("Takes it out of the plugins folder").clicked() {
                                    if asking {
                                        remove = Some((status.id.clone(), status.folder.clone()));
                                    } else {
                                        ask_remove = Some(status.folder.clone());
                                    }
                                }
                                if status.enabled {
                                    if ui.button("Turn off").clicked() {
                                        disable = Some(status.id.clone());
                                    }
                                } else if can_approve && primary(ui, p, "Approve and turn on").clicked() {
                                    approve = Some(status.id.clone());
                                }
                            }
                        });
                    });
                    if !status.description.is_empty() {
                        ui.label(&status.description);
                    }
                    if !status.note.is_empty() {
                        ui.colored_label(p.warn, &status.note);
                    }
                    if !status.builtin && !status.enabled && !status.sha256.is_empty() {
                        let list = |items: &[String]| if items.is_empty() { "nothing".to_string() } else { items.join(", ") };
                        ui.label(format!("Asks to contact: {}", list(&status.requested.http_hosts)));
                        if status.requested.secrets.iter().any(|name| name == "*") {
                            ui.label("Asks to use secrets: any you choose, each only going to its own sites");
                        } else {
                            ui.label(format!("Asks to use secrets: {}", list(&status.requested.secrets)));
                        }
                        if status.requested.deck {
                            ui.label("Asks to play on the keys and change the deck's brightness");
                        }
                    }
                    if !status.http_hosts.is_empty() {
                        ui.label(RichText::new("Sites you allowed").small().color(p.muted));
                        ui.horizontal_wrapped(|ui| {
                            for host in &status.http_hosts {
                                ui.label(host);
                                if ui.small_button("×").on_hover_text("Stop allowing this site").clicked() {
                                    revoke = Some((status.id.clone(), host.clone()));
                                }
                            }
                        });
                    }
                });
            };

            ui.label(RichText::new("From your plugins folder").strong().size(15.0));
            ui.label(RichText::new("These run in a sandbox, and can only do what you allow.").small().color(p.muted));
            ui.add_space(4.0);
            for status in statuses.iter().filter(|s| !s.builtin) {
                plugin_card(ui, status);
            }
            card(ui, p, |ui| {
                ui.label(RichText::new("Add a plugin").strong());
                ui.label("A plugin is one folder inside the plugins folder, holding a plugin.toml and the plugin's built .wasm file. Source code won't run from there: it has to be built first.");
                ui.horizontal(|ui| {
                    if ui.button("Open the plugins folder").clicked() {
                        open_folder = true;
                    }
                    if ui.button("Refresh").on_hover_text("Read the plugins folder again").clicked() {
                        refresh = true;
                    }
                    if ui.button("Open the plugin guide").on_hover_text("How to write your own").clicked() {
                        open_guide = true;
                    }
                    if samples_missing && ui.button("Restore the sample plugins").on_hover_text("Puts back the samples you removed").clicked() {
                        restore_samples = true;
                    }
                });
                if !message.is_empty() {
                    ui.colored_label(p.ok, &message);
                }
                ui.label(RichText::new("To write your own, start with the guide. Each sample has its source code in its folder, and an AI assistant can read the guide through its get_guide tool.").small().color(p.muted));
            });

            ui.add_space(6.0);
            ui.label(RichText::new("Built into the app").strong().size(15.0));
            ui.label(RichText::new("These are part of the app itself, so you won't find them in the plugins folder.").small().color(p.muted));
            ui.add_space(4.0);
            for status in statuses.iter().filter(|s| s.builtin) {
                plugin_card(ui, status);
            }
        });
        if let Some(id) = approve
            && let Err(error) = self.engine.approve_plugin(&id)
        {
            self.log(false, error);
        }
        if let Some(id) = disable
            && let Err(error) = self.engine.disable_plugin(&id)
        {
            self.log(false, error);
        }
        if let Some((plugin, host)) = revoke {
            if let Err(error) = Grants::revoke_host(&self.ws.paths, &plugin, &host) {
                self.log(false, error);
            }
            self.grants = Grants::load(&self.ws.paths);
            self.engine.refresh_plugins();
        }
        if open_folder {
            self.open_path(&plugins_dir);
        }
        if open_guide {
            self.open_path(&plugins_dir.join(pyroporch_core::sample_plugin::GUIDE_FILE));
        }
        if refresh {
            self.refresh_plugin_folder();
        }
        if restore_samples {
            self.restore_samples();
        }
        if let Some(folder) = ask_remove {
            self.confirm_remove_plugin = Some(folder);
        }
        if let Some((id, folder)) = remove {
            self.remove_plugin(&id, &folder);
        }
    }

    fn open_path(&mut self, path: &std::path::Path) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "explorer"
        } else {
            "xdg-open"
        };
        if let Err(error) = std::process::Command::new(opener).arg(path).spawn() {
            self.log(false, format!("couldn't open the folder: {error}"));
        }
    }

    pub fn vault_view(&mut self, ui: &mut Ui) {
        let p = self.palette();
        let state = self.engine.vault.lock().map_or(VaultState::Locked, |v| v.state());
        let secrets = self.engine.vault.lock().map_or_else(|_| Vec::new(), |v| v.list());
        let engine = self.engine.clone();
        let form = &mut self.vault_form;
        let mut add_secret = None;
        let mut delete_secret = None;
        screen(ui, p, "Vault", "Passwords, API keys and tokens are kept here, encrypted. They never appear in your config files.", |ui| {
            if !form.message.is_empty() {
                let color = if form.is_error { p.bad } else { p.ok };
                ui.colored_label(color, &form.message);
                ui.add_space(6.0);
            }
            match state {
                VaultState::NotCreated => card(ui, p, |ui| {
                    ui.label(RichText::new("Set up your vault").strong());
                    ui.label(format!("Choose a passphrase of at least {MIN_PASSPHRASE_CHARS} characters. It can't be recovered, so keep it somewhere safe."));
                    let first = password_field(ui, "vault_new", &mut form.passphrase, "Passphrase", 300.0);
                    let second = password_field(ui, "vault_confirm", &mut form.confirm, "Same passphrase again", 300.0);
                    if primary(ui, p, "Create vault").clicked() {
                        if form.passphrase.text() != form.confirm.text() {
                            form.message = "The two passphrases don't match.".into();
                            form.is_error = true;
                        } else if let Ok(mut vault) = engine.vault.lock() {
                            match vault.create(form.passphrase.text()) {
                                Ok(()) => {
                                    form.message = "Your vault is ready and open.".into();
                                    form.is_error = false;
                                    form.passphrase.wipe();
                                    form.confirm.wipe();
                                    forget_field(ui, first.id);
                                    forget_field(ui, second.id);
                                }
                                Err(error) => {
                                    form.message = error;
                                    form.is_error = true;
                                }
                            }
                        }
                    }
                }),
                VaultState::Locked => card(ui, p, |ui| {
                    ui.label(RichText::new("The vault is locked").strong());
                    let field = password_field(ui, "vault_unlock", &mut form.passphrase, "Passphrase", 300.0);
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if (primary(ui, p, "Unlock").clicked() || enter)
                            && let Ok(mut vault) = engine.vault.lock()
                        {
                            match vault.unlock(form.passphrase.text()) {
                                Ok(()) => {
                                    form.message.clear();
                                    form.passphrase.wipe();
                                    forget_field(ui, field.id);
                                }
                                Err(error) => {
                                    form.message = error;
                                    form.is_error = true;
                                }
                            }
                        }
                        #[cfg(feature = "os-keychain")]
                        if ui.button("Unlock with this device's keychain").clicked()
                            && let Ok(mut vault) = engine.vault.lock()
                        {
                            match vault.unlock_from_device() {
                                Ok(()) => {
                                    form.message.clear();
                                    form.passphrase.wipe();
                                }
                                Err(error) => {
                                    form.message = error;
                                    form.is_error = true;
                                }
                            }
                        }
                    });
                }),
                VaultState::Unlocked => {
                    card(ui, p, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} stored", secrets.len())).strong());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("🔒 Lock now").clicked()
                                    && let Ok(mut vault) = engine.vault.lock()
                                {
                                    vault.lock();
                                }
                                if primary(ui, p, "+ Add").clicked() {
                                    add_secret = Some(SecretDialog::default());
                                }
                            });
                        });
                        if secrets.is_empty() {
                            ui.label(RichText::new("Nothing stored yet.").color(p.muted));
                        }
                        for secret in &secrets {
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("🔒 {}", secret.name)).strong());
                                ui.label(RichText::new(format!("only goes to {}", secret.allowed_hosts.join(", "))).color(p.muted));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("Delete").clicked() {
                                        delete_secret = Some(secret.name.clone());
                                    }
                                    if ui.small_button("Replace").clicked() {
                                        add_secret = Some(SecretDialog {
                                            name: secret.name.clone(),
                                            hosts: secret.allowed_hosts.join(", "),
                                            ..SecretDialog::default()
                                        });
                                    }
                                });
                            });
                        }
                    });
                    card(ui, p, |ui| {
                        ui.label(RichText::new("Change passphrase").strong());
                        let old = password_field(ui, "vault_old", &mut form.passphrase, "Current passphrase", 300.0);
                        let new = password_field(ui, "vault_next", &mut form.new_passphrase, "New passphrase", 300.0);
                        if ui.button("Change passphrase").clicked()
                            && let Ok(mut vault) = engine.vault.lock()
                        {
                            match vault.change_passphrase(form.passphrase.text(), form.new_passphrase.text()) {
                                Ok(()) => {
                                    form.message = "Passphrase changed.".into();
                                    form.is_error = false;
                                    form.passphrase.wipe();
                                    form.new_passphrase.wipe();
                                    forget_field(ui, old.id);
                                    forget_field(ui, new.id);
                                }
                                Err(error) => {
                                    form.message = error;
                                    form.is_error = true;
                                }
                            }
                        }
                    });
                    #[cfg(feature = "os-keychain")]
                    card(ui, p, |ui| {
                        ui.label(RichText::new("This device").strong());
                        ui.label("Keep the vault's key in the system keychain, so you don't need the passphrase here. Anyone signed in to this computer as you could then unlock it.");
                        ui.horizontal(|ui| {
                            if ui.button("Remember on this device").clicked()
                                && let Ok(vault) = engine.vault.lock()
                            {
                                (form.message, form.is_error) = match vault.remember_on_device() {
                                    Ok(()) => ("This device will remember the vault.".into(), false),
                                    Err(error) => (error, true),
                                };
                            }
                            if ui.button("Forget on this device").clicked()
                                && let Ok(vault) = engine.vault.lock()
                            {
                                (form.message, form.is_error) = match vault.forget_on_device() {
                                    Ok(()) => ("This device no longer remembers the vault.".into(), false),
                                    Err(error) => (error, true),
                                };
                            }
                        });
                    });
                }
            }
            card(ui, p, |ui| {
                ui.label(RichText::new("How your information is protected").strong());
                for line in [
                    "Stored values are encrypted with XChaCha20-Poly1305. They need to be sent later, so a one-way hash wouldn't work.",
                    "The key comes from your passphrase through Argon2id, which is built to make guessing slow and costly. Your passphrase itself is never saved.",
                    "Each secret can only go to the sites you listed. That list is sealed with it, so editing the file breaks the secret instead of redirecting it.",
                    "Plugins and AI assistants only ever handle a placeholder. The real value is filled in as the request leaves.",
                ] {
                    ui.label(format!("• {line}"));
                }
            });
        });
        if let Some(dialog) = add_secret {
            self.secret_dialog = Some(dialog);
        }
        if let Some(name) = delete_secret
            && let Ok(mut vault) = self.engine.vault.lock()
            && let Err(error) = vault.delete(&name)
        {
            self.vault_form.message = error;
            self.vault_form.is_error = true;
        }
    }

    pub fn assistant_view(&mut self, ui: &mut Ui) {
        let p = self.palette();
        let exe = std::env::current_exe().map_or_else(|_| "pyroporch".to_string(), |e| e.display().to_string());
        let snippet = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": {"pyroporch": {"command": exe, "args": ["mcp"]}}
        }))
        .unwrap_or_default();
        let audit_path = self.ws.paths.audit();
        let recent: Vec<String> = std::fs::read_to_string(audit_path)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.split_whitespace().nth(1) == Some("mcp"))
            .rev()
            .take(8)
            .map(|line| line.split_whitespace().skip(2).collect::<Vec<_>>().join(" "))
            .collect();
        let mut changed = false;
        let mcp = &mut self.ws.settings.mcp;
        let security = &mut self.ws.settings.security;
        screen(ui, p, "AI Assistant", "A built-in MCP server lets an AI assistant add keys and pages for you. It runs only when an assistant you set up starts it, and it opens no network port.", |ui| {
            card(ui, p, |ui| {
                ui.label(RichText::new("Connect an assistant").strong());
                ui.label("Add this to your assistant's MCP settings:");
                let label = ui.label(RichText::new("MCP settings").small().color(p.muted));
                let mut shown = snippet.as_str();
                ui.add(egui::TextEdit::multiline(&mut shown).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(5)).labelled_by(label.id);
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(snippet.clone());
                }
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("What it may do").strong());
                changed |= ui.checkbox(&mut mcp.enabled, "Allow assistants to connect").changed();
                changed |= ui.checkbox(&mut mcp.read_only, "Look only, no changes").changed();
                changed |= ui
                    .checkbox(&mut security.mcp_may_use_secrets, "Let keys it presses use vault secrets")
                    .on_hover_text("Off by default. Also needs this device to remember the vault.")
                    .changed();
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("What it can never do").strong());
                for line in [
                    "Read or store a password or key. It can ask you to enter one, and only gets a placeholder back.",
                    "Allow a new site, approve a plugin, or approve a key that acts on your computer. Those wait for you.",
                    "Save a credential in a key's settings. Those requests are refused before anything is written.",
                ] {
                    ui.label(format!("• {line}"));
                }
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("Recent changes by an assistant").strong());
                if recent.is_empty() {
                    ui.label(RichText::new("None yet.").color(p.muted));
                }
                for line in &recent {
                    ui.label(line);
                }
            });
        });
        if changed {
            self.mark_settings_dirty();
        }
    }

    pub fn settings_view(&mut self, ui: &mut Ui) {
        let p = self.palette();
        let root = self.ws.paths.root.clone();
        let errors = self.ws.errors.clone();
        let mut changed = false;
        let mut open_folder = false;
        let mut export = None;
        let mut import = false;
        let mut quit = false;
        let mut sleep_now = false;
        let message = self.transfer_message.clone();
        let active_name = self.profile().map_or_else(String::new, |x| x.name.clone());
        let settings = &mut self.ws.settings;
        screen(ui, p, "Settings", "Everything here is also in plain config files. Edits to those files take effect while the app is running.", |ui| {
            card(ui, p, |ui| {
                ui.label(RichText::new("Appearance").strong());
                egui::Grid::new("appearance").num_columns(2).show(ui, |ui| {
                    ui.label("Theme");
                    ui.horizontal(|ui| {
                        changed |= ui.selectable_value(&mut settings.ui.theme, "dark".to_string(), "Dark").changed();
                        changed |= ui.selectable_value(&mut settings.ui.theme, "light".to_string(), "Light").changed();
                    });
                    ui.end_row();
                    ui.label("Accent colour");
                    let mut accent = parse_color(&settings.ui.accent).unwrap_or(p.accent);
                    if ui.color_edit_button_srgba(&mut accent).changed() {
                        settings.ui.accent = color_hex(accent);
                        changed = true;
                    }
                    ui.end_row();
                    let label = ui.label("Size");
                    changed |= ui.add(egui::Slider::new(&mut settings.ui.scale, 0.75..=1.5)).labelled_by(label.id).drag_stopped();
                    ui.end_row();
                    // Linux has no tray to show it in.
                    if cfg!(not(target_os = "linux")) {
                        ui.label(if cfg!(target_os = "macos") { "Menu bar icon" } else { "Tray icon" });
                        ui.horizontal(|ui| {
                            for (value, name) in [("color", "Colored"), ("white", "White"), ("black", "Black")] {
                                changed |= ui.selectable_value(&mut settings.ui.tray_icon, value.to_string(), name).changed();
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("Security").strong());
                ui.horizontal(|ui| {
                    let label = ui.label("Lock the vault after");
                    changed |= ui.add(egui::DragValue::new(&mut settings.security.auto_lock_minutes).range(0..=1440)).labelled_by(label.id).changed();
                    ui.label("idle minutes (0 means never)");
                });
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("Running").strong());
                changed |= ui.checkbox(&mut settings.run_in_background, "Keep running in the background when the window is closed").changed();
                let reopen = if cfg!(target_os = "linux") {
                    "Your deck keeps working. Start the app again to bring the window back."
                } else {
                    "Your deck keeps working. Bring the window back from the menu bar or system tray."
                };
                ui.label(RichText::new(reopen).small().color(p.muted));
                if ui.button("Quit Pyro Porch").clicked() {
                    quit = true;
                }
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("Back up and share").strong());
                ui.label("Export your keys, folders and pictures to one file, or bring them in from one.");
                ui.horizontal(|ui| {
                    if ui.button("Export everything…").clicked() {
                        export = Some(None);
                    }
                    if ui.button(format!("Export \"{active_name}\"…")).clicked() {
                        export = Some(Some(()));
                    }
                    if ui.button("Import…").clicked() {
                        import = true;
                    }
                });
                ui.label(RichText::new("Passwords and keys are never in the file. Whoever imports it enters their own, and approves its keys and sites again.").small().color(p.muted));
                if !message.is_empty() {
                    ui.colored_label(p.ok, &message);
                }
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("USB deck").strong());
                changed |= ui.checkbox(&mut settings.hardware.enabled, "Look for a USB deck (takes effect at next start)").changed();
                ui.horizontal(|ui| {
                    let label = ui.label("Brightness");
                    changed |= ui.add(egui::Slider::new(&mut settings.hardware.brightness, 0..=100)).labelled_by(label.id).changed();
                });
                if ui.button("Sleep the deck").on_hover_text("Turns every key off. Press any key to wake it. You can also put this on a key, from the Deck list of actions.").clicked() {
                    sleep_now = true;
                }
                ui.label(RichText::new("Close the deck maker's own app first: only one program can hold the device.").small().color(p.muted));
            });
            card(ui, p, |ui| {
                ui.label(RichText::new("Config files").strong());
                ui.label(RichText::new(root.display().to_string()).monospace());
                if ui.button("Open the config folder").clicked() {
                    open_folder = true;
                }
                if errors.is_empty() {
                    ui.colored_label(p.ok, "All config files read correctly.");
                }
                for error in &errors {
                    ui.colored_label(p.bad, error);
                }
            });
        });
        if changed {
            self.mark_settings_dirty();
            self.apply_theme(ui.ctx());
        }
        if open_folder {
            self.open_path(&root);
        }
        if let Some(scope) = export {
            self.choose_export(scope.is_some());
        }
        if import {
            self.choose_import();
        }
        if quit {
            self.quit_now();
        }
        if sleep_now {
            self.asleep = true;
        }
    }

    fn choose_export(&mut self, only_active: bool) {
        let name = format!("pyro-porch.{FILE_EXTENSION}");
        let picked = rfd::FileDialog::new()
            .set_title("Export")
            .set_file_name(name)
            .add_filter("Pyro Porch config", &[FILE_EXTENSION])
            .save_file();
        if let Some(path) = picked {
            self.export_to(&path, only_active);
        }
    }

    pub fn export_to(&mut self, path: &std::path::Path, only_active: bool) {
        let id = self.active_id();
        let result = transfer::export(&self.ws, &self.engine, only_active.then_some(id.as_str()))
            .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
        self.transfer_message = match result {
            Ok(()) => format!("Exported to {}.", path.display()),
            Err(error) => format!("Couldn't export: {error}"),
        };
    }

    fn choose_import(&mut self) {
        let picked = rfd::FileDialog::new().set_title("Import").add_filter("Pyro Porch config", &[FILE_EXTENSION]).pick_file();
        if let Some(path) = picked {
            self.import_from(&path);
        }
    }

    /// Reads a file and asks the person before anything changes.
    pub fn import_from(&mut self, path: &std::path::Path) {
        let read = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|text| transfer::read(&text));
        match read {
            Ok(bundle) => {
                self.import = Some(ImportDialog {
                    bundle,
                    use_appearance: false,
                });
            }
            Err(error) => self.transfer_message = format!("Couldn't import: {error}"),
        }
    }

    pub fn import_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.import.take() else {
            return;
        };
        let p = self.palette();
        let mut go = false;
        let mut keep = true;
        egui::Modal::new(egui::Id::unique("import_dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading("Import this file?");
            let names: Vec<&str> = dialog.bundle.profiles.values().map(|x| x.name.as_str()).collect();
            ui.label(format!("{} profile(s): {}", names.len(), names.join(", ")));
            if !dialog.bundle.presets.is_empty() {
                ui.label(format!("{} preset(s)", dialog.bundle.presets.len()));
            }
            ui.label(RichText::new("They're added alongside what you have, and nothing is replaced. Keys that act on your computer stay off until you approve them.").small().color(p.muted));
            if dialog.bundle.ui.is_some() {
                ui.checkbox(&mut dialog.use_appearance, "Also use its colours and size");
            }
            ui.horizontal(|ui| {
                if primary(ui, p, "Import").clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
            });
        });
        if !go {
            if keep {
                self.import = Some(dialog);
            }
            return;
        }
        let (engine, appearance) = (self.engine.clone(), dialog.use_appearance);
        self.transfer_message = match transfer::apply(&mut self.ws, &engine, dialog.bundle, appearance) {
            Ok(report) => {
                let mut parts = vec![format!("Imported {} profile(s).", report.profiles.len())];
                if report.need_approval > 0 {
                    parts.push(format!("{} key(s) need your approval.", report.need_approval));
                }
                if !report.sites.is_empty() {
                    parts.push(format!("Sites to allow: {}.", report.sites.join(", ")));
                }
                if !report.missing_secrets.is_empty() {
                    parts.push(format!("Secrets to add to your vault: {}.", report.missing_secrets.join(", ")));
                }
                if report.removed_keys > 0 {
                    parts.push(format!("Left out {} item(s) holding a password in plain text.", report.removed_keys));
                }
                parts.join(" ")
            }
            Err(error) => format!("Couldn't import: {error}"),
        };
        self.apply_theme_now(ctx);
    }

    /// The list of things an assistant or a key has asked the person to decide.
    pub fn requests_window(&mut self, ctx: &egui::Context) {
        if !self.show_requests {
            return;
        }
        let p = self.palette();
        let mut open = true;
        let mut allow = None;
        let mut deny = None;
        let mut enter = None;
        let mut dismiss = None;
        egui::Window::new("Waiting for you").open(&mut open).collapsible(false).default_width(460.0).show(ctx, |ui| {
            if self.pending.hosts.is_empty() && self.pending.secrets.is_empty() {
                ui.label("Nothing is waiting.");
            }
            for (index, request) in self.pending.hosts.iter().enumerate() {
                ui.label(RichText::new(format!("Allow {} to contact {}?", request.plugin, request.host)).strong());
                ui.label(RichText::new(format!("Reason given: {}", request.reason)).color(p.muted));
                ui.horizontal(|ui| {
                    if primary(ui, p, "Allow").clicked() {
                        allow = Some(index);
                    }
                    if ui.button("Don't allow").clicked() {
                        deny = Some(index);
                    }
                });
                ui.separator();
            }
            for (index, request) in self.pending.secrets.iter().enumerate() {
                ui.label(RichText::new(format!("Enter the secret {}", request.name)).strong());
                ui.label(RichText::new(format!("Reason given: {} It will only go to {}.", request.reason, request.hosts.join(", "))).color(p.muted));
                ui.horizontal(|ui| {
                    if primary(ui, p, "Enter it").clicked() {
                        enter = Some(index);
                    }
                    if ui.button("Not now").clicked() {
                        dismiss = Some(index);
                    }
                });
                ui.separator();
            }
        });
        // Each answer takes its request out of the file as it is now, not this window's copy of
        // it, so a request that came in since isn't lost.
        if let Some(index) = allow {
            let request = self.pending.hosts[index].clone();
            match Grants::allow_host(&self.ws.paths, &request.plugin, &request.host) {
                Ok(()) => {
                    self.log(true, format!("allowed {}", request.host));
                    let _ = Pending::remove_host(&self.ws.paths, &request.plugin, &request.host);
                }
                Err(error) => self.log(false, error),
            }
            self.grants = Grants::load(&self.ws.paths);
            self.engine.refresh_plugins();
        }
        if let Some(index) = deny {
            let request = self.pending.hosts[index].clone();
            let _ = Pending::remove_host(&self.ws.paths, &request.plugin, &request.host);
            self.log(true, format!("didn't allow {}", request.host));
        }
        if let Some(index) = dismiss {
            let _ = Pending::remove_secret(&self.ws.paths, &self.pending.secrets[index].name);
        }
        if let Some(index) = enter {
            let request = self.pending.secrets[index].clone();
            let _ = Pending::remove_secret(&self.ws.paths, &request.name);
            self.secret_dialog = Some(SecretDialog {
                name: request.name,
                hosts: request.hosts.join(", "),
                reason: request.reason,
                ..SecretDialog::default()
            });
            self.show_requests = false;
        }
        if allow.is_some() || deny.is_some() || dismiss.is_some() || enter.is_some() {
            self.pending = Pending::load(&self.ws.paths);
        }
        if !open {
            self.show_requests = false;
        }
    }

    /// The dialog for storing a secret. It's the one place a value gets typed in.
    pub fn secret_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.secret_dialog.take() else {
            return;
        };
        let p = self.palette();
        let state = self.engine.vault.lock().map_or(VaultState::Locked, |v| v.state());
        let mut keep = true;
        let mut saved = false;
        let mut go_to_vault = false;
        egui::Window::new("Store a secret").collapsible(false).resizable(false).default_width(420.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            if !dialog.reason.is_empty() {
                ui.label(RichText::new(&dialog.reason).color(p.muted));
            }
            match state {
                VaultState::NotCreated => {
                    ui.label("Set up your vault first. It only takes a passphrase.");
                    ui.horizontal(|ui| {
                        if primary(ui, p, "Go to the vault").clicked() {
                            go_to_vault = true;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                VaultState::Locked => {
                    ui.label("The vault is locked.");
                    let field = password_field(ui, "dialog_pass", &mut dialog.passphrase, "Passphrase", 300.0);
                    ui.horizontal(|ui| {
                        if primary(ui, p, "Unlock").clicked()
                            && let Ok(mut vault) = self.engine.vault.lock()
                        {
                            match vault.unlock(dialog.passphrase.text()) {
                                Ok(()) => {
                                    dialog.passphrase.wipe();
                                    dialog.error.clear();
                                    forget_field(ui, field.id);
                                }
                                Err(error) => dialog.error = error,
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                VaultState::Unlocked => {
                    egui::Grid::new("secret_form").num_columns(2).show(ui, |ui| {
                        let label = ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(280.0).hint_text("github_token")).labelled_by(label.id);
                        ui.end_row();
                        let label = ui.label("Value");
                        let field = labelled_password_field(ui, "dialog_value", &mut dialog.value, label.id, 280.0);
                        dialog.value_field = Some(field.id);
                        ui.end_row();
                        let label = ui.label("Only send to");
                        ui.add(egui::TextEdit::singleline(&mut dialog.hosts).desired_width(280.0).hint_text("api.example.com")).labelled_by(label.id);
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if primary(ui, p, "Encrypt and store").clicked() {
                            let hosts: Vec<String> = dialog.hosts.split(',').map(str::to_string).collect();
                            if let Ok(mut vault) = self.engine.vault.lock() {
                                match vault.set(dialog.name.trim(), dialog.value.text(), &hosts) {
                                    Ok(()) => saved = true,
                                    Err(error) => dialog.error = error,
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
            }
            if !dialog.error.is_empty() {
                ui.colored_label(p.bad, &dialog.error);
            }
        });
        if saved {
            self.log(true, format!("stored the secret {} in the vault", dialog.name.trim()));
            if let Some(target) = dialog.target.clone() {
                self.put_secret_reference(&target, dialog.name.trim());
            }
            keep = false;
        }
        if go_to_vault {
            self.view = View::Vault;
            keep = false;
        }
        if !keep {
            // Dropping the dialog wipes the value; this clears the field's undo history too.
            if let Some(field) = dialog.value_field {
                egui::text_edit::TextEditState::default().store(ctx, field);
            }
            return;
        }
        self.secret_dialog = Some(dialog);
    }

    fn put_secret_reference(&mut self, target: &SecretTarget, name: &str) {
        // The key the secret was asked for, not whichever one happens to be selected now.
        let Some(key) = self
            .screen_of_mut(&target.id)
            .and_then(|screen| screen.key_at_mut(target.id.row, target.id.col))
        else {
            return;
        };
        let was_approved = key.is_approved();
        if let Some(step) = key.steps.get_mut(target.step) {
            step.settings.insert(target.field.clone(), Value::String(pyroporch_sdk::secret_ref(name)));
        }
        if was_approved {
            key.approve();
        }
        // The profile the key is in, which may not be the one on screen by now.
        self.dirty_profiles.insert(target.id.profile.clone(), std::time::Instant::now());
    }
}
