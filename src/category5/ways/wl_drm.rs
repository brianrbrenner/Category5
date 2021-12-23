// Implementation of mesa's wl_drm
// interfaces for importing GPU buffers into
// vkcomp.
//
// https://wayland.app/protocols/wayland-drm#wl_drm
//
// Austin Shafer - 2021
extern crate wayland_server as ws;

use crate::category5::atmosphere::Atmosphere;
use std::cell::RefCell;
use std::rc::Rc;
use utils::log;
use ws::Main;

use nix::sys::stat::SFlag;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

use super::protocol::wl_drm::wl_drm;

/// In FreeBSD types.h:
///
/// ```
/// #define makedev(M, m)   __makedev((M), (m))
/// static __inline dev_t
/// __makedev(int _Major, int _Minor)
/// {
///     return (((dev_t)(_Major & 0xffffff00) << 32) | ((_Major & 0xff) << 8) |
///         ((dev_t)(_Minor & 0xff00) << 24) | (_Minor & 0xffff00ff));
/// }
/// ```
fn makedev(major: u64, minor: u64) -> libc::dev_t {
    (((major & 0xffffff00) as u64) << 32)
        | (((major & 0xff) as u64) << 8)
        | ((minor & 0xff00 as u64) << 24)
        | (minor & 0xffff00ff)
}

fn get_drm_dev_name(atmos: &Atmosphere) -> String {
    let (major, minor) = atmos.get_drm_dev();

    let mut dev_name = Vec::<c_char>::with_capacity(256); // Matching value of SPECNAMELEN in FreeBSD 13+

    let buf: *mut c_char = unsafe {
        libc::devname_r(
            makedev(major as u64, minor as u64),
            SFlag::S_IFCHR.bits(), // Must be S_IFCHR or S_IFBLK
            dev_name.as_mut_ptr(),
            dev_name.capacity() as c_int,
        )
    };

    // Buffer was too small (weird issue with the size of buffer) or the device could not be named.
    assert!(!buf.is_null());

    // SAFETY: The buffer written to by devname_r is guaranteed to be NUL terminated.
    let cstr = unsafe { CStr::from_ptr(buf) };
    format!("/dev/{}", cstr.to_string_lossy().into_owned())
}

pub fn wl_drm_setup(atmos_rc: Rc<RefCell<Atmosphere>>, wl_drm: Main<wl_drm::WlDrm>) {
    println!("LIBC DEV_T = {:?}", std::any::type_name::<libc::dev_t>());
    // Send the name of the DRM device reported by vkcomp
    let atmos = atmos_rc.borrow();
    let drm_name = get_drm_dev_name(&atmos);
    log::error!("DRM device returned by wl_drm is {}", drm_name);

    wl_drm.device(drm_name);
}

/// Ignores all requests. We only use this protocol to deliver
/// the drm name.
pub fn wl_drm_handle_request(req: wl_drm::Request, _wl_drm: Main<wl_drm::WlDrm>) {
    log::error!("Unimplemented wl_drm request {:?}", req);
}
