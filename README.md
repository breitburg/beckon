# Elementary Intelligence

Summon any AI chat service from a system-wide shortcut.

Elementary Intelligence runs quietly in the background. Press your shortcut
(default <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd>) to bring up a simple
entry, type a message, and press <kbd>Enter</kbd> — it opens your chosen
assistant with the message ready to send.

Built natively for elementary OS in Rust and GTK4, inheriting the system
stylesheet so it feels at home.

## Features

- **System-wide hotkey** — configurable trigger combination.
- **Spotlight-style entry** — a minimal, centered prompt.
- **Pluggable services** — Claude (default), ChatGPT, Gemini and Mistral, each
  defined by a `{q}` URL template you can edit in the config.
- **Start on login** — runs as a background service.

## How the hotkey works

Wayland has no in-process global key grab, and Pantheon ships no GlobalShortcuts
portal. Instead the app registers a *custom keybinding* with the compositor via
`org.gnome.settings-daemon.plugins.media-keys` (which elementary's
settings-daemon honours). When you press the combo, Gala runs the app with
`--spotlight`, and the already-running single instance shows the entry.

## Build & run

```sh
cargo run            # builds and launches the background service + settings
```

Or install system-wide with meson:

```sh
meson setup build
ninja -C build
sudo ninja -C build install
```

## Configuration

Settings live in
`~/.config/com.github.breitburg.elementary-intelligence/config.toml` and are
also editable from the app's settings window. Add or tweak services by editing
the `[[services]]` entries — the `{q}` placeholder is replaced with the
URL-encoded message:

```toml
[[services]]
name = "Claude"
url_template = "https://claude.ai/new?q={q}"
```

## Flatpak

`flatpak/com.github.breitburg.elementary-intelligence.yml` is a starting
manifest. Note it needs dconf permissions to register the host keybinding, and
vendored cargo sources for an offline build (see comments in the manifest).

## License

GPL-3.0-or-later
