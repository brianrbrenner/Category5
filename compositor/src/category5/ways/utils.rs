// Common functions for wayland code
//
// Austin Shafer - 2020
pub extern crate wayland_server as ws;
use ws::{Filter,Client};

use crate::category5::utils::{
    WindowId,
    timing::get_current_millis,
    atmosphere::Atmosphere
};
use crate::category5::utils::logging::LogLevel;
use crate::log;

use std::cell::RefCell;
use std::rc::Rc;

// Helper method for registering the property id of a client
//
// We need to make an id for the client for our entity component set in
// the atmosphere. This method should be used when creating globals, so
// we can register the new client with the atmos
//
// Returns the id created
pub fn register_new_client(atmos_cell: Rc<RefCell<Atmosphere>>, client: Client)
                           -> WindowId
{
    let id;
    {
        let mut atmos = atmos_cell.borrow_mut();
        // make a new client id
        id = atmos.mint_client_id();

        if !client.data_map().insert_if_missing(move || id) {
            log!(LogLevel::error, "registering a client that has already been registered");
        }

        // Track this surface in the compositor state
        atmos.add_window_id(id);
    }

    // when the client is destroyed we need to tell the atmosphere
    // to free the reserved space
    // TODO add destructor
    client.add_destructor(Filter::new(move |_, _, _| {
        atmos_cell.borrow_mut().free_window_id(id);
    }));

    return id;
}

// Grab the id belonging to this client
//
// The id is stored in the userdata map, which is kind of annoying to deal with
// we wrap it here so it can change easily
//
// If the client does not currently have an id, register it
pub fn get_id_from_client(atmos: Rc<RefCell<Atmosphere>>, client: Client)
                          -> WindowId
{
    match client.data_map().get::<WindowId>() {
        Some(id) => *id,
        // The client hasn't been assigned an id
        None => register_new_client(atmos, client),
    }
}
