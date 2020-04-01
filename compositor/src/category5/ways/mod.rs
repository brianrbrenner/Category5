// Wayland binding fun fun fun
//
//
// Austin Shafer - 2019

#![allow(dead_code, unused_variables, non_camel_case_types)]
#[macro_use]
pub mod utils;
#[allow(non_upper_case_globals)]
mod wayland_bindings;
#[macro_use]
mod wayland_safe;
pub mod compositor;
mod surface;

// Gets a private struct from a wl_resource
//
// wl_resources have a "user data" section which holds a private
// struct for us. This macro provides a safe and ergonomic way to grab
// that struct. The userdata will always have a container which holds
// our private struct, for now it is a RefCell. This macro "checks out"
// the private struct from its container to keep the borrow checker
// happy and our code safe.
//
// This macro uses unsafe code
//
// Example usage:
//      (get a reference to a `Surface` struct)
//  let mut surface = get_userdata!(resource, Surface).unwrap();
//
// Arguments:
//  resource: *mut wl_resource
//  generic: the type of private struct
//
// Returns:
//  Option holding the RefMut we can access the struct through
#[allow(unused_macros)]
#[macro_use]
macro_rules! get_userdata {
    // We need to know what type to use for the RefCell
    ($resource:ident, $generic:ty) => {
        unsafe {
            // use .as_mut to get an option<&> we can match against
            match (wl_resource_get_user_data($resource)
                   as *mut RefCell<$generic>).as_mut() {
                None => None,
                // Borrowing from the refcell will dynamically enforce
                // lifetime contracts. This can panic.
                Some(cell) => Some((*cell).borrow_mut()),
            }
        }
    }
}

pub mod task;
