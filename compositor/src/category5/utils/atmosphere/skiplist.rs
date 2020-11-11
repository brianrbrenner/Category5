// Support code for handling window heirarchies
//
// Austin Shafer - 2020

use crate::category5::utils::atmosphere::*;
use crate::category5::utils::WindowId;
use crate::category5::input::Input;

// A skiplist is an entry in a linked list designed to be
// added in the atmosphere's property system
//
// The idea is that each window has one of these
// which points to the next and previous windows in
// the global ordering for that desktop. These properties
// will be consistently published by the atmosphere just
// like the rest.

impl Atmosphere {
    /// Removes a window from the heirarchy.
    ///
    /// Use this to pull a window out, and then insert it in focus
    pub fn skiplist_remove_window(&mut self, id: WindowId) {
        let next = self.get_skiplist_next(id);
        let prev = self.get_skiplist_prev(id);

        // TODO: recalculate skip
        if let Some(p) = prev {
            self.set_skiplist_next(p, next);
        }
        if let Some(n) = next {
            self.set_skiplist_prev(n, prev);
        }

        // If this was the window in focus, set the focus
        // to the next window in the order
        let focus = self.get_window_in_focus();
        if let Some(f) = focus {
            if f == id {
                self.set_global_prop(&GlobalProperty::focus(next));
            }
        }
    }

    /// Add a window above another
    ///
    /// This is used for the subsurface ordering requests
    pub fn skiplist_place_above(&mut self,
                                id: WindowId,
                                target: WindowId)
    {
        // remove id from its skiplist just in case
        self.skiplist_remove_window(id);

        // TODO: recalculate skip
        let prev = self.get_skiplist_prev(target);
        if let Some(p) = prev {
            self.set_skiplist_next(p, Some(id));
        }
        self.set_skiplist_prev(target, Some(id));

        // Now point id to the target and its neighbor
        self.set_skiplist_prev(id, prev);
        self.set_skiplist_next(id, Some(target));
    }

    /// Add a window below another
    ///
    /// This is used for the subsurface ordering requests
    pub fn skiplist_place_below(&mut self,
                                id: WindowId,
                                target: WindowId)
    {
        // remove id from its skiplist just in case
        self.skiplist_remove_window(id);

        // TODO: recalculate skip
        let next = self.get_skiplist_next(target);
        if let Some(n) = next {
            self.set_skiplist_prev(n, Some(id));
        }
        self.set_skiplist_next(target, Some(id));

        // Now point id to the target and its neighbor
        self.set_skiplist_prev(id, Some(target));
        self.set_skiplist_next(id, next);
    }

    /// Get the window currently in use
    pub fn get_window_in_focus(&self) -> Option<WindowId> {
        match self.get_global_prop(GlobalProperty::FOCUS) {
            Some(GlobalProperty::focus(w)) => *w,
            None => None,
            _ => panic!("property not found"),
        }
    }

    /// Get the client in focus.
    /// This is better for subsystems like input which need to
    /// find the seat of the client currently in use.
    pub fn get_client_in_focus(&self) -> Option<ClientId> {
        // get the surface in focus
        if let Some(win) = self.get_window_in_focus() {
            // now get the client for that surface
            return Some(self.get_owner(win));
        }
        return None;
    }

    /// Set the window currently in focus
    pub fn focus_on(&mut self, win: Option<WindowId>) {
        log!(LogLevel::debug, "focusing on window {:?}", win);
        if let Some(id) = win {
            let prev_focus = self.get_window_in_focus();
            // If they clicked on the focused window, don't
            // do anything
            if let Some(prev) = prev_focus {
                if prev == id {
                    return;
                }

                // Send leave event(s) to the old focus
                Input::keyboard_leave(self, prev);

                // point the previous focus at the new focus
                self.set_skiplist_prev(prev, win);
            }
            // Send enter event(s) to the new focus
            // spec says this MUST be done after the leave events are sent
            Input::keyboard_enter(self, id);

            self.skiplist_remove_window(id);
            self.set_skiplist_next(id, prev_focus);
            self.set_skiplist_prev(id, None);
        }
        self.set_global_prop(&GlobalProperty::focus(win));

        // TODO: recalculate skip
    }
}

// (see PropertyMapIterator for lifetime comments
impl<'a> Atmosphere {
    /// return an iterator of valid ids.
    ///
    /// This will be all ids that are have been `activate`d
    pub fn visible_windows(&'a self) -> VisibleWindowIterator<'a> {
        self.into_iter()
    }

    /// return an iterator over the subsurfaces of id
    ///
    /// This will be all ids that are have been `activate`d
    pub fn visible_subsurfaces(&'a self, id: WindowId)
                               -> VisibleWindowIterator<'a>
    {
        VisibleWindowIterator {
            vwi_atmos: &self,
            vwi_cur: self.get_top_child(id),
        }
    }
}

// Iterator for visible windows in a desktop
pub struct VisibleWindowIterator<'a> {
    vwi_atmos: &'a Atmosphere,
    // the current window we are on
    vwi_cur: Option<WindowId>,
}

// Non-consuming iterator over an Atmosphere
//
// This will only show the visible windows
impl<'a> IntoIterator for &'a Atmosphere {
    type Item = WindowId;
    type IntoIter = VisibleWindowIterator<'a>;

    // note that into_iter() is consuming self
    fn into_iter(self) -> Self::IntoIter {
        VisibleWindowIterator {
            vwi_atmos: &self,
            vwi_cur: self.get_window_in_focus(),
        }
    }
}

impl<'a> Iterator for VisibleWindowIterator<'a> {
    // Our item type is a WindowId
    type Item = WindowId;

    fn next(&mut self) -> Option<WindowId> {
        let ret = self.vwi_cur.take();
        // TODO: actually skip
        if let Some(id) = ret {
            self.vwi_cur = self.vwi_atmos.get_skiplist_next(id);
        }

        return ret;
    }
}
