//! Event Handling
//!
// Austin Shafer - 2022

use crate::dom;
use crate::dom::DakotaDOM;
use crate::input::{Keycode, Mods, MouseButton};
use std::rc::Rc;
use utils::ecs::{ECSId, ECSInstance};

pub struct EventSystem {
    /// The global event queue
    /// This will be iterable after dispatching, and
    /// must be cleared (all events handled) before the
    /// next dispatch
    es_global_event_queue: Vec<Event>,
    /// The compiled set of event handlers.
    es_handler_ecs_inst: ECSInstance,
    /// Ties a string name to a handler id
    /// (name, Id)
    es_name_to_handler_map: Vec<(String, ECSId)>,
}

impl EventSystem {
    pub fn new() -> Self {
        let handler_ecs = ECSInstance::new();

        Self {
            es_global_event_queue: Vec::new(),
            es_handler_ecs_inst: handler_ecs,
            es_name_to_handler_map: Vec::new(),
        }
    }
}

pub type HandlerArgs = Rc<Vec<String>>;

#[derive(Debug)]
pub enum Event {
    WindowResized { args: HandlerArgs, size: dom::Size },
    WindowClosed { args: HandlerArgs },
    WindowRedrawComplete { args: HandlerArgs },
    InputKeyDown { key: Keycode, modifiers: Mods },
    InputKeyUp { key: Keycode, modifiers: Mods },
    InputMouseButtonDown { button: MouseButton, x: i32, y: i32 },
    InputMouseButtonUp { button: MouseButton, x: i32, y: i32 },
}

impl EventSystem {
    pub fn clear_event_queue(&mut self) {
        self.es_global_event_queue.clear();
    }

    /// Look up the ECS Id of an Event Handler given its string name
    ///
    /// This is used to go from the human name the app gave the handler to
    /// the O(1) integer id of the handler that dakota will use.
    pub fn get_handler_id_from_name(&mut self, name: String) -> ECSId {
        // first, get the ECS id for this name
        // check if this event handler has already been defined
        match self.es_name_to_handler_map.iter().find(|(n, _)| *n == name) {
            Some((_, ecs_id)) => ecs_id.clone(),
            // otherwise make a new id for it, it's a new name
            None => {
                let ecs_id = self.es_handler_ecs_inst.mint_new_id();
                self.es_name_to_handler_map
                    .push((name.clone(), ecs_id.clone()));
                ecs_id
            }
        }
    }

    /// Add a window resize event to the global queue
    ///
    /// This signifies that a window was resized, and is triggered
    /// anytime OOD is returned from thundr.
    pub fn add_event_window_resized(&mut self, dom: &DakotaDOM, new_size: dom::Size) {
        if let Some(events) = dom.window.events.as_ref() {
            if let Some(handler) = events.resize.as_ref() {
                self.es_global_event_queue.push(Event::WindowResized {
                    args: handler.args.clone(),
                    size: new_size,
                });
            }
        }
    }

    /// Add a redraw request completion to the global queue
    ///
    /// Since while dispatching it isn't guaranteed that a redraw
    /// will take place, this lets a client know that the previous frame
    /// was drawn, and it should handle any once-per-frame actions it
    /// needs to take.
    ///
    /// This isn't a performance limiting event, the app doesn't need to
    /// use this to control drawing. This should be used to queue up the
    /// next elements to be presented, or run subroutines. Dakota will
    /// internally worry about drawing everything.
    pub fn add_event_window_redraw_complete(&mut self, dom: &DakotaDOM) {
        if let Some(events) = dom.window.events.as_ref() {
            if let Some(handler) = events.redraw_complete.as_ref() {
                self.es_global_event_queue
                    .push(Event::WindowRedrawComplete {
                        args: handler.args.clone(),
                    });
            }
        }
    }

    /// Notify the app that a window was closed
    ///
    /// This is not an optional event. It will always be sent. It is
    /// optional in the element tree however.
    pub fn add_event_window_closed(&mut self, dom: &DakotaDOM) {
        if let Some(events) = dom.window.events.as_ref() {
            if let Some(handler) = events.closed.as_ref() {
                self.es_global_event_queue.push(Event::WindowClosed {
                    args: handler.args.clone(),
                });
                return;
            }
        }

        // If we couldn't get the arg array from the tree, then
        // just create an empty one
        self.es_global_event_queue.push(Event::WindowClosed {
            args: Rc::new(Vec::with_capacity(0)),
        });
    }

    pub fn add_event_key_down(&mut self, _dom: &DakotaDOM, key: Keycode, mods: Mods) {
        self.es_global_event_queue.push(Event::InputKeyDown {
            key: key,
            modifiers: mods,
        });
    }
    pub fn add_event_key_up(&mut self, _dom: &DakotaDOM, key: Keycode, mods: Mods) {
        self.es_global_event_queue.push(Event::InputKeyUp {
            key: key,
            modifiers: mods,
        });
    }

    pub fn add_event_mouse_button_down(
        &mut self,
        _dom: &DakotaDOM,
        button: MouseButton,
        x: i32,
        y: i32,
    ) {
        self.es_global_event_queue
            .push(Event::InputMouseButtonDown {
                button: button,
                x: x,
                y: y,
            });
    }
    pub fn add_event_mouse_button_up(
        &mut self,
        _dom: &DakotaDOM,
        button: MouseButton,
        x: i32,
        y: i32,
    ) {
        self.es_global_event_queue.push(Event::InputMouseButtonUp {
            button: button,
            x: x,
            y: y,
        });
    }

    /// Get the slice of currently unhandled events
    ///
    /// The app should do this in its main loop after dispatching.
    /// These will be cleared during each dispatch.
    pub fn get_events<'a>(&'a self) -> &'a [Event] {
        self.es_global_event_queue.as_slice()
    }
}