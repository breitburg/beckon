// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! The Spotlight-style entry: a compact, borderless prompt that expands into
//! an in-place chat once the first message is sent. Conversation state lives
//! in this window's closures, so dismissing it ends the conversation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::gdk;
use gtk4::gdk_pixbuf;
use gtk4::gio;
use gtk4::glib;
use gtk4::pango::WrapMode;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box as GtkBox, Button, ContentFit, Entry, EventControllerKey, Image, Label,
    Orientation, Picture, PolicyType, Revealer, RevealerTransitionType, ScrolledWindow, Separator,
    Window,
};

use serde_json::{json, Value};

use crate::api::{self, ChatEvent};
use crate::blur;
use crate::config::Config;
use crate::markdown;
use crate::settings_window;
use crate::tools;
use crate::transcript::Transcript;

/// Corner radius of the card, in pixels. Single source of truth: it both
/// clips the compositor blur region (below) and is substituted into the
/// stylesheet's `border-radius` at load time (see `app::load_css`), so the
/// frosted-glass blur and the drawn card always share the same corners.
pub const CORNER_RADIUS: u32 = 8;

/// Fixed width of the card, in pixels. The window never changes width — only
/// its height grows as the chat reveals — so the compositor keeps it centered
/// without horizontal drift.
const WINDOW_WIDTH: i32 = 620;

/// Widest, in pixels, an attached image is shown inline in the chat. The
/// full-resolution data URL still rides along to the model; only the on-screen
/// thumbnail is bounded.
const ATTACHMENT_MAX_WIDTH: f64 = 220.0;

/// Tallest, in pixels, an attached image is shown inline; aspect-preserving, so
/// the height is the binding cap for wide screenshots.
const ATTACHMENT_MAX_HEIGHT: f64 = 100.0;

/// A bounded, left-aligned thumbnail for an inline image attachment from a
/// `data:…;base64,…` URL, or `None` if it isn't a base64 data URL or the bytes
/// don't decode as an image.
///
/// The source is scaled down *at decode time* to fit within `ATTACHMENT_MAX_WIDTH`
/// by `ATTACHMENT_MAX_HEIGHT` (aspect preserved), so the thumbnail's intrinsic
/// size is the bound. The full-resolution data URL still rides along to the model.
///
/// The picture is returned inside a start-aligned horizontal box, and that nesting
/// is load-bearing: a `Picture` preserves aspect, so it answers a height-for-width
/// query with `width / aspect`. The user message is a *vertical* box, which measures
/// each child's height at the column's full width — typically the wider text label's
/// width — so a bare picture would reserve `column_width / aspect` of vertical space
/// and then draw the (narrower) image letterboxed inside it, leaving a tall gap. A
/// *horizontal* wrapper is instead measured at the picture's own width, so the row
/// is exactly the thumbnail's height.
fn attachment_thumbnail(data_url: &str) -> Option<GtkBox> {
    let base64 = data_url.split_once(";base64,").map(|(_, data)| data)?;
    let bytes = glib::Bytes::from_owned(glib::base64_decode(base64));
    let stream = gio::MemoryInputStream::from_bytes(&bytes);
    let pixbuf = gdk_pixbuf::Pixbuf::from_stream_at_scale(
        &stream,
        ATTACHMENT_MAX_WIDTH as i32,
        ATTACHMENT_MAX_HEIGHT as i32,
        true,
        gio::Cancellable::NONE,
    )
    .ok()?;
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let picture = Picture::for_paintable(&gdk::Texture::for_pixbuf(&pixbuf));
    picture.set_content_fit(ContentFit::Contain);
    picture.set_can_shrink(false);
    picture.set_size_request(width, height);
    picture.add_css_class("user-attachment");

    let frame = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .halign(Align::Start)
        .valign(Align::Start)
        .build();
    frame.append(&picture);
    Some(frame)
}

/// A tool call's `arguments` (a JSON-encoded string) pretty-printed for display
/// in its disclosure, falling back to the raw string if it isn't valid JSON.
fn pretty_args(item: &Value) -> String {
    let raw = item["arguments"].as_str().unwrap_or("");
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| raw.to_string())
}

/// Build, wire up and present the entry window. `screenshot` is an OpenAI
/// `image_url` data URL attached to the first message. Returns the window so
/// the caller can track and toggle it.
pub fn present(app: &Application, config: &Rc<RefCell<Config>>, screenshot: Option<String>) -> Window {
    let window = Window::builder()
        .application(app)
        .decorated(false)
        .resizable(false)
        .default_width(WINDOW_WIDTH)
        .build();
    window.set_widget_name("spotlight");
    window.add_css_class("spotlight");

    // The card carries the rounded background and shadow; the surrounding
    // window stays transparent so the corners read as rounded and the shadow
    // has a gutter to render into.
    let card = GtkBox::builder().orientation(Orientation::Vertical).build();
    card.add_css_class("spotlight-card");
    // Fix the card's width so the window never reflows horizontally as replies
    // stream in; long lines wrap instead. The 32px CSS margin sits outside this
    // request, so the toplevel ends up exactly WINDOW_WIDTH wide.
    card.set_size_request(WINDOW_WIDTH - 64, -1);

    let entry_row = GtkBox::builder().orientation(Orientation::Horizontal).build();
    entry_row.add_css_class("entry-row");

    // The search icon is a standalone widget (not the Entry's built-in primary
    // icon) so the attachment chip can sit between it and the text field.
    let search_icon = Image::from_icon_name("edit-find-symbolic");
    search_icon.add_css_class("search-icon");
    search_icon.set_valign(Align::Center);
    entry_row.append(&search_icon);

    let entry = Entry::builder()
        .placeholder_text("Ask anything…")
        .has_frame(false)
        .hexpand(true)
        .build();
    entry.add_css_class("spotlight-entry");
    entry_row.append(&entry);

    // Attachment chip at the trailing edge, only while a screenshot is pending
    // for the first send.
    let chip = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(6)
        .valign(Align::Center)
        .visible(screenshot.is_some())
        .build();
    chip.add_css_class("attachment-chip");
    chip.append(&Image::from_icon_name("image-x-generic-symbolic"));
    chip.append(&Label::new(Some("Screenshot")));
    entry_row.append(&chip);

    // Settings shortcut at the trailing edge of the field.
    let settings_button = Button::from_icon_name("applications-system-symbolic");
    settings_button.add_css_class("flat");
    settings_button.set_valign(Align::Center);
    settings_button.set_tooltip_text(Some("Settings"));
    entry_row.append(&settings_button);

    card.append(&entry_row);

    // The conversation slides open below the entry; the window only ever
    // grows downward, so the pill's top edge stays put.
    let messages = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        // As the scrolled window's child, the viewport stretches it to fill when
        // it is shorter than the visible area; hug the top instead so a single
        // short message keeps its own height rather than filling vertically.
        .valign(Align::Start)
        .build();
    messages.add_css_class("messages");

    let scrolled = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(420)
        .child(&messages)
        .build();

    // A rule separates the field from the conversation; it lives inside the
    // revealer so it slides in with the chat and leaves the collapsed pill clean.
    let chat_area = GtkBox::builder().orientation(Orientation::Vertical).build();
    let separator = Separator::new(Orientation::Horizontal);
    separator.add_css_class("field-separator");
    chat_area.append(&separator);
    chat_area.append(&scrolled);

    let revealer = Revealer::builder()
        .transition_type(RevealerTransitionType::SlideDown)
        .transition_duration(250)
        .reveal_child(false)
        .child(&chat_area)
        .build();
    revealer.add_css_class("chat-revealer");
    card.append(&revealer);

    window.set_child(Some(&card));

    // Conversation state, dropped with the window: fresh chat per invocation.
    let history: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    let streaming = Rc::new(Cell::new(false));
    let screenshot = Rc::new(RefCell::new(screenshot));
    // Handle to the in-flight reply task, so Escape-to-clear can abort it
    // (dropping the channel receiver, which stops the worker thread).
    let active_stream: Rc<RefCell<Option<glib::JoinHandle<()>>>> = Rc::new(RefCell::new(None));
    // The transcript rendering the current turn, shared so Escape-to-clear can
    // tear it down from its own (separate) controller closure.
    let active_transcript: Rc<RefCell<Option<Transcript>>> = Rc::new(RefCell::new(None));
    // The first user message is hidden while the chat is a single exchange; it
    // is revealed once a second turn arrives and the history becomes worth
    // scrolling back through. Holds the whole message row (text plus any
    // attachment thumbnails), so the image hides and reveals with the text.
    let first_user_message: Rc<RefCell<Option<GtkBox>>> = Rc::new(RefCell::new(None));

    // Stick to the bottom while the reply streams in, but let the user scroll
    // up and stay there. The label only resizes after `set_markup` returns, so
    // scrolling happens on the adjustment's own change notifications.
    let stick_to_bottom = Rc::new(Cell::new(true));
    let adjustment = scrolled.vadjustment();
    {
        let stick = stick_to_bottom.clone();
        adjustment.connect_value_changed(move |adj| {
            stick.set(adj.value() + adj.page_size() >= adj.upper() - 1.0);
        });
    }
    {
        let stick = stick_to_bottom.clone();
        adjustment.connect_changed(move |adj| {
            if stick.get() {
                adj.set_value(adj.upper() - adj.page_size());
            }
        });
    }

    {
        let app = app.clone();
        let config = config.clone();
        let window = window.clone();
        settings_button.connect_clicked(move |_| {
            settings_window::present(&app, &config);
            window.close();
        });
    }

    // Enter → append the message and stream the reply into the card.
    {
        let config = config.clone();
        let chip = chip.clone();
        let revealer = revealer.clone();
        let messages = messages.clone();
        let history = history.clone();
        let streaming = streaming.clone();
        let screenshot = screenshot.clone();
        let first_user_message = first_user_message.clone();
        let active_stream = active_stream.clone();
        let active_transcript = active_transcript.clone();
        entry.connect_activate(move |entry| {
            let text = entry.text();
            let message = markdown::clean(&text);
            if message.is_empty() || streaming.get() {
                return;
            }

            // Any attachments pending in the field ride along on this message
            // as Responses API content parts; the field's chip clears once
            // they've been consumed. Currently sourced from a screenshot, but
            // the rendering below treats them generically.
            let attachments: Vec<String> = screenshot.borrow_mut().take().into_iter().collect();
            if !attachments.is_empty() {
                chip.set_visible(false);
            }

            // A plain string for text-only turns; a content-part array once
            // there's at least one image to carry alongside the text.
            let content = if attachments.is_empty() {
                json!(message)
            } else {
                let mut parts = vec![json!({"type": "input_text", "text": message})];
                for data_url in &attachments {
                    parts.push(json!({"type": "input_image", "image_url": data_url}));
                }
                Value::Array(parts)
            };
            let is_first_turn = history.borrow().is_empty();
            history.borrow_mut().push(json!({"role": "user", "content": content}));
            entry.set_text("");
            // Past the first turn, the field invites a follow-up.
            entry.set_placeholder_text(Some("Follow up…"));

            // The user's turn is a column: attachment thumbnails stacked above
            // the text, so an image shows in the chat the same way it was sent.
            // Hug the content vertically (Start) rather than filling the row, so
            // the image keeps its own height instead of stretching to fill.
            let user_message = GtkBox::builder()
                .orientation(Orientation::Vertical)
                .spacing(8)
                .halign(Align::Start)
                .valign(Align::Start)
                .build();
            for data_url in &attachments {
                if let Some(thumbnail) = attachment_thumbnail(data_url) {
                    user_message.append(&thumbnail);
                }
            }
            let user_label = Label::builder()
                .label(&message)
                .halign(Align::Start)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(WrapMode::WordChar)
                .max_width_chars(40)
                .selectable(true)
                .build();
            user_label.add_css_class("user-message");
            user_message.append(&user_label);
            if is_first_turn {
                // A plain opening turn stays hidden until a follow-up arrives, so
                // a single exchange shows just the answer. A turn carrying an
                // attachment is always shown — the user added the image to see it
                // land in the chat.
                if attachments.is_empty() {
                    user_message.set_visible(false);
                    *first_user_message.borrow_mut() = Some(user_message.clone());
                }
            } else {
                // Extra breathing room above each follow-up question, setting it
                // apart from the previous reply.
                user_message.set_margin_top(12);
                if let Some(first) = first_user_message.borrow_mut().take() {
                    // Second turn: the conversation now has history worth showing.
                    first.set_visible(true);
                }
            }
            messages.append(&user_message);

            // First send: reveal the chat below the entry. The blur region
            // from map is kept as-is — Gala forbids a second get_panel on the
            // same surface (fatal protocol error) — and its inset-based region
            // already follows the growing window.
            if !revealer.reveals_child() {
                revealer.set_reveal_child(true);
            }

            // Reasoning, tool calls and answer text render into this turn's
            // transcript in arrival order. Shared so Escape can tear it down.
            let transcript = Transcript::new(messages.clone(), &user_message);
            transcript.set_busy(true);
            *active_transcript.borrow_mut() = Some(transcript.clone());

            streaming.set(true);
            let (api_config, system_prompt) = {
                let config = config.borrow();
                (
                    api::ApiConfig {
                        base_url: config.api_base_url.clone(),
                        api_key: config.api_key.clone(),
                        model: config.model.clone(),
                    },
                    config.system_prompt.trim().to_string(),
                )
            };
            // Prepend the configured system prompt to the turn, if any.
            let mut payload = Vec::new();
            if !system_prompt.is_empty() {
                payload.push(json!({"role": "system", "content": system_prompt}));
            }
            payload.extend(history.borrow().iter().cloned());
            // Hand the model only the tools the user enabled; an empty registry
            // makes the request omit `tools` and behave exactly as before.
            let registry = tools::registry_for(&config.borrow().enabled_tools);
            let (sender, receiver) = async_channel::unbounded::<ChatEvent>();
            api::stream_chat(api_config, payload, registry, sender);

            let history = history.clone();
            let streaming = streaming.clone();
            let handle = glib::spawn_future_local(async move {
                // The visible answer is accumulated here only to persist it as a
                // single assistant message once the turn ends; rendering (and the
                // tool-call/output items) is the transcript's job.
                let mut accumulated = String::new();
                let mut errored = false;
                while let Ok(event) = receiver.recv().await {
                    match event {
                        ChatEvent::Reasoning(delta) => transcript.push_reasoning(&delta),
                        ChatEvent::Delta(delta) => {
                            accumulated.push_str(&delta);
                            transcript.push_answer(&delta);
                        }
                        ChatEvent::ToolCall { name, item } => {
                            // Persist the call so follow-up turns include it, and
                            // render it inline with its arguments.
                            let args = pretty_args(&item);
                            history.borrow_mut().push(item);
                            transcript.push_tool_call(&name, &args);
                        }
                        ChatEvent::ToolResult { item } => {
                            // Persist the output right after its matching call and
                            // fill it into the call's disclosure.
                            let output =
                                item["output"].as_str().unwrap_or_default().to_string();
                            history.borrow_mut().push(item);
                            transcript.push_tool_result(&output);
                        }
                        ChatEvent::Done => break,
                        ChatEvent::Error(message) => {
                            errored = true;
                            transcript.show_error(&message);
                            break;
                        }
                    }
                }
                transcript.finish();
                if !errored && !accumulated.is_empty() {
                    history
                        .borrow_mut()
                        .push(json!({"role": "assistant", "content": accumulated}));
                }
                streaming.set(false);
            });
            *active_stream.borrow_mut() = Some(handle);
        });
    }

    // Esc → clear the conversation if there is one, otherwise dismiss. The first
    // press resets to an empty prompt; a second (now-empty) press closes.
    let key_controller = EventControllerKey::new();
    {
        let window = window.clone();
        let history = history.clone();
        let revealer = revealer.clone();
        let entry_for_key = entry.clone();
        let first_user_message = first_user_message.clone();
        let streaming = streaming.clone();
        let active_stream = active_stream.clone();
        let active_transcript = active_transcript.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            let has_chat = revealer.reveals_child() || !history.borrow().is_empty();
            if !has_chat {
                window.close();
                return glib::Propagation::Stop;
            }
            // Clear: abort any in-flight reply, drop history and message widgets,
            // collapse the chat, and restore the initial prompt.
            if let Some(handle) = active_stream.borrow_mut().take() {
                handle.abort();
            }
            streaming.set(false);
            history.borrow_mut().clear();
            first_user_message.borrow_mut().take();
            // Tearing the transcript down stops its fade ticker and removes every
            // rendered block (the user rows and assistant content alike).
            if let Some(transcript) = active_transcript.borrow_mut().take() {
                transcript.clear();
            }
            revealer.set_reveal_child(false);
            entry_for_key.set_text("");
            entry_for_key.set_placeholder_text(Some("Ask anything…"));
            glib::Propagation::Stop
        });
    }
    window.add_controller(key_controller);

    // Clicking away (losing focus) → dismiss. The compositor's keybinding
    // handling can bounce focus off the freshly mapped window, so the close
    // stays disarmed during a grace period. If the bounce won, focus is taken
    // back — with the grace renewed, since that present bounces in turn and
    // an armed close would read it as the user clicking away.
    let grace = Rc::new(Cell::new(true));
    {
        let grace = grace.clone();
        let window = window.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            if window.is_visible() && !window.is_active() {
                eprintln!("spotlight: focus lost during grace, presenting again");
                window.present();
                let grace = grace.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    grace.set(false);
                });
            } else {
                grace.set(false);
            }
        });
    }
    window.connect_is_active_notify(move |window| {
        if !window.is_active() && !grace.get() {
            eprintln!("spotlight: dismissed on focus loss");
            window.close();
        }
    });

    // Ask the compositor to blur the desktop behind the card (Pantheon shell).
    window.connect_map(|window| blur::apply(window, CORNER_RADIUS));

    window.present();
    entry.grab_focus();
    window
}
