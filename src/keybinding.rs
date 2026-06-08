// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! System-wide hotkey registration.
//!
//! Wayland has no in-process global key grab, and Pantheon ships no
//! GlobalShortcuts portal. The reliable path is a *custom keybinding* in
//! `org.gnome.settings-daemon.plugins.media-keys`, which elementary's
//! settings-daemon honours: the compositor runs our command when the combo is
//! pressed, and that command re-invokes us with `--spotlight`.

use std::env;

use gtk4::gio;
use gtk4::prelude::*;

const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_KEYBINDING_SCHEMA: &str =
    "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
/// Dedicated, app-specific relay path so we never collide with the user's own
/// custom shortcuts.
const RELAY_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/elementary-intelligence/";

/// Absolute command the compositor runs when the hotkey fires.
fn spotlight_command() -> String {
    let exe = env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "elementary-intelligence".to_string());
    format!("{exe} --spotlight")
}

/// Register (or update) the system-wide shortcut. When `enabled` is false the
/// binding is cleared so the entry stays but does nothing — toggling it back on
/// simply restores the accelerator.
pub fn apply(accelerator: &str, enabled: bool) {
    let binding = if enabled { accelerator } else { "" };
    let relay = gio::Settings::with_path(CUSTOM_KEYBINDING_SCHEMA, RELAY_PATH);
    let _ = relay.set_string("name", "Elementary Intelligence");
    let _ = relay.set_string("command", &spotlight_command());
    let _ = relay.set_string("binding", binding);

    // Make sure our relay path is listed in the media-keys custom bindings.
    let media_keys = gio::Settings::new(MEDIA_KEYS_SCHEMA);
    let mut paths: Vec<String> = media_keys
        .strv("custom-keybindings")
        .iter()
        .map(|s| s.to_string())
        .collect();
    if !paths.iter().any(|p| p == RELAY_PATH) {
        paths.push(RELAY_PATH.to_string());
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let _ = media_keys.set_strv("custom-keybindings", refs);
    }
    gio::Settings::sync();
}
