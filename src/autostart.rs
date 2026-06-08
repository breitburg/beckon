// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Optional launch-on-login via a freedesktop autostart entry.

use std::env;
use std::fs;
use std::path::PathBuf;

use crate::APP_ID;

fn autostart_path() -> PathBuf {
    let mut path = PathBuf::from(gtk4::glib::user_config_dir());
    path.push("autostart");
    path.push(format!("{APP_ID}.desktop"));
    path
}

/// Create or remove the autostart entry to match `enabled`.
pub fn set_enabled(enabled: bool) {
    let path = autostart_path();
    if enabled {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let exe = env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "elementary-intelligence".to_string());
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Elementary Intelligence\n\
             Exec={exe}\n\
             Icon={APP_ID}\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        );
        if let Err(err) = fs::write(&path, entry) {
            eprintln!("Could not write {}: {err}", path.display());
        }
    } else {
        let _ = fs::remove_file(&path);
    }
}
