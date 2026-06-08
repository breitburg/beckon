// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::APP_ID;

/// A single AI service the user can send messages to.
///
/// `url_template` contains a `{q}` placeholder that is replaced with the
/// percent-encoded message before the link is opened.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub url_template: String,
}

impl Service {
    fn new(name: &str, url_template: &str) -> Self {
        Self {
            name: name.to_string(),
            url_template: url_template.to_string(),
        }
    }

    /// Build a launchable URL for `message` (already percent-encoded).
    pub fn url_for(&self, encoded_message: &str) -> String {
        self.url_template.replace("{q}", encoded_message)
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Whether the global shortcut is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Name of the currently selected [`Service`].
    pub selected_service: String,
    /// GTK accelerator string, e.g. `<Control><Shift>space`.
    pub shortcut: String,
    /// Whether to launch the background service on login.
    pub start_on_login: bool,
    /// The configurable list of services.
    pub services: Vec<Service>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            selected_service: "Claude".to_string(),
            shortcut: "<Control><Shift>space".to_string(),
            start_on_login: true,
            services: vec![
                Service::new("Claude", "https://claude.ai/new?q={q}"),
                Service::new("ChatGPT", "https://chatgpt.com/?q={q}"),
                Service::new("Gemini", "https://gemini.google.com/app?q={q}"),
                Service::new("Mistral", "https://chat.mistral.ai/chat?q={q}"),
            ],
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        let mut path = PathBuf::from(gtk4::glib::user_config_dir());
        path.push(APP_ID);
        path.push("config.toml");
        path
    }

    /// Load the config from disk, falling back to defaults on any error. On
    /// first run the defaults are written out so the file is there to edit.
    pub fn load() -> Self {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_else(|err| {
                eprintln!("Could not parse {}: {err}; using defaults", path.display());
                Config::default()
            }),
            Err(_) => {
                let config = Config::default();
                config.save();
                config
            }
        }
    }

    /// Persist the config to disk, creating the directory if needed.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                eprintln!("Could not create {}: {err}", parent.display());
                return;
            }
        }
        match toml::to_string_pretty(self) {
            Ok(contents) => {
                if let Err(err) = fs::write(&path, contents) {
                    eprintln!("Could not write {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("Could not serialize config: {err}"),
        }
    }

    /// The currently selected service, falling back to the first one.
    pub fn current_service(&self) -> Option<&Service> {
        self.services
            .iter()
            .find(|s| s.name == self.selected_service)
            .or_else(|| self.services.first())
    }
}
