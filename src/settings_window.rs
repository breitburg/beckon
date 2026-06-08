// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! The persistent settings window: pick the service, the trigger shortcut and
//! whether to launch on login.
//!
//! Laid out as a native elementary settings form — right-aligned labels in the
//! left column, controls in the right column of a single grid.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, DropDown, EventControllerKey,
    Grid, HeaderBar, Label, MenuButton, Orientation, Switch, Widget,
};

use crate::autostart;
use crate::config::Config;
use crate::keybinding;

/// Show the settings window, creating it if it does not exist yet.
pub fn present(app: &Application, config: &Rc<RefCell<Config>>) {
    // Reuse an existing settings window if one is already open.
    if let Some(window) = app
        .windows()
        .into_iter()
        .find(|w| w.widget_name() == "settings")
    {
        window.present();
        return;
    }

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Elementary Intelligence")
        .resizable(false)
        .default_width(380)
        .build();
    window.set_widget_name("settings");

    // Flat, backgroundless header that blends into the window: window controls
    // and the menu only, no title text.
    let header = HeaderBar::new();
    header.add_css_class("flat");
    header.set_title_widget(Some(&Label::new(None)));
    let menu_model = gio::Menu::new();
    menu_model.append(Some("Quit Elementary Intelligence"), Some("app.quit"));
    let menu_button = MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .build();
    header.pack_end(&menu_button);
    window.set_titlebar(Some(&header));

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .build();

    // --- Heading: bold title + a purple enable toggle ----------------------
    let heading = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .build();
    // Full-width so the gradient spans the window; vertical padding lives in CSS
    // so the gradient fills the whole section.
    heading.add_css_class("app-heading");

    let title = Label::new(Some("Elementary Intelligence"));
    title.add_css_class("app-title");
    title.set_halign(Align::Center);
    heading.append(&title);

    let enable_switch = Switch::builder()
        .active(config.borrow().enabled)
        .halign(Align::Center)
        .build();
    enable_switch.add_css_class("brand");
    heading.append(&enable_switch);
    content.append(&heading);

    let grid = Grid::builder()
        .row_spacing(12)
        .column_spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();

    // --- Service -----------------------------------------------------------
    let (service_names, selected_index): (Vec<String>, Option<usize>) = {
        let config = config.borrow();
        let names = config.services.iter().map(|s| s.name.clone()).collect();
        let index = config
            .services
            .iter()
            .position(|s| s.name == config.selected_service);
        (names, index)
    };
    let name_refs: Vec<&str> = service_names.iter().map(String::as_str).collect();
    let service_dropdown = DropDown::from_strings(&name_refs);
    service_dropdown.set_hexpand(true);
    if let Some(index) = selected_index {
        service_dropdown.set_selected(index as u32);
    }
    add_row(&grid, 0, "Service", &service_dropdown);

    {
        let config = config.clone();
        service_dropdown.connect_selected_notify(move |dropdown| {
            let index = dropdown.selected() as usize;
            let mut config = config.borrow_mut();
            if let Some(service) = config.services.get(index) {
                config.selected_service = service.name.clone();
                config.save();
            }
        });
    }

    // --- Shortcut ----------------------------------------------------------
    let shortcut_button = Button::with_label(&accel_to_label(&config.borrow().shortcut));
    shortcut_button.set_hexpand(true);
    shortcut_button.set_tooltip_text(Some("Click, then press the new combination"));
    add_row(&grid, 1, "Shortcut", &shortcut_button);

    let capturing = Rc::new(Cell::new(false));
    {
        let capturing = capturing.clone();
        let shortcut_button_inner = shortcut_button.clone();
        shortcut_button.connect_clicked(move |button| {
            capturing.set(true);
            button.set_label("Press keys…");
            shortcut_button_inner.add_css_class("suggested-action");
        });
    }

    let key_controller = EventControllerKey::new();
    {
        let capturing = capturing.clone();
        let config = config.clone();
        let shortcut_button = shortcut_button.clone();
        key_controller.connect_key_pressed(move |_, key, _, state| {
            if !capturing.get() {
                return glib::Propagation::Proceed;
            }
            let reset = |button: &Button, label: &str| {
                capturing.set(false);
                button.remove_css_class("suggested-action");
                button.set_label(label);
            };
            if key == gdk::Key::Escape {
                reset(&shortcut_button, &accel_to_label(&config.borrow().shortcut));
                return glib::Propagation::Stop;
            }
            if is_modifier_key(key) {
                return glib::Propagation::Stop; // wait for the real key
            }
            let mods = state & gtk4::accelerator_get_default_mod_mask();
            if !gtk4::accelerator_valid(key, mods) {
                return glib::Propagation::Stop;
            }
            let accel = gtk4::accelerator_name(key, mods).to_string();
            reset(&shortcut_button, &accel_to_label(&accel));

            let mut config = config.borrow_mut();
            config.shortcut = accel.clone();
            config.save();
            keybinding::apply(&accel, config.enabled);
            glib::Propagation::Stop
        });
    }
    window.add_controller(key_controller);

    // --- Start on login ----------------------------------------------------
    let login_switch = Switch::builder()
        .active(config.borrow().start_on_login)
        .halign(Align::Start)
        .valign(Align::Center)
        .build();
    add_row(&grid, 2, "Start on login", &login_switch);

    {
        let config = config.clone();
        login_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            let mut config = config.borrow_mut();
            config.start_on_login = enabled;
            config.save();
            autostart::set_enabled(enabled);
        });
    }

    // The form follows the enable state.
    grid.set_sensitive(config.borrow().enabled);
    {
        let config = config.clone();
        let grid = grid.clone();
        enable_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            let mut config = config.borrow_mut();
            config.enabled = enabled;
            config.save();
            keybinding::apply(&config.shortcut, enabled);
            grid.set_sensitive(enabled);
        });
    }

    content.append(&grid);
    window.set_child(Some(&content));
    window.present();
}

/// Attach a labelled control as one form row: right-aligned label in column 0,
/// control in column 1.
fn add_row(grid: &Grid, row: i32, label: &str, control: &impl IsA<Widget>) {
    let label = Label::builder()
        .label(label)
        .halign(Align::End)
        .valign(Align::Center)
        .build();
    grid.attach(&label, 0, row, 1, 1);
    grid.attach(control, 1, row, 1, 1);
}

/// Render a stored GTK accelerator (e.g. `<Control><Shift>space`) as a
/// human-readable label (e.g. `Ctrl+Shift+Space`).
fn accel_to_label(accelerator: &str) -> String {
    match gtk4::accelerator_parse(accelerator) {
        Some((key, mods)) if key != gdk::Key::VoidSymbol => {
            gtk4::accelerator_get_label(key, mods).to_string()
        }
        _ => accelerator.to_string(),
    }
}

fn is_modifier_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
    )
}
