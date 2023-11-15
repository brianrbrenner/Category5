// A set of helper structs for common operations
//
// Austin Shafer - 2020
pub mod timing;
#[macro_use]
pub mod logging;
pub mod fdwatch;
pub mod log;
pub mod region;

use std::ops::Deref;
use std::os::unix::io::OwnedFd;
use std::slice;

extern crate anyhow;
pub use anyhow::{anyhow, Context, Error, Result};

// Window Contents
//
// This allows for easy abstraction of the type
// of data being used to update a mesh.
#[allow(non_camel_case_types)]
pub enum WindowContents<'a> {
    dmabuf(&'a Dmabuf),
    mem_image(&'a MemImage),
}

// Represents a raw pointer to a region of memory
// containing an image buffer
//
// *Does Not* free the memory when it is dropped. This
// is used to represent shm buffers from wayland.
#[derive(Debug)]
pub struct MemImage {
    ptr: *const u8,
    // size of the pixel elements, in bytes
    pub element_size: usize,
    pub width: usize,
    pub height: usize,
    /// The number of pixels between the start of one row and the
    /// next. If no stride was specifid, this will default to 0,
    /// which is what vulkan uses to indicate pixels are tightly
    /// packed.
    pub stride: u32,
}

#[allow(dead_code)]
impl MemImage {
    pub fn as_slice(&self) -> &[u8] {
        if !self.ptr.is_null() {
            unsafe {
                return slice::from_raw_parts(
                    self.ptr,
                    self.width * self.height * self.element_size,
                );
            }
        } else {
            panic!("Trying to dereference null pointer");
        }
    }

    pub fn new(ptr: *const u8, element_size: usize, width: usize, height: usize) -> MemImage {
        MemImage {
            ptr: ptr,
            element_size: element_size,
            width: width,
            height: height,
            stride: 0,
        }
    }

    /// Sets the stride of this image to something besides the default 0
    pub fn set_stride(&mut self, stride: u32) {
        self.stride = stride;
    }

    /// Performs a simple checksum of adding all the pixels
    /// up in a gigantic int. Not perfect but should work for
    /// comparisons.
    pub fn checksum(&self) -> usize {
        let mut ret: usize = 0;

        for field in self.as_slice().iter() {
            ret += *field as usize;
        }

        ret
    }
}

// WARNING
// While it is safe according to the language, it is not actually
// safe to use. This is needed so that a MemImage can be sent from
// the wayland thread to the rendering thread. The rendering thread
// needs to consume this immediately. If the wl_buffer is released
// before this is consumed then things will become very bad.
unsafe impl Send for MemImage {}

impl Deref for MemImage {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        if !self.ptr.is_null() {
            return self.as_slice();
        } else {
            panic!("Trying to dereference null pointer");
        }
    }
}

// dmabuf from linux_dmabuf
// Represents one dma buffer the client has added.
// Will be referenced by Params during wl_buffer
// creation.
#[allow(dead_code)]
#[derive(Debug)]
pub struct Dmabuf {
    pub db_fd: OwnedFd,
    pub db_plane_idx: u32,
    pub db_offset: u32,
    pub db_stride: u32,
    // These will be added later during creation
    pub db_width: i32,
    pub db_height: i32,
    pub db_mods: u64,
}

impl Clone for Dmabuf {
    fn clone(&self) -> Self {
        Self {
            db_fd: self.db_fd.try_clone().expect("Could not DUP fd"),
            db_plane_idx: self.db_plane_idx,
            db_offset: self.db_offset,
            db_stride: self.db_stride,
            db_width: self.db_width,
            db_height: self.db_height,
            db_mods: self.db_mods,
        }
    }
}

impl Dmabuf {
    pub fn new(fd: OwnedFd, plane: u32, offset: u32, stride: u32, mods: u64) -> Dmabuf {
        Dmabuf {
            db_fd: fd,
            db_plane_idx: plane,
            db_offset: offset,
            db_stride: stride,
            // these will be added later during creation
            db_width: -1,
            db_height: -1,
            db_mods: mods,
        }
    }
}

/// Helper to perform max on PartialOrd types
///
/// We are using PartialOrd so that size and offset can handle
/// floating point types that do not support Ord
pub fn partial_max<T: PartialOrd>(a: T, b: T) -> T {
    if a >= b {
        return a;
    } else {
        return b;
    }
}

/// Helper to perform min on PartialOrd types
pub fn partial_min<T: PartialOrd>(a: T, b: T) -> T {
    if a <= b {
        return a;
    } else {
        return b;
    }
}
