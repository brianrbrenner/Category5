//! # Input Subsystem
//!
//! This subsystem recieves less attention from the others, but is every
//! bit as important. It reads events from libinput and updates our
//! compositor state in reaction. It is heavily tied to `ways` through the
//! atmosphere, and is the least "standalone" of all the subsystems.
//!
//! `input` runs in the same thread as `ways` for performance
//! reasons. Originally the design was for input to have its own thread,
//! and all three threads performed message passing to update each
//! other. This performed abysmally, both in regards to power efficiency
//! and runtime efficiency. On high refresh rate mice the amount of
//! messages generated by `input` was enormous and made the system
//! unusable. Additionally, being in the same thread means that we can use
//! kqueue to block on both the libinput fd and the libwayland fd, and
//! dispatch to the proper code when one of them is woken up.
//!
//! The `input` code is also the ugliest of all the subsystems, and is
//! essentially a giant state machine that accepts libinput events and
//! generates atmosphere and wayland events. It uses xkbcommon to handle
//! keymaps and get the current keyboard state.
//!
//! The main dispatch method is `handle_input_event`, and it is the
//! recommended starting place for new readers.

// Austin Shafer - 2020

// Note that when including this file you need to use
// ::input::*, because the line below imports an
// external input crate.
#![allow(dead_code)]
pub mod codes;
pub mod event;

extern crate input;
extern crate nix;
extern crate wayland_server as ws;
extern crate xkbcommon;

use ws::protocol::wl_pointer;
use ws::Main;

use crate::category5::atmosphere::Atmosphere;
use crate::category5::ways::{role::Role, xdg_shell::xdg_toplevel::ResizeEdge};
use event::*;
use utils::{log, timing::*, WindowId};

use input::event::keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait};
use input::event::pointer;
use input::event::pointer::{ButtonState, PointerEvent};
use input::event::Event;
use input::{Libinput, LibinputInterface};

use xkbcommon::xkb;
pub use xkbcommon::xkb::{keysyms, Keysym};

use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::RawFd;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::path::Path;

use std::sync::{Arc, Mutex};

use std::mem::drop;

/// This is sort of like a private userdata struct which
/// is used as an interface to the systems devices
///
/// i.e. this could call consolekit to avoid having to
/// be a root user to get raw input.
struct Inkit {
    // For now we don't have anything special to do,
    // so we are just putting a phantom int here since
    // we need to have something.
    _inner: u32,
}

/// This is the interface that libinput uses to abstract away
/// consolekit and friends.
///
/// In our case we just pass the arguments through to `open`.
/// We need to use the unix open extensions so that we can pass
/// custom flags.
impl LibinputInterface for Inkit {
    // open a device
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<RawFd, i32> {
        log::debug!(" Opening device {:?}", path);
        match OpenOptions::new()
            // the unix extension's custom_flag field below
            // masks out O_ACCMODE, i.e. read/write, so add
            // them back in
            .read(true)
            .write(true)
            // libinput wants to use O_NONBLOCK
            .custom_flags(flags)
            .open(path)
        {
            Ok(f) => {
                // this turns the File into an int, so we
                // don't need to worry about the File's
                // lifetime.
                let fd = f.into_raw_fd();
                log::error!("Returning raw fd {}", fd);
                Ok(fd)
            }
            Err(e) => {
                // leave this in, it gives great error msgs
                log::error!("Error on opening {:?}", e);
                Err(-1)
            }
        }
    }

    // close a device
    fn close_restricted(&mut self, fd: RawFd) {
        unsafe {
            // this will close the file
            drop(File::from_raw_fd(fd));
        }
    }
}

/// This represents an input system
///
/// Input is grabbed from the udev interface, but
/// any method should be applicable. It just feeds
/// the ways and wm subsystems input events
///
/// We will also stash our xkb resources here, and
/// will consult this before sending out keymaps/syms
pub struct Input {
    /// libinput context
    libin: Libinput,
    /// xkb goodies
    i_xkb_ctx: xkb::Context,
    i_xkb_keymap: xkb::Keymap,
    /// this is referenced by Seat, which needs to map and
    /// share it with the clients
    pub i_xkb_keymap_name: String,
    /// xkb state machine
    i_xkb_state: xkb::State,

    /// Tracking info for the modifier keys
    /// These keys are sent separately in the modifiers event
    pub i_mod_ctrl: bool,
    pub i_mod_alt: bool,
    pub i_mod_shift: bool,
    pub i_mod_caps: bool,
    pub i_mod_meta: bool,
    pub i_mod_num: bool,

    /// Resize tracking
    /// When we resize a window we want to batch together the
    /// changes and send one configure message per frame
    /// The window currently being resized
    /// The currently grabbed resizing window is in the atmosphere
    /// changes to the window surface to be sent this frame
    pub i_resize_diff: (f64, f64),
    /// The surface that the pointer is currently over
    /// note that this may be different than the application focus
    pub i_pointer_focus: Option<WindowId>,
}

impl Input {
    /// Create an input subsystem.
    ///
    /// Setup the libinput library from a udev context
    pub fn new() -> Input {
        let kit: Inkit = Inkit { _inner: 0 };
        let mut libin = Libinput::new_with_udev(kit);

        // we need to choose a "seat" for udev to listen on
        // the default seat is seat0, which is all input devs
        libin.udev_assign_seat("seat0").unwrap();

        // Create all the components for xkb
        // A description of this can be found in the xkb
        // section of wayland-book.com
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            &"",
            &"",
            &"",
            &"", // These should be env vars
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("Could not initialize a xkb keymap");
        let km_name = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);

        let state = xkb::State::new(&keymap);

        Input {
            libin: libin,
            i_xkb_ctx: context,
            i_xkb_keymap: keymap,
            i_xkb_keymap_name: km_name,
            i_xkb_state: state,
            i_mod_ctrl: false,
            i_mod_alt: false,
            i_mod_shift: false,
            i_mod_caps: false,
            i_mod_meta: false,
            i_mod_num: false,
            i_resize_diff: (0.0, 0.0),
            i_pointer_focus: None,
        }
    }

    /// Get a pollable fd
    ///
    /// This saves power and is monitored by kqueue in
    /// the ways event loop
    pub fn get_poll_fd(&mut self) -> RawFd {
        self.libin.as_raw_fd()
    }

    /// Processs any pending input events
    ///
    /// dispatch will grab the latest available data
    /// from the devices and perform libinputs internal
    /// (time sensitive) operations on them
    /// It will then handle all the available input events
    /// before returning.
    pub fn dispatch(&mut self) {
        self.libin.dispatch().unwrap();

        // now go through each event
        while let Some(iev) = self.next_available() {
            self.handle_input_event(&iev);
        }
    }

    fn get_scroll_event(&self, ev: &dyn pointer::PointerScrollEvent) -> Axis {
        let mut ret = Axis {
            a_hori_val: 0.0,
            a_vert_val: 0.0,
            a_v120_val: None,
            a_source: 0,
        };
        if ev.has_axis(pointer::Axis::Horizontal) {
            ret.a_hori_val = ev.scroll_value(pointer::Axis::Horizontal);
        }
        if ev.has_axis(pointer::Axis::Vertical) {
            ret.a_vert_val = ev.scroll_value(pointer::Axis::Vertical);
        }

        log::debug!("scrolling by {:?}", ret);
        return ret;
    }

    /// Get the next available event from libinput
    ///
    /// Dispatch should be called before this so libinput can
    /// internally read and prepare all events.
    fn next_available(&mut self) -> Option<InputEvent> {
        // TODO: need to fix this wrapper
        let ev = self.libin.next();
        match ev {
            Some(Event::Pointer(PointerEvent::Motion(m))) => {
                log::debug!("moving mouse by ({}, {})", m.dx(), m.dy());

                return Some(InputEvent::pointer_move(PointerMove {
                    pm_dx: m.dx(),
                    pm_dy: m.dy(),
                }));
            }
            // TODO: actually handle advanced scrolling/finger behavior
            // We should track ScrollWheel using the v120 api, and handle
            // high-res and wheel click behavior. For ScrollFinger we
            // should handle kinetic scrolling
            Some(Event::Pointer(PointerEvent::ScrollFinger(sf))) => {
                let mut ax = self.get_scroll_event(&sf);
                ax.a_source = AXIS_SOURCE_FINGER;
                return Some(InputEvent::axis(ax));
            }
            Some(Event::Pointer(PointerEvent::ScrollWheel(sw))) => {
                let mut ax = self.get_scroll_event(&sw);
                ax.a_source = AXIS_SOURCE_WHEEL;

                // Mouse wheels will be handled with the higher resolution
                // v120 API for discrete scrolling
                ax.a_v120_val = Some((
                    sw.scroll_value_v120(pointer::Axis::Horizontal),
                    sw.scroll_value_v120(pointer::Axis::Vertical),
                ));

                return Some(InputEvent::axis(ax));
            }
            Some(Event::Pointer(PointerEvent::Button(b))) => {
                log::debug!("pointer button {:?}", b.button());

                return Some(InputEvent::click(Click {
                    c_code: b.button(),
                    c_state: b.button_state(),
                }));
            }
            Some(Event::Keyboard(KeyboardEvent::Key(k))) => {
                log::debug!("keyboard event: {:?}", k.key());
                return Some(InputEvent::key(Key {
                    k_code: k.key(),
                    k_state: k.key_state(),
                }));
            }
            Some(_e) => log::debug!("Unhandled Input Event: {:?}", _e),
            None => (),
        };

        return None;
    }

    fn send_pointer_frame(pointer: &Main<wl_pointer::WlPointer>) {
        if pointer.as_ref().version() >= 5 {
            pointer.frame();
        }
    }

    fn send_axis(
        pointer: &Main<wl_pointer::WlPointer>,
        axis_type: wl_pointer::Axis,
        val: f64,
        val_discrete: Option<f64>,
    ) {
        let time = get_current_millis();
        // deliver the axis events, one for each direction
        if val != 0.0 {
            if let Some(discrete) = val_discrete {
                if pointer.as_ref().version() >= 8 {
                    pointer.axis_value120(axis_type, discrete as i32);
                } else if pointer.as_ref().version() >= 5 {
                    pointer.axis_discrete(axis_type, discrete as i32);
                }
            }

            pointer.axis(time, axis_type, val);
        } else {
            // If neither axis has a non-zero value, then we should
            // tell the application that the axis series has stopped. This
            // is needed for firefox, not having it means scrolling stops working
            // when you load a page for the first time.
            if pointer.as_ref().version() >= 5 {
                pointer.axis_stop(time, axis_type);
            }
        }
    }

    /// Perform a scrolling motion.
    ///
    /// Generates the wl_pointer.axis event.
    fn handle_pointer_axis(&mut self, a: &Axis, atmos: &Atmosphere) {
        // Find the active window
        if let Some(id) = self.i_pointer_focus {
            // get the seat for this client
            if let Some(cell) = atmos.get_seat_from_window_id(id) {
                let seat = cell.borrow();
                // Get the pointer
                for si in seat.s_proxies.borrow().iter() {
                    for pointer in si.si_pointers.iter() {
                        Self::send_axis(
                            pointer,
                            wl_pointer::Axis::VerticalScroll,
                            a.a_vert_val,
                            // convert our Option<tuple> to Option<f64>
                            a.a_v120_val.map(|a| a.1),
                        );
                        Self::send_axis(
                            pointer,
                            wl_pointer::Axis::HorizontalScroll,
                            a.a_hori_val,
                            a.a_v120_val.map(|a| a.0),
                        );
                        // Send the source of this input event. This will for now be either
                        // finger scrolling on a touchpad or scroll wheel scrolling. Firefox
                        // breaks scrolling without this, I think it wants it to decide if
                        // it should do kinetic scrolling or not.
                        if pointer.as_ref().version() >= 5 {
                            pointer
                                .axis_source(wl_pointer::AxisSource::from_raw(a.a_source).unwrap());
                        }
                        Self::send_pointer_frame(pointer);
                        // Mark the atmosphere as changed so that it fires frame throttling
                        // callbacks. Otherwise we may end up sending scroll events but not
                        // telling the app to redraw, causing sutters.
                        atmos.mark_changed();
                    }
                }
            }
        }
    }

    /// This is called once per frame by the thread's main even loop. It exists to get
    /// the input system up to date and allow it to dispatch any cached state it has.
    ///
    /// Applies batched input changes to the window dimensions. We keep a `i_resize_diff`
    /// of the current pointer changes that need to have an xdg configure event for them.
    /// This method resets the diff and sends the value to xdg.
    pub fn update_from_eventloop(&mut self, atmos: &mut Atmosphere) {
        if let Some(id) = atmos.get_resizing() {
            if let Some(cell) = atmos.get_surface_from_id(id) {
                let surf = cell.borrow();
                match &surf.s_role {
                    Some(Role::xdg_shell_toplevel(ss)) => {
                        // send the xdg configure events
                        ss.borrow_mut().configure(
                            &mut atmos,
                            &surf,
                            Some((self.i_resize_diff.0 as f32, self.i_resize_diff.1 as f32)),
                        );
                    }
                    _ => (),
                }
            }

            // clear the diff so we can batch more
            self.i_resize_diff = (0.0, 0.0);
        }
    }

    /// Generate the wl_keyboard.enter event for id's seat, if it
    /// has a keyboard.
    ///
    /// Atmos is passed since this is called from `atmos.focus_on`,
    /// so atmos' rc may be held.
    pub fn keyboard_enter(atmos: &Atmosphere, id: WindowId) {
        log::error!("Keyboard entered WindowId {:?}", id);
        if let Some(cell) = atmos.get_seat_from_window_id(id) {
            let seat = cell.borrow_mut();
            // TODO: verify
            // The client may have allocated multiple seats, and we should
            // deliver events to all of them
            for si in seat.s_proxies.borrow().iter() {
                for keyboard in si.si_keyboards.iter() {
                    if let Some(surf) = atmos.get_wl_surface_from_id(id) {
                        keyboard.enter(
                            seat.s_serial,
                            &surf,
                            Vec::new(), // TODO: update modifiers if needed
                        );
                    }
                }
            }
        }
    }

    // Generate the wl_keyboard.leave event for id's seat, if it
    // has a keyboard.
    //
    // Atmos is passed since this is called from `atmos.focus_on`,
    // so atmos' rc may be held.
    pub fn keyboard_leave(atmos: &Atmosphere, id: WindowId) {
        log::error!("Keyboard left WindowId {:?}", id);
        if let Some(cell) = atmos.get_seat_from_window_id(id) {
            let seat = cell.borrow_mut();
            // TODO: verify
            // The client may have allocated multiple seats, and we should
            // deliver events to all of them
            for si in seat.s_proxies.borrow().iter() {
                for keyboard in si.si_keyboards.iter() {
                    if let Some(surf) = atmos.get_wl_surface_from_id(id) {
                        keyboard.leave(seat.s_serial, &surf);
                    }
                }
            }
        }
    }

    /// Generate the wl_pointer.enter event for id's seat, if it
    /// has a pointer.
    ///
    /// Atmos is passed since this may be called from `atmos.focus_on`,
    /// so atmos' rc may be held.
    pub fn pointer_enter(atmos: &Atmosphere, id: WindowId) {
        log::error!("Pointer entered WindowId {:?}", id);
        if let Some(cell) = atmos.get_seat_from_window_id(id) {
            if let Some(surf) = atmos.get_wl_surface_from_id(id) {
                let (cx, cy) = atmos.get_cursor_pos();
                if let Some((sx, sy)) = atmos.global_coords_to_surf(id, cx, cy) {
                    let seat = cell.borrow_mut();
                    // TODO: verify
                    // The client may have allocated multiple seats, and we should
                    // deliver events to all of them
                    for si in seat.s_proxies.borrow().iter() {
                        for pointer in si.si_pointers.iter() {
                            pointer.enter(
                                seat.s_serial,
                                &surf,
                                sx as f64,
                                sy, // surface local coordinates
                            );
                            Self::send_pointer_frame(pointer);
                        }
                    }
                }
            }
        }
    }

    /// Generate the wl_pointer.leave event for id's seat, if it
    /// has a pointer.
    ///
    /// Atmos is passed since this may be called from `atmos.focus_on`,
    /// so atmos' rc may be held.
    pub fn pointer_leave(atmos: &Atmosphere, id: WindowId) {
        log::error!("Pointer left WindowId {:?}", id);
        if let Some(cell) = atmos.get_seat_from_window_id(id) {
            let seat = cell.borrow_mut();
            // TODO: verify
            // The client may have allocated multiple seats, and we should
            // deliver events to all of them
            for si in seat.s_proxies.borrow().iter() {
                for pointer in si.si_pointers.iter() {
                    if let Some(surf) = atmos.get_wl_surface_from_id(id) {
                        pointer.leave(seat.s_serial, &surf);
                        Self::send_pointer_frame(pointer);
                    }
                }
            }
        }
    }

    /// Move the pointer
    ///
    /// Also generates wl_pointer.motion events to the surface
    /// in focus if the cursor is on that surface
    fn handle_pointer_move(&mut self, atmos: &Atmosphere, m: &PointerMove) {
        // Update the atmosphere with the new cursor pos
        atmos.add_cursor_pos(m.pm_dx, m.pm_dy);

        // If a resize is happening then collect the cursor changes
        // to send at the end of the frame
        if atmos.get_resizing().is_some() {
            log::error!("Resizing in progress");
            self.i_resize_diff.0 += m.pm_dx;
            self.i_resize_diff.1 += m.pm_dy;
            return;
        }
        // Get the cursor position
        let (cx, cy) = atmos.get_cursor_pos();

        // Get the window the pointer is over
        let focus = atmos.find_window_with_input_at_point(cx as f32, cy as f32);
        // If the pointer is over top of a different window, change the
        // pointer focus and send the leave/enter events
        if focus != self.i_pointer_focus {
            if let Some(id) = self.i_pointer_focus {
                Input::pointer_leave(&atmos, id);
            }
            if let Some(id) = focus {
                Input::pointer_enter(&atmos, id);
            }
            self.i_pointer_focus = focus;
        }

        // deliver the motion event
        if let Some(id) = focus {
            if let Some(cell) = atmos.get_seat_from_window_id(id) {
                // get the seat for this client
                let seat = cell.borrow();
                // Get the pointer
                for si in seat.s_proxies.borrow().iter() {
                    for pointer in si.si_pointers.iter() {
                        // If the pointer is over this surface
                        if let Some((sx, sy)) = atmos.global_coords_to_surf(id, cx, cy) {
                            // deliver the motion event
                            pointer.motion(get_current_millis(), sx, sy);
                            Self::send_pointer_frame(pointer);
                        }
                    }
                }
            }
        }
    }

    /// Delivers the wl_pointer.button event to any surface in focus.
    ///
    /// This is the big ugly state machine for processing an input
    /// token that was the result of clicking the pointer. We need
    /// to find what the cursor is over and perform the appropriate
    /// action.
    ///
    /// If a click is over a background window it is brought into focus
    /// clicking on a background titlebar can also start a grab
    fn handle_click_on_window(&mut self, atmos: &Atmosphere, c: &Click) {
        let cursor = atmos.get_cursor_pos();
        // did our click bring a window into focus?
        let mut set_focus = false;

        // first check if we are releasing a grab
        if let Some(_id) = atmos.get_grabbed() {
            match c.c_state {
                ButtonState::Released => {
                    log::debug!("Ungrabbing window {:?}", _id);
                    atmos.set_grabbed(None);
                    return;
                }
                _ => (),
            }
        }

        // find the window under the cursor
        let resizing = atmos.get_resizing();
        if resizing.is_some() && c.c_state == ButtonState::Released {
            // We are releasing a resize, and we might not be resizing
            // the same window as find_window_at_point would report
            if let Some(id) = resizing {
                // if on one of the edges start a resize
                if let Some(surf) = atmos.get_surface_from_id(id) {
                    match &surf.borrow_mut().s_role {
                        Some(Role::xdg_shell_toplevel(ss)) => {
                            match c.c_state {
                                // The release is handled above
                                ButtonState::Released => {
                                    log::debug!("Stopping resize of {:?}", id);
                                    atmos.set_resizing(None);
                                    ss.borrow_mut().ss_cur_tlstate.tl_resizing = false;
                                    // TODO: send final configure here?
                                }
                                // this should never be pressed
                                _ => (),
                            }
                        }
                        // TODO: resizing for other shell types
                        _ => (),
                    }
                }
            }
        } else if let Some(id) =
            atmos.find_window_with_input_at_point(cursor.0 as f32, cursor.1 as f32)
        {
            // If the surface's root window is not in focus, make it in focus
            if let Some(focus) = atmos.get_root_win_in_focus() {
                // If this is a surface that is part of a subsurface stack,
                // get the root id. Otherwise this is a root.
                let root = match atmos.get_root_window(id) {
                    Some(root) => root,
                    None => id,
                };

                if root != focus && c.c_state == ButtonState::Pressed {
                    set_focus = true;
                }
            } else {
                // If no window is in focus, then we have just clicked on
                // one and should focus on it
                set_focus = true;
            }

            if set_focus {
                // Tell atmos that this is the one in focus
                atmos.focus_on(Some(id));
            }

            // do this first here so we don't do it more than once
            let edge = atmos.point_is_on_window_edge(id, cursor.0 as f32, cursor.1 as f32);

            // First check if we are over an edge, or if we are resizing
            // and released the click
            if edge != ResizeEdge::None {
                // if on one of the edges start a resize
                if let Some(surf) = atmos.get_surface_from_id(id) {
                    match &surf.borrow_mut().s_role {
                        Some(Role::xdg_shell_toplevel(ss)) => {
                            match c.c_state {
                                ButtonState::Pressed => {
                                    log::debug!("Resizing window {:?}", id);
                                    atmos.set_resizing(Some(id));
                                    ss.borrow_mut().ss_cur_tlstate.tl_resizing = true;
                                }
                                // releasing is handled above
                                _ => (),
                            }
                        }
                        // TODO: resizing for other shell types
                        _ => (),
                    }
                }
            } else if atmos.point_is_on_titlebar(id, cursor.0 as f32, cursor.1 as f32) {
                // now check if we are over the titlebar
                // if so we will grab the bar
                match c.c_state {
                    ButtonState::Pressed => {
                        log::debug!("Grabbing window {:?}", id);
                        atmos.set_grabbed(Some(id));
                    }
                    ButtonState::Released => {
                        log::debug!("Ungrabbing window {:?}", id);
                        atmos.set_grabbed(None);
                    }
                }
            } else if !set_focus {
                // else the click was over the meat of the window, so
                // deliver the event to the wayland client

                // get the seat for this client
                if let Some(cell) = atmos.get_seat_from_window_id(id) {
                    let seat = cell.borrow_mut();
                    for si in seat.s_proxies.borrow().iter() {
                        for pointer in si.si_pointers.iter() {
                            // Trigger a button event
                            pointer.button(
                                seat.s_serial,
                                get_current_millis(),
                                c.c_code,
                                match c.c_state {
                                    ButtonState::Pressed => wl_pointer::ButtonState::Pressed,
                                    ButtonState::Released => wl_pointer::ButtonState::Released,
                                },
                            );
                            Self::send_pointer_frame(pointer);
                        }
                    }
                }
            }
        }
    }

    // TODO: add gesture recognition
    pub fn handle_compositor_shortcut(&mut self, atmos: &Atmosphere, key: &Key) -> bool {
        // TODO: keysyms::KEY_Meta_L doesn't work? should be 125 for left meta
        if key.k_code == 125 && key.k_state == KeyState::Pressed {
            match atmos.get_renderdoc_recording() {
                true => atmos.set_renderdoc_recording(false),
                false => atmos.set_renderdoc_recording(true),
            }
            return true;
        }
        return false;
    }

    /// Handle the user typing on the keyboard.
    ///
    /// Deliver the wl_keyboard.key and modifier events.
    pub fn handle_keyboard(&mut self, atmos: &Atmosphere, key: &Key) {
        if self.handle_compositor_shortcut(key) {
            return;
        }

        // Do the xkbcommon keyboard update first, since it needs to happen
        // even if there isn't a window in focus
        // let xkb keep track of the keyboard state
        let changed = self.i_xkb_state.update_key(
            // add 8 to account for differences between evdev and x11
            key.k_code + 8,
            match key.k_state {
                KeyState::Pressed => xkb::KeyDirection::Down,
                KeyState::Released => xkb::KeyDirection::Up,
            },
        );

        // if any modifiers were touched we should send their event
        let mods = if changed != 0 {
            // First we need to update our own tracking of what keys are held down
            self.i_mod_ctrl = self
                .i_xkb_state
                .mod_name_is_active(&xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
            self.i_mod_alt = self
                .i_xkb_state
                .mod_name_is_active(&xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE);
            self.i_mod_shift = self
                .i_xkb_state
                .mod_name_is_active(&xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE);
            self.i_mod_caps = self
                .i_xkb_state
                .mod_name_is_active(&xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE);
            self.i_mod_meta = self
                .i_xkb_state
                .mod_name_is_active(&xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE);
            self.i_mod_num = self
                .i_xkb_state
                .mod_name_is_active(&xkb::MOD_NAME_NUM, xkb::STATE_MODS_EFFECTIVE);

            // Now we can serialize the modifiers into a format suitable
            // for sending to the client
            let depressed = self.i_xkb_state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
            let latched = self.i_xkb_state.serialize_mods(xkb::STATE_MODS_LATCHED);
            let locked = self.i_xkb_state.serialize_mods(xkb::STATE_MODS_LOCKED);
            let layout = self.i_xkb_state.serialize_layout(xkb::STATE_LAYOUT_LOCKED);

            Some((depressed, latched, locked, layout))
        } else {
            None
        };

        // if there is a window in focus
        if let Some(id) = atmos.get_client_in_focus() {
            // get the seat for this client
            if let Some(cell) = atmos.get_seat_from_client_id(id) {
                let mut seat = cell.borrow_mut();
                for si in seat.s_proxies.borrow().iter() {
                    for keyboard in si.si_keyboards.iter() {
                        if let Some((depressed, latched, locked, layout)) = mods {
                            // Finally fire the wayland event
                            log::debug!("Sending modifiers to window {:?}", id);
                            keyboard.modifiers(seat.s_serial, depressed, latched, locked, layout);
                        }

                        // give the keycode to the client
                        let time = get_current_millis();
                        let state = map_key_state(key.k_state);
                        log::debug!("Sending key {} to window {:?}", key.k_code, id);
                        keyboard.key(seat.s_serial, time, key.k_code, state);
                    }
                }
                // increment the serial for next time
                seat.s_serial += 1;
            }
        }
        // otherwise the click is over the background, so
        // ignore it
    }

    /// Dispatch an arbitrary input event
    ///
    /// Input events are either handled by us or by the wayland client
    /// we need to figure out the appropriate destination and perform
    /// the right action.
    pub fn handle_input_event(&mut self, iev: &InputEvent) {
        match iev {
            InputEvent::pointer_move(m) => self.handle_pointer_move(m),
            InputEvent::axis(a) => self.handle_pointer_axis(a),
            InputEvent::click(c) => self.handle_click_on_window(c),
            InputEvent::key(k) => self.handle_keyboard(k),
        }
    }
}
