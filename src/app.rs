// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Application wiring: single-instance lifecycle, the background hold, the
//! `--spotlight` command line and the global stylesheet.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk::Display;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Application, CssProvider, Settings};

use crate::config::Config;
use crate::keybinding;
use crate::settings_window;
use crate::spotlight;
use crate::APP_ID;

const STYLE: &str = include_str!("../data/style.css");

pub fn build() -> Application {
    let app = Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    // Shared, mutable config for the lifetime of the process.
    let config = Rc::new(RefCell::new(Config::load()));

    app.connect_startup(glib::clone!(
        #[strong]
        config,
        move |app| {
            load_css();
            follow_color_scheme();

            // Stay resident so the hotkey can reach us with no window open.
            std::mem::forget(app.hold());

            // Make sure the system-wide shortcut reflects the saved state.
            let config = config.borrow();
            keybinding::apply(&config.shortcut, config.enabled);

            let quit = gio::SimpleAction::new("quit", None);
            quit.connect_activate(glib::clone!(
                #[weak]
                app,
                move |_, _| app.quit()
            ));
            app.add_action(&quit);
        }
    ));

    app.connect_command_line(glib::clone!(
        #[strong]
        config,
        move |app, cmdline| {
            let wants_spotlight = cmdline
                .arguments()
                .iter()
                .any(|arg| arg.to_string_lossy() == "--spotlight");

            if wants_spotlight {
                toggle_spotlight(app, &config);
            } else if cmdline.is_remote() {
                // The user launched the app again while it was already running
                // (e.g. clicked the icon) — open settings. The initial
                // background launch shows nothing.
                settings_window::present(app, &config);
            }
            0
        }
    ));

    app
}

/// Show the entry, or dismiss it if it is already open (toggle).
fn toggle_spotlight(app: &Application, config: &Rc<RefCell<Config>>) {
    if !config.borrow().enabled {
        return;
    }
    if let Some(window) = app
        .windows()
        .into_iter()
        .find(|w| w.widget_name() == "spotlight" && w.is_visible())
    {
        window.close();
    } else {
        spotlight::present(app, config);
    }
}

/// Follow the desktop light/dark preference, mirroring it exactly: dark only
/// for "prefer-dark", light otherwise. elementary reports plain "default" (not
/// "prefer-light") when leaving dark, so anything but "prefer-dark" must reset
/// to light — otherwise the app stays stuck dark.
fn follow_color_scheme() {
    let Some(settings) = Settings::default() else {
        return;
    };
    let Some(source) = gio::SettingsSchemaSource::default() else {
        return;
    };
    if source.lookup("org.gnome.desktop.interface", true).is_none() {
        return;
    }

    let interface = gio::Settings::new("org.gnome.desktop.interface");
    apply_color_scheme(&interface, &settings);
    interface.connect_changed(
        Some("color-scheme"),
        glib::clone!(
            #[weak]
            settings,
            move |interface, _| apply_color_scheme(interface, &settings)
        ),
    );
    // Keep the subscription alive for the lifetime of the process.
    std::mem::forget(interface);
}

fn apply_color_scheme(interface: &gio::Settings, settings: &Settings) {
    let dark = interface.string("color-scheme") == "prefer-dark";
    settings.set_property("gtk-application-prefer-dark-theme", dark);
}

fn load_css() {
    let provider = CssProvider::new();
    provider.load_from_data(STYLE);
    if let Some(display) = Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
