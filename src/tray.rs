//! The menu bar or system tray item, for reaching the app while it runs in the background.
//!
//! Linux has no tray here: it would need all of GTK. Starting the app again brings its window back,
//! and `pyroporch --quit` closes it.

pub enum Request {
    Show,
    Quit,
}

#[cfg(not(target_os = "linux"))]
mod native {
    use super::Request;
    use std::sync::mpsc::{Receiver, channel};
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    const ICON_SIZE: u32 = 44;

    fn picture(style: &str) -> Result<Icon, String> {
        let image = match style {
            "white" => crate::icon::render_mark_tinted(ICON_SIZE, 255),
            "black" => crate::icon::render_mark_tinted(ICON_SIZE, 0),
            _ => crate::icon::render_mark(ICON_SIZE),
        };
        Icon::from_rgba(image.into_raw(), ICON_SIZE, ICON_SIZE).map_err(|e| e.to_string())
    }

    pub struct Tray {
        _icon: TrayIcon,
        show: MenuId,
        quit: MenuId,
        events: Receiver<MenuEvent>,
    }

    impl Tray {
        pub fn new(ctx: &eframe::egui::Context, style: &str) -> Result<Self, String> {
            let show = MenuItem::new("Open Pyro Porch", true, None);
            let quit = MenuItem::new("Quit Pyro Porch", true, None);
            let menu = Menu::new();
            menu.append(&show).map_err(|e| e.to_string())?;
            menu.append(&quit).map_err(|e| e.to_string())?;

            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("Pyro Porch")
                .with_icon(picture(style)?)
                .build()
                .map_err(|e| e.to_string())?;

            let (sender, events) = channel();
            let wake = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event| {
                let _ = sender.send(event);
                wake.request_repaint();
            }));
            Ok(Self {
                _icon: tray,
                show: show.id().clone(),
                quit: quit.id().clone(),
                events,
            })
        }

        /// Changes the icon to `color`, `white` or `black`.
        pub fn set_style(&self, style: &str) -> Result<(), String> {
            self._icon.set_icon(Some(picture(style)?)).map_err(|e| e.to_string())
        }

        pub fn poll(&self) -> Option<Request> {
            while let Ok(event) = self.events.try_recv() {
                if *event.id() == self.show {
                    return Some(Request::Show);
                }
                if *event.id() == self.quit {
                    return Some(Request::Quit);
                }
            }
            None
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use native::Tray;

#[cfg(target_os = "linux")]
pub struct Tray;

#[cfg(target_os = "linux")]
impl Tray {
    pub fn new(_ctx: &eframe::egui::Context, _style: &str) -> Result<Self, String> {
        Ok(Self)
    }
    pub fn set_style(&self, _style: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn poll(&self) -> Option<Request> {
        None
    }
}
