// Support code for handling window heirarchies
//
// Austin Shafer - 2020

use super::*;
use crate::category5::input::Input;
use utils::{log, ClientId, WindowId};

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
    }

    /// Add a window above another
    ///
    /// This is used for the subsurface ordering requests
    pub fn skiplist_place_above(&mut self, id: WindowId, target: WindowId) {
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
    pub fn skiplist_place_below(&mut self, id: WindowId, target: WindowId) {
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

    /// Get the client in focus.
    /// This is better for subsystems like input which need to
    /// find the seat of the client currently in use.
    pub fn get_client_in_focus(&self) -> Option<ClientId> {
        // get the surface in focus
        if let Some(win) = self.get_win_focus() {
            // now get the client for that surface
            return Some(self.get_owner(win));
        }
        return None;
    }

    /// Set the window currently in focus
    pub fn focus_on(&mut self, win: Option<WindowId>) {
        log::debug!("focusing on window {:?}", win);

        if let Some(id) = win {
            // check if a new app was selected
            let prev_win_focus = self.get_win_focus();
            if let Some(prev) = prev_win_focus {
                let mut update_app = false;
                if let Some(root) = self.get_root_window(id) {
                    if root != prev {
                        update_app = true;
                    } else {
                        // If this window is already selected, just bail
                        return;
                    }
                } else if prev != id {
                    // If the root window was None, then win *is* a root
                    // window, and we still need to check it
                    update_app = true;
                }

                // if so, update window focus
                if update_app {
                    self.set_win_focus(win);

                    self.skiplist_remove_window(id);
                    // point the previous focus at the new focus
                    self.set_skiplist_prev(prev, win);
                    self.set_skiplist_next(id, prev_focus);
                    self.set_skiplist_prev(id, None);

                    // Send leave event(s) to the old focus
                    Input::keyboard_leave(self, prev);
                }
            }

            // set win to the surf focus
            self.set_surf_focus(win);
            // Send enter event(s) to the new focus
            // spec says this MUST be done after the leave events are sent
            Input::keyboard_enter(self, id);
        } else {
            // Otherwise we have unselected any surfaces, so clear both focus types
            self.set_win_focus(None);
            self.set_surf_focus(None);
        }

        // TODO: recalculate skip
    }

    pub fn add_new_top_subsurf(&mut self, parent: WindowId, win: WindowId) {
        log::debug!("Adding subsurface {:?} to {:?}", win, parent);
        self.set_parent_window(win, Some(parent));
        // Add ourselves to the top of the skiplist
        let old_top = self.get_top_child(parent);
        if let Some(top) = old_top {
            self.skiplist_place_above(win, top);
        }

        self.set_top_child(parent, Some(win));
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
    pub fn visible_subsurfaces(&'a self, id: WindowId) -> VisibleWindowIterator<'a> {
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
