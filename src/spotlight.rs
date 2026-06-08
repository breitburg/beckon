// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! The Spotlight-style entry: a compact, borderless prompt that opens the
//! selected AI service with the typed message.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Application, Box as GtkBox, Button, Entry, EventControllerKey, Orientation, Window};

use crate::blur;
use crate::config::Config;
use crate::settings_window;

/// Build, wire up and present the entry window. Returns the window so the
/// caller can track and toggle it.
pub fn present(app: &Application, config: &Rc<RefCell<Config>>) -> Window {
    let window = Window::builder()
        .application(app)
        .decorated(false)
        .resizable(false)
        .default_width(600)
        .build();
    window.set_widget_name("spotlight");
    window.add_css_class("spotlight");

    // The card carries the rounded background and shadow; the surrounding
    // window stays transparent so the corners read as rounded and the shadow
    // has a gutter to render into.
    let card = GtkBox::builder().orientation(Orientation::Horizontal).build();
    card.add_css_class("spotlight-card");

    let entry = Entry::builder()
        .placeholder_text("Your message…")
        .primary_icon_name("edit-find-symbolic")
        .primary_icon_activatable(false)
        .has_frame(false)
        .hexpand(true)
        .build();
    entry.add_css_class("spotlight-entry");
    card.append(&entry);

    // Settings shortcut at the trailing edge of the field.
    let settings_button = Button::from_icon_name("applications-system-symbolic");
    settings_button.add_css_class("flat");
    settings_button.set_valign(Align::Center);
    settings_button.set_tooltip_text(Some("Settings"));
    card.append(&settings_button);

    window.set_child(Some(&card));

    {
        let app = app.clone();
        let config = config.clone();
        let window = window.clone();
        settings_button.connect_clicked(move |_| {
            settings_window::present(&app, &config);
            window.close();
        });
    }

    // Enter → open the service with the typed message.
    let service = config.borrow().current_service().cloned();
    let window_for_activate = window.clone();
    entry.connect_activate(move |entry| {
        let text = entry.text();
        let message = text.trim();
        if message.is_empty() {
            return;
        }
        if let Some(service) = &service {
            let encoded = glib::Uri::escape_string(message, None, false);
            let url = service.url_for(&encoded);
            // Synchronous launch: not tied to this window, so closing it
            // immediately afterwards can't cancel the open.
            if let Err(err) =
                gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>)
            {
                eprintln!("Could not open URL: {err}");
            }
        }
        window_for_activate.close();
    });

    // Esc → dismiss.
    let key_controller = EventControllerKey::new();
    let window_for_key = window.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            window_for_key.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    // Clicking away (losing focus) → dismiss.
    window.connect_is_active_notify(|window| {
        if !window.is_active() {
            window.close();
        }
    });

    // Ask the compositor to blur the desktop behind the card (Pantheon shell).
    window.connect_map(|window| blur::apply(window));

    window.present();
    entry.grab_focus();
    window
}
