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

extern crate dakota as dak;
extern crate nix;
extern crate wayland_protocols;
extern crate wayland_server as ws;
extern crate xkbcommon;

use wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use ws::protocol::wl_keyboard;
use ws::protocol::wl_pointer;
use ws::Resource;

use crate::category5::atmosphere::{Atmosphere, SurfaceId};
use crate::category5::vkcomp::wm;
use crate::category5::ways::role::Role;
use utils::{log, timing::*};

use xkbcommon::xkb;

use core::convert::TryFrom;

/// This represents an input system
///
/// Input is grabbed from the udev interface, but
/// any method should be applicable. It just feeds
/// the ways and wm subsystems input events
///
/// We will also stash our xkb resources here, and
/// will consult this before sending out keymaps/syms
pub struct Input {
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
}

#[derive(Copy, Eq, PartialEq, Clone)]
enum ButtonState {
    Pressed,
    Released,
}

// A helper function to map a KeyState from the input event
// into a KeyState from wl_keyboard
fn map_key_state(state: ButtonState) -> wl_keyboard::KeyState {
    match state {
        ButtonState::Pressed => wl_keyboard::KeyState::Pressed,
        ButtonState::Released => wl_keyboard::KeyState::Released,
    }
}

// NOTE:
// The XKB entries above are not marked send/sync. Due to the way
// cat5 is written they will never be used from multiple threads,
// so we can safely mark this input handler as sendable
unsafe impl Send for Input {}

impl Input {
    /// Create an input subsystem.
    ///
    /// Setup the libinput library from a udev context
    pub fn new() -> Input {
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
        }
    }

    fn send_pointer_frame(pointer: &wl_pointer::WlPointer) {
        if pointer.version() >= 5 {
            pointer.frame();
        }
    }

    fn send_axis(
        pointer: &wl_pointer::WlPointer,
        axis_type: wl_pointer::Axis,
        val: f64,
        val_discrete: f64,
    ) {
        let time = get_current_millis();
        // deliver the axis events, one for each direction
        if val != 0.0 {
            if val_discrete != 0.0 && pointer.version() >= 8 {
                pointer.axis_value120(axis_type, val_discrete as i32);
            } else {
                if val_discrete != 0.0 && pointer.version() >= 5 {
                    // Divide by 120 here to go from our v120 value to a discrete
                    // -1/+1 value. We have to do this since libinput's axis_value_discrete
                    // is deprecated
                    pointer.axis_discrete(axis_type, val_discrete as i32 / 120);
                }
                pointer.axis(time, axis_type, val);
            }
        } else {
            // If neither axis has a non-zero value, then we should
            // tell the application that the axis series has stopped. This
            // is needed for firefox, not having it means scrolling stops working
            // when you load a page for the first time.
            if pointer.version() >= 5 {
                pointer.axis_stop(time, axis_type);
            }
        }
    }

    /// Perform a scrolling motion.
    ///
    /// Generates the wl_pointer.axis event.
    fn handle_pointer_axis(
        &mut self,
        atmos: &mut Atmosphere,
        xrel: Option<f64>,
        yrel: Option<f64>,
        v120_val: (f64, f64),
        source: dak::AxisSource,
    ) {
        // Find the active window
        if let Some(id) = atmos.get_pointer_focus() {
            // get the seat for this client
            if let Some(cell) = atmos.get_seat_from_surface_id(&id) {
                let seat = cell.lock().unwrap();
                // Get the pointer
                for si in seat.s_proxies.iter() {
                    for pointer in si.si_pointers.iter() {
                        // Send the source of this input event. This will for now be either
                        // finger scrolling on a touchpad or scroll wheel scrolling. Firefox
                        // breaks scrolling without this, I think it wants it to decide if
                        // it should do kinetic scrolling or not.
                        if pointer.version() >= 5 {
                            pointer.axis_source(
                                wl_pointer::AxisSource::try_from(source as u32).unwrap(),
                            );
                        }
                        if let Some(hori_val) = xrel {
                            Self::send_axis(
                                pointer,
                                wl_pointer::Axis::HorizontalScroll,
                                hori_val,
                                v120_val.0,
                            );
                        }
                        if let Some(vert_val) = yrel {
                            Self::send_axis(
                                pointer,
                                wl_pointer::Axis::VerticalScroll,
                                vert_val,
                                // convert our Option<tuple> to Option<f64>
                                v120_val.1,
                            );
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

    /// Generate the wl_keyboard.enter event for id's seat, if it
    /// has a keyboard.
    ///
    /// Atmos is passed since this is called from `atmos.focus_on`,
    /// so atmos' rc may be held.
    pub fn keyboard_enter(atmos: &Atmosphere, id: &SurfaceId) {
        log::error!("Keyboard entered SurfaceId {:?}", id);
        if let Some(cell) = atmos.get_seat_from_surface_id(id) {
            let seat = cell.lock().unwrap();
            // TODO: verify
            // The client may have allocated multiple seats, and we should
            // deliver events to all of them
            for si in seat.s_proxies.iter() {
                for keyboard in si.si_keyboards.iter() {
                    if let Some(surf) = atmos.get_wl_surface_from_id(id) {
                        keyboard.enter(
                            seat.s_serial,
                            &surf,
                            Vec::with_capacity(0), // TODO: update modifiers if needed
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
    pub fn keyboard_leave(atmos: &Atmosphere, id: &SurfaceId) {
        log::error!("Keyboard left SurfaceId {:?}", id);
        if let Some(cell) = atmos.get_seat_from_surface_id(id) {
            let seat = cell.lock().unwrap();
            // TODO: verify
            // The client may have allocated multiple seats, and we should
            // deliver events to all of them
            for si in seat.s_proxies.iter() {
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
    pub fn pointer_enter(atmos: &Atmosphere, id: &SurfaceId) {
        log::error!("Pointer entered SurfaceId {:?}", id);
        if let Some(cell) = atmos.get_seat_from_surface_id(id) {
            if let Some(surf) = atmos.get_wl_surface_from_id(id) {
                let (cx, cy) = atmos.get_cursor_pos();
                // Get our surface coordinates
                if let Some((sx, sy)) = atmos.global_coords_to_surf(id, cx, cy) {
                    let seat = cell.lock().unwrap();
                    // TODO: verify
                    // The client may have allocated multiple seats, and we should
                    // deliver events to all of them
                    for si in seat.s_proxies.iter() {
                        for pointer in si.si_pointers.iter() {
                            pointer.enter(seat.s_serial, &surf, sx, sy);
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
    pub fn pointer_leave(atmos: &mut Atmosphere, id: &SurfaceId) {
        log::error!("Pointer left SurfaceId {:?}", id);

        // Clear the current cursor image
        atmos.add_wm_task(wm::task::Task::reset_cursor);

        if let Some(cell) = atmos.get_seat_from_surface_id(id) {
            let seat = cell.lock().unwrap();
            // TODO: verify
            // The client may have allocated multiple seats, and we should
            // deliver events to all of them
            for si in seat.s_proxies.iter() {
                for pointer in si.si_pointers.iter() {
                    if let Some(surf) = atmos.get_wl_surface_from_id(id) {
                        pointer.leave(seat.s_serial, &surf);
                        Self::send_pointer_frame(pointer);
                    }
                }
            }
        }
    }

    /// Send a resize xdg_shell resize configure
    ///
    /// We need to do this on pointer movement to tell the client how
    /// to resize themselves.
    fn send_resize_configure(&mut self, atmos: &mut Atmosphere) {
        log::error!("Resizing in progress");
        if let Some(id) = atmos.get_resizing() {
            if let Some(cell) = atmos.get_surface_from_id(&id) {
                let mut surf = cell.lock().unwrap();
                let (xdg_surf, ss) = match &surf.s_role {
                    Some(Role::xdg_shell_toplevel(xs, ss)) => (xs.clone(), ss.clone()),
                    _ => panic!("Resizing unsupported shell type"), // TODO: other shells
                };

                // send the xdg configure events
                ss.lock().unwrap().configure(
                    atmos, xdg_surf, &mut surf, true, // resizing
                );
            }
        }
    }

    /// Move the pointer
    ///
    /// Also generates wl_pointer.motion events to the surface
    /// in focus if the cursor is on that surface
    fn handle_pointer_move(&mut self, atmos: &mut Atmosphere, dx: f64, dy: f64) {
        // Update the atmosphere with the new cursor pos
        atmos.add_cursor_pos(dx, dy);

        // If a resize is happening then collect the cursor changes
        // to send at the end of the frame
        if atmos.get_resizing().is_some() {
            self.send_resize_configure(atmos);
            return;
        }

        let (cx, cy) = atmos.get_cursor_pos();
        atmos.recalculate_pointer_focus();

        // deliver the motion event
        if let Some(id) = atmos.get_pointer_focus() {
            if let Some(cell) = atmos.get_seat_from_surface_id(&id) {
                // get the seat for this client
                let seat = cell.lock().unwrap();
                // Get the pointer
                for si in seat.s_proxies.iter() {
                    for pointer in si.si_pointers.iter() {
                        // If the pointer is over this surface
                        if let Some((sx, sy)) = atmos.global_coords_to_surf(&id, cx, cy) {
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
    fn handle_click_on_window(
        &mut self,
        atmos: &mut Atmosphere,
        button: dak::MouseButton,
        state: ButtonState,
    ) {
        let cursor = atmos.get_cursor_pos();

        // first check if we are releasing a grab
        if let Some(_id) = atmos.get_grabbed() {
            match state {
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
        if resizing.is_some() && state == ButtonState::Released {
            if state == ButtonState::Released {
                // We are releasing a resize, and we might not be resizing
                // the same window as find_window_at_point would report
                if let Some(id) = resizing.as_ref() {
                    // if on one of the edges start a resize
                    if let Some(surf) = atmos.get_surface_from_id(id) {
                        let mut surf = surf.lock().unwrap();
                        surf.s_state
                            .cs_xdg_state
                            .xs_tlstate
                            .as_mut()
                            .unwrap()
                            .tl_resizing = false;

                        let (xdg_surf, ss) = match &surf.s_role {
                            Some(Role::xdg_shell_toplevel(xs, ss)) => (xs.clone(), ss.clone()),
                            _ => panic!("Resizing unsupported shell type"), // TODO: other shells
                        };

                        log::debug!("Stopping resize of {:?}", id);
                        atmos.set_resizing(None);
                        let mut ss = ss.lock().unwrap();
                        // As per spec send final configure here
                        ss.configure(atmos, xdg_surf, &mut surf, false);
                    }
                }
            }
        } else if let Some(id) =
            atmos.find_window_with_input_at_point(cursor.0 as f32, cursor.1 as f32)
        {
            // will our click bring a window into focus?
            let mut set_focus = false;
            if let Some(focus) = atmos.get_root_win_in_focus() {
                // If this is a surface that is part of a subsurface stack,
                // get the root id. Otherwise this is a root.
                let root = match atmos.a_root_window.get_clone(&id) {
                    Some(root) => root,
                    None => id.clone(),
                };
                if root != focus && state == ButtonState::Pressed {
                    set_focus = true;
                }
            }

            // Tell atmos that this is the one in focus
            atmos.focus_on(Some(id.clone()));

            // do this first here so we don't do it more than once
            let edge = atmos.point_is_on_window_edge(&id, cursor.0 as f32, cursor.1 as f32);

            // First check if we are over an edge, or if we are resizing
            // and released the click
            if edge != ResizeEdge::None {
                // if on one of the edges start a resize
                if let Some(surf_cell) = atmos.get_surface_from_id(&id) {
                    let mut surf = surf_cell.lock().unwrap();
                    if let Some(Role::xdg_shell_toplevel(_, _)) = &mut surf.s_role {
                        if state == ButtonState::Pressed {
                            log::debug!("Resizing window {:?}", id);
                            atmos.set_resizing(Some(id));
                            surf.s_state
                                .cs_xdg_state
                                .xs_tlstate
                                .as_mut()
                                .unwrap()
                                .tl_resizing = false;
                        }
                    }
                }
            } else if atmos.point_is_on_titlebar(&id, cursor.0 as f32, cursor.1 as f32) {
                // now check if we are over the titlebar
                // if so we will grab the bar
                match state {
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
                if let Some(cell) = atmos.get_seat_from_surface_id(&id) {
                    let seat = cell.lock().unwrap();
                    for si in seat.s_proxies.iter() {
                        for pointer in si.si_pointers.iter() {
                            // Trigger a button event
                            pointer.button(
                                seat.s_serial,
                                get_current_millis(),
                                button.to_linux_button_code(),
                                match state {
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
    fn handle_compositor_shortcut(
        &mut self,
        atmos: &mut Atmosphere,
        key: dak::Keycode,
        state: ButtonState,
    ) -> bool {
        // TODO: keysyms::KEY_Meta_L doesn't work? should be 125 for left meta
        if key == dak::Keycode::LMETA && state == ButtonState::Pressed {
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
    fn handle_keyboard(
        &mut self,
        atmos: &mut Atmosphere,
        dakota_key: dak::Keycode,
        key: u32,
        state: ButtonState,
    ) {
        if self.handle_compositor_shortcut(atmos, dakota_key, state) {
            return;
        }

        // Do the xkbcommon keyboard update first, since it needs to happen
        // even if there isn't a window in focus
        // let xkb keep track of the keyboard state
        let changed = self.i_xkb_state.update_key(
            // add 8 to account for differences between evdev and x11
            key + 8,
            match state {
                ButtonState::Pressed => xkb::KeyDirection::Down,
                ButtonState::Released => xkb::KeyDirection::Up,
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
            if let Some(cell) = atmos.get_seat_from_client_id(&id) {
                let mut seat = cell.lock().unwrap();
                for si in seat.s_proxies.iter() {
                    for keyboard in si.si_keyboards.iter() {
                        if let Some((depressed, latched, locked, layout)) = mods {
                            // Finally fire the wayland event
                            log::debug!("Sending modifiers to window {:?}", id);
                            keyboard.modifiers(seat.s_serial, depressed, latched, locked, layout);
                        }

                        // give the keycode to the client
                        let time = get_current_millis();
                        let state = map_key_state(state);
                        log::debug!("Sending key {} to window {:?}", key, id);
                        keyboard.key(seat.s_serial, time, key, state);
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
    pub fn handle_input_event(&mut self, atmos: &mut Atmosphere, ev: &dak::Event) {
        match ev {
            dak::Event::InputMouseMove { dx, dy } => self.handle_pointer_move(atmos, *dx, *dy),
            dak::Event::InputScroll {
                xrel,
                yrel,
                v120_val,
                source,
                ..
            } => self.handle_pointer_axis(atmos, *xrel, *yrel, *v120_val, *source),
            dak::Event::InputMouseButtonUp { button, .. } => {
                self.handle_click_on_window(atmos, *button, ButtonState::Released)
            }
            dak::Event::InputMouseButtonDown { button, .. } => {
                self.handle_click_on_window(atmos, *button, ButtonState::Pressed)
            }
            dak::Event::InputKeyUp {
                key, raw_keycode, ..
            } => self.handle_keyboard(
                atmos,
                *key,
                match raw_keycode {
                    dak::RawKeycode::Linux(k) => *k,
                },
                ButtonState::Released,
            ),
            dak::Event::InputKeyDown {
                key, raw_keycode, ..
            } => self.handle_keyboard(
                atmos,
                *key,
                match raw_keycode {
                    dak::RawKeycode::Linux(k) => *k,
                },
                ButtonState::Pressed,
            ),
            _ => (),
        };
    }
}
