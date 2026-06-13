// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Rendering of a streaming assistant turn into the conversation list.
//!
//! A turn is not a single answer: a tool-using model interleaves reasoning,
//! tool calls and answer text across several round-trips. So rendering is
//! *arrival-driven* — [`Transcript`] appends a block the moment a new kind of
//! event arrives and extends the trailing block while events of the same kind
//! keep coming. Reasoning traces and tool calls share one [`Disclosure`] widget
//! so they read identically; the answer streams into a faded markdown label.
//!
//! This module is view-only: conversation history (the JSON resent to the model)
//! is owned by the caller. A [`Transcript`] holds shared (`Rc`) handles, so it is
//! cheap to clone into the stream task and the Escape-to-clear handler.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::glib;
use gtk4::pango::WrapMode;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Image, Label, Orientation, Revealer, RevealerTransitionType,
    Spinner, Widget,
};

use crate::markdown::{self, clean};

/// Duration over which a freshly arrived chunk fades from faint to opaque.
const FADE: Duration = Duration::from_millis(250);

/// Streaming render state for one answer. Text that has finished fading is kept
/// in `settled` and rendered with full markdown; chunks still within the fade
/// window trail it as plain, alpha-ramped spans so new text fades in as it
/// arrives.
#[derive(Default)]
struct FadeState {
    settled: String,
    /// Cached `markdown::to_pango(&settled)`, recomputed only when `settled`
    /// grows — the fade ticker renders every frame and must not re-parse the
    /// whole body each time.
    settled_markup: String,
    pending: Vec<(String, Instant)>,
}

impl FadeState {
    fn push(&mut self, chunk: String, now: Instant) {
        self.pending.push((chunk, now));
    }

    /// Move chunks whose fade has completed into the settled body. Arrivals are
    /// monotonic, so expired chunks are always at the front.
    fn settle(&mut self, now: Instant) {
        let mut grew = false;
        while self
            .pending
            .first()
            .is_some_and(|(_, arrival)| now.duration_since(*arrival) >= FADE)
        {
            let (text, _) = self.pending.remove(0);
            self.settled.push_str(&text);
            grew = true;
        }
        if grew {
            self.settled_markup = markdown::to_pango(&self.settled);
        }
    }

    /// Markdown for the settled body, followed by each still-fading chunk in a
    /// span whose alpha reflects how far through the fade it is.
    fn to_markup(&self, now: Instant) -> String {
        let mut markup = self.settled_markup.clone();
        for (text, arrival) in &self.pending {
            let progress = now.duration_since(*arrival).as_secs_f64() / FADE.as_secs_f64();
            let percent = (progress.clamp(0.0, 1.0) * 100.0).max(1.0) as u32;
            markup.push_str(&format!(
                "<span alpha=\"{percent}%\">{}</span>",
                glib::markup_escape_text(text)
            ));
        }
        markup
    }

    /// Everything received so far, settled plus still-fading, in order.
    fn full_text(&self) -> String {
        let mut text = self.settled.clone();
        for (chunk, _) in &self.pending {
            text.push_str(chunk);
        }
        text
    }

    fn is_empty(&self) -> bool {
        self.settled.is_empty() && self.pending.is_empty()
    }
}

/// A flat toggle header (chevron + title, with a spinner for the "running"
/// state) over a revealer holding a markdown body. Used for both reasoning
/// traces and tool calls so they look identical in the transcript.
#[derive(Clone)]
pub struct Disclosure {
    container: GtkBox,
    chevron: Image,
    spinner: Spinner,
    title: Label,
    revealer: Revealer,
    body: Label,
}

impl Disclosure {
    fn new() -> Self {
        let body = Label::builder()
            .halign(Align::Fill)
            .hexpand(true)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(WrapMode::WordChar)
            .selectable(true)
            .use_markup(true)
            .build();
        body.add_css_class("disclosure-body");

        let revealer = Revealer::builder()
            .transition_type(RevealerTransitionType::SlideDown)
            .transition_duration(200)
            .reveal_child(true)
            .child(&body)
            .build();

        // The spinner stands in for the chevron while a tool runs; one or the
        // other is visible, never both.
        let spinner = Spinner::new();
        spinner.set_visible(false);
        let chevron = Image::from_icon_name("pan-down-symbolic");
        let title = Label::new(None);
        let header = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .build();
        header.append(&spinner);
        header.append(&chevron);
        header.append(&title);

        let toggle = Button::builder().child(&header).build();
        toggle.add_css_class("flat");
        toggle.add_css_class("disclosure-toggle");
        toggle.set_halign(Align::Start);

        let container = GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(2)
            .build();
        container.add_css_class("disclosure");
        container.append(&toggle);
        container.append(&revealer);

        // The header toggles the body open or closed; the chevron tracks it.
        {
            let revealer = revealer.clone();
            let chevron = chevron.clone();
            toggle.connect_clicked(move |_| {
                let open = !revealer.reveals_child();
                revealer.set_reveal_child(open);
                chevron.set_icon_name(Some(chevron_icon(open)));
            });
        }

        Disclosure {
            container,
            chevron,
            spinner,
            title,
            revealer,
            body,
        }
    }

    fn widget(&self) -> &GtkBox {
        &self.container
    }

    fn set_title(&self, text: &str) {
        self.title.set_label(text);
    }

    fn set_body_markup(&self, markup: &str) {
        self.body.set_markup(markup);
    }

    fn set_open(&self, open: bool) {
        self.revealer.set_reveal_child(open);
        self.chevron.set_icon_name(Some(chevron_icon(open)));
    }

    fn set_running(&self, running: bool) {
        self.spinner.set_visible(running);
        self.chevron.set_visible(!running);
        if running {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
    }
}

fn chevron_icon(open: bool) -> &'static str {
    if open {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    }
}

/// One streaming answer: a markdown label fed by a [`FadeState`] and a frame
/// ticker that ramps each chunk's alpha. The block owns its own `finished` flag
/// so a later block (or a clear) can stop this ticker without touching others.
#[derive(Clone)]
struct AnswerBlock {
    label: Label,
    fade: Rc<RefCell<FadeState>>,
    ticking: Rc<Cell<bool>>,
    finished: Rc<Cell<bool>>,
}

impl AnswerBlock {
    fn new() -> Self {
        let label = Label::builder()
            .halign(Align::Fill)
            .hexpand(true)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(WrapMode::WordChar)
            .selectable(true)
            .use_markup(true)
            .build();
        label.add_css_class("assistant-message");
        AnswerBlock {
            label,
            fade: Rc::new(RefCell::new(FadeState::default())),
            ticking: Rc::new(Cell::new(false)),
            finished: Rc::new(Cell::new(false)),
        }
    }

    fn widget(&self) -> &Label {
        &self.label
    }

    fn render(&self) {
        self.label.set_markup(&self.fade.borrow().to_markup(Instant::now()));
    }

    fn push(&self, delta: &str) {
        self.fade.borrow_mut().push(delta.to_string(), Instant::now());
        self.render();
        self.start_ticker();
    }

    /// While chunks are fading, re-render on a frame timer so their alpha ramps
    /// even when the stream pauses; the timer stops itself once everything has
    /// settled, the stream finishes, or the block is superseded.
    fn start_ticker(&self) {
        if self.ticking.get() {
            return;
        }
        self.ticking.set(true);
        let fade = self.fade.clone();
        let ticking = self.ticking.clone();
        let finished = self.finished.clone();
        let label = self.label.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            if finished.get() {
                ticking.set(false);
                return glib::ControlFlow::Break;
            }
            fade.borrow_mut().settle(Instant::now());
            label.set_markup(&fade.borrow().to_markup(Instant::now()));
            if fade.borrow().pending.is_empty() {
                ticking.set(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn has_content(&self) -> bool {
        !self.fade.borrow().is_empty()
    }

    /// Stop fading and snap to the final, fully opaque markdown render.
    fn snap(&self) {
        self.finished.set(true);
        let text = self.fade.borrow().full_text();
        self.label.set_markup(&markdown::to_pango(&clean(&text)));
    }

    /// Keep the partial answer and trail an italic error line; the `.error`
    /// tint would colour the whole body, so it is left off here.
    fn append_error(&self, message: &str) {
        self.finished.set(true);
        let text = self.fade.borrow().full_text();
        let error_line = format!("<i>{}</i>", glib::markup_escape_text(message));
        self.label.set_markup(&format!(
            "{}\n\n{error_line}",
            markdown::to_pango(&clean(&text))
        ));
    }

    fn stop(&self) {
        self.finished.set(true);
    }
}

/// A reasoning trace, with the timer used to label it once collapsed.
struct ReasoningBlock {
    disc: Disclosure,
    text: String,
    started: Instant,
    collapsed: bool,
}

/// What kind of block is currently trailing, so a delta can decide whether to
/// extend it or open a new one. Kept separate from the live widget handles
/// (the `Option` fields below) — the routing rules only need the kind.
#[derive(Clone, Copy, PartialEq)]
enum Trailing {
    None,
    Reasoning,
    Answer,
    Tool,
}

struct Inner {
    messages: GtkBox,
    /// A spinner shown as the last child while waiting for the model to produce
    /// the next block; detached whenever a block is the live activity.
    spinner_row: GtkBox,
    spinner: Spinner,
    /// The last content block appended, used as the anchor for inserting the
    /// next one *before* the trailing spinner.
    last_block: RefCell<Option<Widget>>,
    trailing: Cell<Trailing>,
    reasoning: RefCell<Option<ReasoningBlock>>,
    answer: RefCell<Option<AnswerBlock>>,
    /// The tool whose result is still pending, plus its name and rendered
    /// arguments, held so the matching result can fill in the body.
    pending_tool: RefCell<Option<(Disclosure, String, String)>>,
}

/// Renders one assistant turn into a shared `messages` box. Cheap to clone.
#[derive(Clone)]
pub struct Transcript {
    inner: Rc<Inner>,
}

impl Transcript {
    /// `messages` is the conversation's vertical box; `anchor` is the just-
    /// appended user-message row, so the first block lands right after it.
    pub fn new(messages: GtkBox, anchor: &impl IsA<Widget>) -> Self {
        let spinner_row = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .halign(Align::Start)
            .build();
        let spinner = Spinner::new();
        spinner.add_css_class("reply-spinner");
        spinner_row.append(&spinner);

        Transcript {
            inner: Rc::new(Inner {
                messages,
                spinner_row,
                spinner,
                last_block: RefCell::new(Some(anchor.clone().upcast())),
                trailing: Cell::new(Trailing::None),
                reasoning: RefCell::new(None),
                answer: RefCell::new(None),
                pending_tool: RefCell::new(None),
            }),
        }
    }

    /// Show or hide the trailing activity spinner. While busy the spinner sits
    /// after the last block; when a block becomes the live activity (reasoning,
    /// answer or a running tool) it is hidden.
    pub fn set_busy(&self, busy: bool) {
        let row = &self.inner.spinner_row;
        if busy {
            if row.parent().is_none() {
                let last = self.inner.last_block.borrow().clone();
                self.inner.messages.insert_child_after(row, last.as_ref());
            }
            self.inner.spinner.start();
        } else if row.parent().is_some() {
            self.inner.spinner.stop();
            self.inner.messages.remove(row);
        }
    }

    pub fn push_reasoning(&self, delta: &str) {
        if self.inner.trailing.get() != Trailing::Reasoning {
            self.settle_trailing();
            let disc = Disclosure::new();
            disc.set_title("Thinking…");
            disc.set_open(true);
            self.append_block(disc.widget());
            *self.inner.reasoning.borrow_mut() = Some(ReasoningBlock {
                disc,
                text: String::new(),
                started: Instant::now(),
                collapsed: false,
            });
        }
        if let Some(block) = self.inner.reasoning.borrow_mut().as_mut() {
            block.text.push_str(delta);
            block.disc.set_body_markup(&markdown::to_pango(&clean(&block.text)));
        }
        self.inner.trailing.set(Trailing::Reasoning);
        self.set_busy(false);
    }

    pub fn push_answer(&self, delta: &str) {
        if self.inner.trailing.get() != Trailing::Answer {
            self.settle_trailing();
            let block = AnswerBlock::new();
            self.append_block(block.widget());
            *self.inner.answer.borrow_mut() = Some(block);
        }
        if let Some(block) = self.inner.answer.borrow().as_ref() {
            block.push(delta);
        }
        self.inner.trailing.set(Trailing::Answer);
        self.set_busy(false);
    }

    pub fn push_tool_call(&self, name: &str, args_display: &str) {
        self.settle_trailing();
        let disc = Disclosure::new();
        disc.set_title(&format!("Running {name}…"));
        disc.set_running(true);
        disc.set_open(false);
        self.append_block(disc.widget());
        *self.inner.pending_tool.borrow_mut() =
            Some((disc, name.to_string(), args_display.to_string()));
        self.inner.trailing.set(Trailing::Tool);
        self.set_busy(false);
    }

    pub fn push_tool_result(&self, output: &str) {
        if let Some((disc, name, args)) = self.inner.pending_tool.borrow_mut().take() {
            disc.set_running(false);
            disc.set_title(&format!("Ran {name}"));
            disc.set_body_markup(&tool_body_markup(&args, output));
            disc.set_open(false);
        }
        self.inner.trailing.set(Trailing::Tool);
        // The tool is done; wait for the model's next block.
        self.set_busy(true);
    }

    pub fn show_error(&self, message: &str) {
        let partial = self.inner.trailing.get() == Trailing::Answer
            && self
                .inner
                .answer
                .borrow()
                .as_ref()
                .is_some_and(AnswerBlock::has_content);
        if partial {
            if let Some(block) = self.inner.answer.borrow().as_ref() {
                block.append_error(message);
            }
        } else {
            let label = Label::builder()
                .halign(Align::Fill)
                .hexpand(true)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(WrapMode::WordChar)
                .selectable(true)
                .use_markup(true)
                .build();
            label.add_css_class("assistant-message");
            label.add_css_class("error");
            label.set_markup(&format!("<i>{}</i>", glib::markup_escape_text(message)));
            self.append_block(&label);
        }
        self.set_busy(false);
    }

    /// End the turn: snap the answer to opaque markdown, relabel a trace the
    /// model left open (only-reasoning, no answer), and drop the spinner.
    pub fn finish(&self) {
        if let Some(block) = self.inner.answer.borrow().as_ref() {
            if block.has_content() {
                block.snap();
            }
        }
        if let Some(block) = self.inner.reasoning.borrow_mut().as_mut() {
            if !block.collapsed {
                block.disc.set_title("Reasoning");
            }
        }
        self.set_busy(false);
    }

    /// Tear the turn down: stop the ticker, remove every block, and reset state.
    /// Used by Escape-to-clear, which wipes the whole conversation.
    pub fn clear(&self) {
        if let Some(block) = self.inner.answer.borrow().as_ref() {
            block.stop();
        }
        while let Some(child) = self.inner.messages.first_child() {
            self.inner.messages.remove(&child);
        }
        *self.inner.last_block.borrow_mut() = None;
        *self.inner.reasoning.borrow_mut() = None;
        *self.inner.answer.borrow_mut() = None;
        *self.inner.pending_tool.borrow_mut() = None;
        self.inner.trailing.set(Trailing::None);
    }

    /// Insert a block after the last one and keep the spinner trailing it.
    fn append_block<W: IsA<Widget> + Clone>(&self, widget: &W) {
        let last = self.inner.last_block.borrow().clone();
        self.inner.messages.insert_child_after(widget, last.as_ref());
        *self.inner.last_block.borrow_mut() = Some(widget.clone().upcast());
        if self.inner.spinner_row.parent().is_some() {
            self.inner
                .messages
                .reorder_child_after(&self.inner.spinner_row, Some(widget));
        }
    }

    /// Finalise whatever block is trailing before a new kind starts: collapse an
    /// open reasoning trace (labelling how long it ran) and snap a streaming
    /// answer to its opaque render so it stops fading once superseded.
    fn settle_trailing(&self) {
        match self.inner.trailing.get() {
            Trailing::Reasoning => {
                if let Some(block) = self.inner.reasoning.borrow_mut().as_mut() {
                    if !block.collapsed {
                        block.collapsed = true;
                        block.disc.set_open(false);
                        let secs = block.started.elapsed().as_secs();
                        block.disc.set_title(&if secs == 0 {
                            "Thought for a moment".to_string()
                        } else {
                            format!("Thought for {secs}s")
                        });
                    }
                }
            }
            Trailing::Answer => {
                if let Some(block) = self.inner.answer.borrow().as_ref() {
                    if block.has_content() {
                        block.snap();
                    }
                }
            }
            Trailing::None | Trailing::Tool => {}
        }
    }
}

/// Render a tool call's arguments and output as a monospace body. Empty or
/// trivial (`{}`) arguments are omitted so the disclosure isn't cluttered.
fn tool_body_markup(args: &str, output: &str) -> String {
    let mut out = String::new();
    let args = args.trim();
    if !args.is_empty() && args != "{}" {
        out.push_str(&mono(args));
    }
    let output = output.trim();
    if !output.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&mono(output));
    }
    out
}

/// Escape `text` and wrap it in a monospace span (newlines kept).
fn mono(text: &str) -> String {
    format!("<tt>{}</tt>", glib::markup_escape_text(text))
}
