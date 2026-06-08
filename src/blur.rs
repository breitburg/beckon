// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Backdrop blur via Gala's `pantheon-desktop-shell` Wayland protocol — the
//! same mechanism the elementary panel and dock use.
//!
//! GTK4 has no `backdrop-filter`, and Pantheon ships no portal for this, so we
//! piggy-back on GTK's existing Wayland connection: bind the shell global,
//! wrap our window's `wl_surface`, and ask the compositor to blur an inset,
//! rounded region matching the card (the transparent gutter + corner radius).

use std::cell::RefCell;

use gtk4::glib::translate::ToGlibPtr;
use gtk4::prelude::*;
use gtk4::Window;

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

// Generated client bindings for the Pantheon shell protocol.
mod pantheon {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/pantheon-desktop-shell-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocol/pantheon-desktop-shell-v1.xml");
}

use pantheon::io_elementary_pantheon_panel_v1::IoElementaryPantheonPanelV1;
use pantheon::io_elementary_pantheon_shell_v1::IoElementaryPantheonShellV1;

/// Inset of the card from the window edge — matches the CSS `margin`.
const INSET: u32 = 32;
/// Corner radius of the card. The card is a pill, so this is half its height
/// (entry min-height 40 + 2×4 padding ≈ 48).
const CLIP_RADIUS: u32 = 24;

extern "C" {
    fn gdk_wayland_surface_get_wl_surface(
        surface: *mut gtk4::gdk::ffi::GdkSurface,
    ) -> *mut std::ffi::c_void;
    fn gdk_wayland_display_get_wl_display(
        display: *mut gtk4::gdk::ffi::GdkDisplay,
    ) -> *mut std::ffi::c_void;
}

struct State {
    shell: Option<IoElementaryPantheonShellV1>,
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "io_elementary_pantheon_shell_v1" {
                state.shell =
                    Some(registry.bind::<IoElementaryPantheonShellV1, _, _>(name, 1, qh, ()));
            }
        }
    }
}

// Neither object emits events; the handlers are never reached.
impl Dispatch<IoElementaryPantheonShellV1, ()> for State {
    fn event(_: &mut Self, _: &IoElementaryPantheonShellV1, _: pantheon::io_elementary_pantheon_shell_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<IoElementaryPantheonPanelV1, ()> for State {
    fn event(_: &mut Self, _: &IoElementaryPantheonPanelV1, _: pantheon::io_elementary_pantheon_panel_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// A connection to the compositor and the bound shell, set up once and reused.
struct Context {
    conn: Connection,
    _queue: EventQueue<State>,
    qh: QueueHandle<State>,
    shell: IoElementaryPantheonShellV1,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// Blur the desktop behind the card of `window`. No-op if the protocol is
/// unavailable (e.g. running under a different compositor or X11).
pub fn apply(window: &Window) {
    let Some(surface) = window.surface() else {
        return;
    };
    let display = WidgetExt::display(window);

    let surface_stash: *mut gtk4::gdk::ffi::GdkSurface = surface.to_glib_none().0;
    let display_stash: *mut gtk4::gdk::ffi::GdkDisplay = display.to_glib_none().0;
    let (wl_display, wl_surface_ptr) = unsafe {
        (
            gdk_wayland_display_get_wl_display(display_stash),
            gdk_wayland_surface_get_wl_surface(surface_stash),
        )
    };
    if wl_display.is_null() || wl_surface_ptr.is_null() {
        return;
    }

    CONTEXT.with(|cell| {
        let mut context = cell.borrow_mut();
        if context.is_none() {
            *context = setup(wl_display);
        }
        let Some(context) = context.as_ref() else {
            eprintln!("Pantheon shell blur protocol unavailable");
            return;
        };

        // Wrap GTK's existing wl_surface as a proxy without taking ownership.
        let id = match unsafe { ObjectId::from_ptr(WlSurface::interface(), wl_surface_ptr.cast()) } {
            Ok(id) => id,
            Err(err) => {
                eprintln!("blur: invalid surface id: {err}");
                return;
            }
        };
        let Ok(wl_surface) = WlSurface::from_id(&context.conn, id) else {
            return;
        };

        let panel = context.shell.get_panel(&wl_surface, &context.qh, ());
        panel.add_blur(INSET, INSET, INSET, INSET, CLIP_RADIUS);
        let _ = context.conn.flush();
    });
}

/// Bind the shell global on GTK's Wayland display. Returns `None` if the
/// compositor doesn't advertise it.
fn setup(wl_display: *mut std::ffi::c_void) -> Option<Context> {
    // Foreign display: the backend does not own or disconnect GTK's connection.
    let backend = unsafe { Backend::from_foreign_display(wl_display.cast()) };
    let conn = Connection::from_backend(backend);
    let mut queue: EventQueue<State> = conn.new_event_queue();
    let qh = queue.handle();

    let _registry = conn.display().get_registry(&qh, ());
    let mut state = State { shell: None };
    queue.roundtrip(&mut state).ok()?;

    let shell = state.shell?;
    Some(Context {
        conn,
        _queue: queue,
        qh,
        shell,
    })
}
