// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! The persistent settings window: the API endpoint, key and model, the two
//! trigger shortcuts and whether to launch on login.
//!
//! Laid out as a native elementary settings form — right-aligned labels in the
//! left column, controls in the right column of a single grid.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CheckButton, DropDown, Entry,
    EventControllerKey, Frame, Grid, HeaderBar, Image, Label, ListBox, ListBoxRow, MenuButton,
    Orientation, PasswordEntry, ScrolledWindow, SelectionMode, Stack, StringList, StringObject,
    Switch, TextView, Widget, Window,
};

use crate::api;
use crate::autostart;
use crate::config::Config;
use crate::keybinding;
use crate::mcp::{McpManager, McpServerConfig, McpStatus, McpTransport, ServerState};
use crate::tools;

/// Which shortcut a capture button is currently recording for.
#[derive(Clone, Copy, PartialEq)]
enum ShortcutTarget {
    Open,
    Screenshot,
}

/// Show the settings window, creating it if it does not exist yet.
pub fn present(app: &Application, config: &Rc<RefCell<Config>>, mcp: &Arc<McpManager>) {
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
        .title("Beckon")
        .default_width(640)
        .default_height(560)
        .build();
    window.set_widget_name("settings");

    // Native elementary header (the brushed gradient titlebar) carrying the
    // title and the menu.
    let header = HeaderBar::new();
    let menu_model = gio::Menu::new();
    menu_model.append(Some("Quit Beckon"), Some("app.quit"));
    let menu_button = MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .build();
    header.pack_end(&menu_button);
    window.set_titlebar(Some(&header));

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .build();

    // Each settings tab is its own grid; a StackSidebar below switches between
    // them. `page()` builds one with the shared spacing/margins. The "Assistant"
    // tab is the assistant itself (model, prompt, endpoint), "General" is the
    // system integration (on/off, shortcuts, autostart), and "Capabilities" is
    // the toolsets + MCP servers the agent can call.
    let general = page();
    let behavior = page();
    let connectors = page();

    // --- API endpoint, key and model ----------------------------------------
    let url_entry = Entry::builder()
        .text(&config.borrow().api_base_url)
        .placeholder_text("https://api.openai.com/v1")
        .hexpand(true)
        .build();
    add_row(&behavior, 0, "API URL", &url_entry);

    let key_entry = PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .build();
    key_entry.set_text(&config.borrow().api_key);
    add_row(&behavior, 1, "API Key", &key_entry);

    // The model picker lists whatever the endpoint's /models reports. Until a
    // fetch succeeds (or when it fails) it holds just the configured model.
    let model_list = StringList::new(&[]);
    if !config.borrow().model.is_empty() {
        model_list.append(&config.borrow().model);
    }
    let model_dropdown = DropDown::builder()
        .model(&model_list)
        .enable_search(true)
        .hexpand(true)
        .build();
    // Search only filters if the dropdown knows how to turn each item into a
    // string to match against — point it at the StringObject's `string`.
    model_dropdown.set_expression(Some(gtk4::PropertyExpression::new(
        StringObject::static_type(),
        gtk4::Expression::NONE,
        "string",
    )));
    add_row(&behavior, 2, "Model", &model_dropdown);

    // Repopulating the list fires selection notifications; ignore them.
    let repopulating = Rc::new(Cell::new(false));
    {
        let config = config.clone();
        let repopulating = repopulating.clone();
        model_dropdown.connect_selected_item_notify(move |dropdown| {
            if repopulating.get() {
                return;
            }
            let Some(item) = dropdown.selected_item().and_downcast::<StringObject>() else {
                return;
            };
            let mut config = config.borrow_mut();
            config.model = item.string().to_string();
            config.save();
        });
    }

    let refresh_models = {
        let config = config.clone();
        let model_list = model_list.clone();
        let model_dropdown = model_dropdown.clone();
        let repopulating = repopulating.clone();
        Rc::new(move || {
            let (base_url, api_key, current) = {
                let config = config.borrow();
                (config.api_base_url.clone(), config.api_key.clone(), config.model.clone())
            };
            if base_url.is_empty() {
                return;
            }
            let (sender, receiver) = async_channel::bounded::<Result<Vec<String>, String>>(1);
            api::list_models(
                api::ApiConfig { base_url, api_key, model: String::new() },
                sender,
            );
            let model_list = model_list.clone();
            let model_dropdown = model_dropdown.clone();
            let repopulating = repopulating.clone();
            glib::spawn_future_local(async move {
                let Ok(Ok(mut models)) = receiver.recv().await else {
                    return; // fetch failed: keep whatever the list holds
                };
                if models.is_empty() {
                    return;
                }
                // The configured model stays available even if unlisted.
                if !current.is_empty() && !models.contains(&current) {
                    models.insert(0, current.clone());
                }
                repopulating.set(true);
                model_list.splice(
                    0,
                    model_list.n_items(),
                    &models.iter().map(String::as_str).collect::<Vec<_>>(),
                );
                if let Some(index) = models.iter().position(|m| *m == current) {
                    model_dropdown.set_selected(index as u32);
                }
                repopulating.set(false);
            });
        })
    };
    refresh_models();

    // Saving on every keystroke is fine for a TOML write, but refetching the
    // model list is debounced until typing pauses.
    let refetch_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let schedule_refresh = {
        let refresh_models = refresh_models.clone();
        let refetch_timer = refetch_timer.clone();
        Rc::new(move || {
            if let Some(source) = refetch_timer.borrow_mut().take() {
                source.remove();
            }
            let refresh_models = refresh_models.clone();
            let timer = refetch_timer.clone();
            let source = glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                timer.borrow_mut().take();
                refresh_models();
            });
            refetch_timer.borrow_mut().replace(source);
        })
    };

    {
        let config = config.clone();
        let schedule_refresh = schedule_refresh.clone();
        url_entry.connect_changed(move |entry| {
            {
                let mut config = config.borrow_mut();
                config.api_base_url = entry.text().trim().to_string();
                config.save();
            }
            schedule_refresh();
        });
    }
    {
        let config = config.clone();
        let schedule_refresh = schedule_refresh.clone();
        key_entry.connect_changed(move |entry| {
            {
                let mut config = config.borrow_mut();
                config.api_key = entry.text().trim().to_string();
                config.save();
            }
            schedule_refresh();
        });
    }

    // --- System prompt ------------------------------------------------------
    let system_view = gtk4::TextView::builder()
        .wrap_mode(gtk4::WrapMode::WordChar)
        .accepts_tab(false)
        .top_margin(6)
        .bottom_margin(6)
        .left_margin(6)
        .right_margin(6)
        .build();
    system_view.buffer().set_text(&config.borrow().system_prompt);
    let system_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .min_content_height(72)
        .max_content_height(160)
        .has_frame(true)
        .hexpand(true)
        .child(&system_view)
        .build();
    system_scroll.add_css_class("system-prompt");
    add_top_row(&behavior, 3, "System prompt", &system_scroll);

    {
        let config = config.clone();
        system_view.buffer().connect_changed(move |buffer| {
            let (start, end) = buffer.bounds();
            let text = buffer.text(&start, &end, false);
            let mut config = config.borrow_mut();
            config.system_prompt = text.to_string();
            config.save();
        });
    }

    // --- Master enable toggle -----------------------------------------------
    let enable_switch = Switch::builder()
        .active(config.borrow().enabled)
        .halign(Align::Start)
        .valign(Align::Center)
        .build();
    enable_switch.add_css_class("brand");
    add_row(&general, 0, "Enabled", &enable_switch);

    // --- Shortcuts -----------------------------------------------------------
    let shortcut_button = Button::with_label(&accel_to_label(&config.borrow().shortcut));
    shortcut_button.set_hexpand(true);
    shortcut_button.set_tooltip_text(Some("Click, then press the new combination"));
    add_row(&general, 1, "Shortcut", &shortcut_button);

    let screenshot_button =
        Button::with_label(&accel_to_label(&config.borrow().screenshot_shortcut));
    screenshot_button.set_hexpand(true);
    screenshot_button.set_tooltip_text(Some(
        "Opens the prompt with a screenshot of your screen attached",
    ));
    add_row(&general, 2, "Screenshot shortcut", &screenshot_button);

    // One shared capture state: only one button records at a time, and the
    // single window-level key controller writes to whichever field is armed.
    let capturing: Rc<Cell<Option<ShortcutTarget>>> = Rc::new(Cell::new(None));
    arm_capture(&shortcut_button, &screenshot_button, &capturing, ShortcutTarget::Open, config);
    arm_capture(&screenshot_button, &shortcut_button, &capturing, ShortcutTarget::Screenshot, config);

    let key_controller = EventControllerKey::new();
    {
        let capturing = capturing.clone();
        let config = config.clone();
        let shortcut_button = shortcut_button.clone();
        let screenshot_button = screenshot_button.clone();
        key_controller.connect_key_pressed(move |_, key, _, state| {
            let Some(target) = capturing.get() else {
                return glib::Propagation::Proceed;
            };
            let button = match target {
                ShortcutTarget::Open => &shortcut_button,
                ShortcutTarget::Screenshot => &screenshot_button,
            };
            let current = |config: &Config| match target {
                ShortcutTarget::Open => config.shortcut.clone(),
                ShortcutTarget::Screenshot => config.screenshot_shortcut.clone(),
            };
            let reset = |button: &Button, label: &str| {
                capturing.set(None);
                button.remove_css_class("suggested-action");
                button.set_label(label);
            };
            if key == gdk::Key::Escape {
                reset(button, &accel_to_label(&current(&config.borrow())));
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
            reset(button, &accel_to_label(&accel));

            let mut config = config.borrow_mut();
            match target {
                ShortcutTarget::Open => config.shortcut = accel,
                ShortcutTarget::Screenshot => config.screenshot_shortcut = accel,
            }
            config.save();
            keybinding::apply(&config);
            glib::Propagation::Stop
        });
    }
    window.add_controller(key_controller);

    // --- Toolsets -----------------------------------------------------------
    // One checkbox per available toolset, gathered in a framed box; each row
    // pairs the toolset's built-in icon with its label, and ticking adds its
    // name to the enabled list the spotlight passes to the model.
    let tools_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    for info in tools::catalog() {
        let row = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .build();
        row.append(&Image::from_icon_name(info.icon));
        row.append(&Label::new(Some(info.label)));

        let check = CheckButton::builder()
            .active(config.borrow().enabled_toolsets.iter().any(|t| t == info.name))
            .tooltip_text(info.description)
            .build();
        check.set_child(Some(&row));
        {
            let config = config.clone();
            let name = info.name;
            check.connect_toggled(move |check| {
                let mut config = config.borrow_mut();
                config.enabled_toolsets.retain(|t| t != name);
                if check.is_active() {
                    config.enabled_toolsets.push(name.to_string());
                }
                config.save();
            });
        }
        tools_box.append(&check);
    }
    let tools_frame = Frame::new(None);
    tools_frame.set_child(Some(&tools_box));
    add_top_row(&connectors, 0, "Toolsets", &tools_frame);

    // --- MCP servers -------------------------------------------------------
    // A framed list of configured MCP servers — one row each with an enable
    // toggle, live connection status, and edit/remove — plus an "Add" button.
    // The rows are rebuilt from config + the manager's snapshot whenever the
    // set changes or a connection's status moves.
    let servers_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    let server_list = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .build();
    servers_box.append(&server_list);

    // Sits below the framed list, not inside it.
    let add_button = Button::builder()
        .label("Add server…")
        .halign(Align::Start)
        .build();
    add_button.add_css_class("flat");

    // Late-bound so row/button handlers can trigger a rebuild of the list they
    // live in (each rebuild recreates those handlers).
    let rebuild: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let builder: Rc<dyn Fn()> = {
        let server_list = server_list.clone();
        let config = config.clone();
        let mcp = mcp.clone();
        let window = window.clone();
        let rebuild = rebuild.clone();
        Rc::new(move || {
            while let Some(child) = server_list.first_child() {
                server_list.remove(&child);
            }
            let snapshot = mcp.snapshot();
            let servers = config.borrow().mcp_servers.clone();
            if servers.is_empty() {
                let empty = Label::builder()
                    .label("No servers configured.")
                    .halign(Align::Start)
                    .build();
                empty.add_css_class("dim-label");
                server_list.append(&empty);
            }
            for (index, server) in servers.into_iter().enumerate() {
                server_list.append(&server_row(
                    &server, index, &snapshot, &config, &mcp, &window, &rebuild,
                ));
            }
        })
    };
    *rebuild.borrow_mut() = Some(builder.clone());
    builder();

    {
        let config = config.clone();
        let mcp = mcp.clone();
        let window = window.clone();
        let rebuild = rebuild.clone();
        add_button.connect_clicked(move |_| {
            open_server_dialog(&window, &config, &mcp, None, &rebuild);
        });
    }

    // While a connection is settling, refresh the rows so the status moves from
    // "Connecting…" to the tool count (or an error) without manual interaction.
    {
        let mcp = mcp.clone();
        let window_weak = window.downgrade();
        let builder = builder.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(800), move || {
            let Some(_window) = window_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if mcp
                .snapshot()
                .iter()
                .any(|s| matches!(s.status, McpStatus::Connecting))
            {
                builder();
            }
            glib::ControlFlow::Continue
        });
    }

    let servers_frame = Frame::new(None);
    servers_frame.set_child(Some(&servers_box));

    // The framed list with the "Add server…" button stacked underneath it.
    let servers_section = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .build();
    servers_section.append(&servers_frame);
    servers_section.append(&add_button);
    add_top_row(&connectors, 1, "MCP servers", &servers_section);

    // --- Start on login ----------------------------------------------------
    let login_switch = Switch::builder()
        .active(config.borrow().start_on_login)
        .halign(Align::Start)
        .valign(Align::Center)
        .build();
    add_row(&general, 3, "Start on login", &login_switch);

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

    // --- Tabbed layout: an icon sidebar on the left switching the stack -----
    let stack = Stack::builder().vexpand(true).hexpand(true).build();
    stack.add_named(&scroll_page(&general), Some("general"));
    stack.add_named(&scroll_page(&behavior), Some("assistant"));
    stack.add_named(&scroll_page(&connectors), Some("capabilities"));

    // Custom nav (GtkStackSidebar can't show icons): one row per page, each a
    // monochrome symbolic icon beside its title. The row's widget-name is the
    // stack child it selects.
    let sidebar = ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    sidebar.set_selection_mode(SelectionMode::Browse);
    sidebar.set_width_request(180);
    for (name, title, icon) in [
        ("general", "General", "applications-system-symbolic"),
        ("assistant", "Assistant", "preferences-other-symbolic"),
        ("capabilities", "Capabilities", "application-x-addon-symbolic"),
    ] {
        let row_box = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(10)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .margin_end(10)
            .build();
        row_box.append(&Image::from_icon_name(icon));
        row_box.append(&Label::new(Some(title)));
        let row = ListBoxRow::new();
        row.set_child(Some(&row_box));
        row.set_widget_name(name);
        sidebar.append(&row);
    }
    {
        let stack = stack.clone();
        sidebar.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                stack.set_visible_child_name(&row.widget_name());
            }
        });
    }
    sidebar.select_row(sidebar.row_at_index(0).as_ref());

    let split = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .vexpand(true)
        .build();
    split.append(&sidebar);
    split.append(&gtk4::Separator::new(Orientation::Vertical));
    split.append(&stack);

    // The rest of the form follows the enable state, but the toggle itself
    // (and the rest of the General page) stays live so it can be flipped back
    // on. Gate the other two pages and General's own controls.
    let set_form_enabled = {
        let behavior = behavior.clone();
        let connectors = connectors.clone();
        let shortcut_button = shortcut_button.clone();
        let screenshot_button = screenshot_button.clone();
        let login_switch = login_switch.clone();
        Rc::new(move |enabled: bool| {
            behavior.set_sensitive(enabled);
            connectors.set_sensitive(enabled);
            shortcut_button.set_sensitive(enabled);
            screenshot_button.set_sensitive(enabled);
            login_switch.set_sensitive(enabled);
        })
    };
    set_form_enabled(config.borrow().enabled);
    {
        let config = config.clone();
        let set_form_enabled = set_form_enabled.clone();
        enable_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            let mut config = config.borrow_mut();
            config.enabled = enabled;
            config.save();
            keybinding::apply(&config);
            set_form_enabled(enabled);
        });
    }

    content.append(&split);
    window.set_child(Some(&content));
    window.present();
}

/// One settings tab: a grid with the shared form spacing and margins.
fn page() -> Grid {
    Grid::builder()
        .row_spacing(12)
        .column_spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build()
}

/// Wrap a page grid in a scroller so a tall page (e.g. many MCP servers)
/// scrolls instead of forcing the window taller.
fn scroll_page(grid: &Grid) -> ScrolledWindow {
    ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .child(grid)
        .build()
}

/// Re-run the late-bound list builder, if one is set. Used by the server-row
/// handlers (and the status poll) to redraw the list after a change.
fn rebuild(rebuild: &Rc<RefCell<Option<Rc<dyn Fn()>>>>) {
    let builder = rebuild.borrow().clone();
    if let Some(builder) = builder {
        builder();
    }
}

/// Build one server row: enable toggle, name, connection status, edit + remove.
fn server_row(
    server: &McpServerConfig,
    index: usize,
    snapshot: &[ServerState],
    config: &Rc<RefCell<Config>>,
    mcp: &Arc<McpManager>,
    window: &ApplicationWindow,
    rebuilder: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) -> GtkBox {
    let row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();

    let enable = CheckButton::builder()
        .active(server.enabled)
        .valign(Align::Center)
        .build();
    {
        let config = config.clone();
        let mcp = mcp.clone();
        let rebuilder = rebuilder.clone();
        enable.connect_toggled(move |check| {
            {
                let mut config = config.borrow_mut();
                if let Some(server) = config.mcp_servers.get_mut(index) {
                    server.enabled = check.is_active();
                }
                config.save();
                mcp.reload(&config.mcp_servers);
            }
            rebuild(&rebuilder);
        });
    }
    row.append(&enable);

    let name = Label::builder()
        .label(&server.name)
        .halign(Align::Start)
        .build();
    row.append(&name);

    let (status, tooltip) = status_text(server, snapshot);
    let status_label = Label::builder()
        .label(&status)
        .halign(Align::Start)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .build();
    status_label.add_css_class("dim-label");
    status_label.set_tooltip_text(tooltip.as_deref());
    row.append(&status_label);

    let edit = Button::from_icon_name("document-edit-symbolic");
    edit.add_css_class("flat");
    edit.set_tooltip_text(Some("Edit"));
    {
        let config = config.clone();
        let mcp = mcp.clone();
        let window = window.clone();
        let rebuilder = rebuilder.clone();
        edit.connect_clicked(move |_| {
            open_server_dialog(&window, &config, &mcp, Some(index), &rebuilder);
        });
    }
    row.append(&edit);

    let remove = Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove"));
    {
        let config = config.clone();
        let mcp = mcp.clone();
        let rebuilder = rebuilder.clone();
        remove.connect_clicked(move |_| {
            {
                let mut config = config.borrow_mut();
                if index < config.mcp_servers.len() {
                    config.mcp_servers.remove(index);
                }
                config.save();
                mcp.reload(&config.mcp_servers);
            }
            rebuild(&rebuilder);
        });
    }
    row.append(&remove);

    row
}

/// The status text (and optional tooltip with the full error) for a server row,
/// derived from the manager's snapshot. Disabled servers show no live status.
fn status_text(server: &McpServerConfig, snapshot: &[ServerState]) -> (String, Option<String>) {
    if !server.enabled {
        return ("Disabled".to_string(), None);
    }
    match snapshot.iter().find(|s| s.name == server.name) {
        Some(state) => match &state.status {
            McpStatus::Connecting => ("Connecting…".to_string(), None),
            McpStatus::Ready => {
                let count = state.tools.len();
                let plural = if count == 1 { "" } else { "s" };
                (format!("{count} tool{plural}"), None)
            }
            McpStatus::Error(message) => ("Error".to_string(), Some(message.clone())),
        },
        None => ("Connecting…".to_string(), None),
    }
}

/// Open the modal add/edit dialog. `existing` is the index being edited, or
/// `None` to add a new server. On save it writes the config, reconnects, and
/// triggers a list rebuild.
fn open_server_dialog(
    parent: &ApplicationWindow,
    config: &Rc<RefCell<Config>>,
    mcp: &Arc<McpManager>,
    existing: Option<usize>,
    rebuilder: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let current = existing.and_then(|i| config.borrow().mcp_servers.get(i).cloned());

    let dialog = Window::builder()
        .title(if existing.is_some() {
            "Edit MCP Server"
        } else {
            "Add MCP Server"
        })
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(360)
        .build();

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    let name_entry = Entry::builder().placeholder_text("My server").build();
    if let Some(server) = &current {
        name_entry.set_text(&server.name);
    }
    content.append(&labeled("Name", &name_entry));

    let transport_model = StringList::new(&["Local (stdio)", "Remote (HTTP)"]);
    let transport_drop = DropDown::builder().model(&transport_model).build();
    let is_http = matches!(
        current.as_ref().map(|s| &s.transport),
        Some(McpTransport::Http)
    );
    transport_drop.set_selected(if is_http { 1 } else { 0 });
    content.append(&labeled("Transport", &transport_drop));

    // stdio fields.
    let stdio_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .build();
    let command_entry = Entry::builder().placeholder_text("npx").build();
    let args_entry = Entry::builder()
        .placeholder_text("-y @modelcontextprotocol/server-everything")
        .build();
    let env_view = TextView::new();
    env_view.set_monospace(true);
    let env_scroll = ScrolledWindow::builder()
        .min_content_height(70)
        .has_frame(true)
        .child(&env_view)
        .build();
    if let Some(server) = &current {
        command_entry.set_text(&server.command);
        args_entry.set_text(&server.args.join(" "));
        env_view.buffer().set_text(&format_env(&server.env));
    }
    stdio_box.append(&labeled("Command", &command_entry));
    stdio_box.append(&labeled("Arguments (space-separated)", &args_entry));
    stdio_box.append(&labeled("Environment (KEY=VALUE per line)", &env_scroll));
    content.append(&stdio_box);

    // http fields.
    let http_box = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .build();
    let url_entry = Entry::builder()
        .placeholder_text("https://example.com/mcp")
        .build();
    let token_entry = PasswordEntry::builder().show_peek_icon(true).build();
    if let Some(server) = &current {
        url_entry.set_text(&server.url);
        token_entry.set_text(&server.auth_token);
    }
    http_box.append(&labeled("URL", &url_entry));
    http_box.append(&labeled("Auth token (optional)", &token_entry));
    content.append(&http_box);

    update_transport_fields(&transport_drop, &stdio_box, &http_box);
    {
        let stdio_box = stdio_box.clone();
        let http_box = http_box.clone();
        transport_drop.connect_selected_notify(move |drop| {
            update_transport_fields(drop, &stdio_box, &http_box);
        });
    }

    let buttons = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let cancel = Button::with_label("Cancel");
    let save = Button::with_label("Save");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    content.append(&buttons);

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog = dialog.clone();
        let config = config.clone();
        let mcp = mcp.clone();
        let rebuilder = rebuilder.clone();
        save.connect_clicked(move |_| {
            let name = name_entry.text().trim().to_string();
            let http = transport_drop.selected() == 1;
            let command = command_entry.text().trim().to_string();
            let url = url_entry.text().trim().to_string();

            // Validate; flag the offending field rather than silently doing
            // nothing, then bail so the user can correct it.
            let duplicate = config
                .borrow()
                .mcp_servers
                .iter()
                .enumerate()
                .any(|(i, s)| s.name == name && Some(i) != existing);
            let mut ok = true;
            for (entry, bad) in [
                (&name_entry, name.is_empty() || duplicate),
                (&url_entry, http && url.is_empty()),
                (&command_entry, !http && command.is_empty()),
            ] {
                if bad {
                    entry.add_css_class("error");
                    ok = false;
                } else {
                    entry.remove_css_class("error");
                }
            }
            if !ok {
                return;
            }

            let buffer = env_view.buffer();
            let env_text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            let server = McpServerConfig {
                name,
                enabled: current.as_ref().map(|s| s.enabled).unwrap_or(true),
                transport: if http {
                    McpTransport::Http
                } else {
                    McpTransport::Stdio
                },
                command,
                args: args_entry
                    .text()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
                env: parse_env(&env_text),
                url,
                auth_token: token_entry.text().to_string(),
            };

            {
                let mut config = config.borrow_mut();
                match existing {
                    Some(i) if i < config.mcp_servers.len() => config.mcp_servers[i] = server,
                    _ => config.mcp_servers.push(server),
                }
                config.save();
                mcp.reload(&config.mcp_servers);
            }
            rebuild(&rebuilder);
            dialog.close();
        });
    }

    dialog.set_child(Some(&content));
    dialog.present();
}

/// Show only the fields relevant to the selected transport.
fn update_transport_fields(drop: &DropDown, stdio_box: &GtkBox, http_box: &GtkBox) {
    let http = drop.selected() == 1;
    stdio_box.set_visible(!http);
    http_box.set_visible(http);
}

/// A control with a small dim caption above it, as one dialog field.
fn labeled(caption: &str, control: &impl IsA<Widget>) -> GtkBox {
    let field = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .build();
    let label = Label::builder().label(caption).halign(Align::Start).build();
    label.add_css_class("dim-label");
    field.append(&label);
    field.append(control);
    field
}

/// Render env vars as `KEY=VALUE` lines for the dialog's text area.
fn format_env(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse `KEY=VALUE` lines back into an env map, skipping blank/invalid lines.
fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Wire a shortcut button to arm capture for `target`, restoring the other
/// button's label if it was mid-capture.
fn arm_capture(
    button: &Button,
    other: &Button,
    capturing: &Rc<Cell<Option<ShortcutTarget>>>,
    target: ShortcutTarget,
    config: &Rc<RefCell<Config>>,
) {
    let capturing = capturing.clone();
    let other = other.clone();
    let config = config.clone();
    button.connect_clicked(move |button| {
        if let Some(previous) = capturing.get() {
            if previous != target {
                let config = config.borrow();
                let accel = match previous {
                    ShortcutTarget::Open => &config.shortcut,
                    ShortcutTarget::Screenshot => &config.screenshot_shortcut,
                };
                other.remove_css_class("suggested-action");
                other.set_label(&accel_to_label(accel));
            }
        }
        capturing.set(Some(target));
        button.set_label("Press keys…");
        button.add_css_class("suggested-action");
    });
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

/// Like `add_row`, but pins the label to the top of a tall control (e.g. the
/// multi-line system-prompt box) instead of centring it.
fn add_top_row(grid: &Grid, row: i32, label: &str, control: &impl IsA<Widget>) {
    let label = Label::builder()
        .label(label)
        .halign(Align::End)
        .valign(Align::Start)
        .margin_top(6)
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
