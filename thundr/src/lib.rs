//! # The Thundr rendering toolkit.
//!
//! Thundr is a Vulkan composition library for use in ui toolkits and
//! wayland compositors. You use it to create a set of images from
//! textures or window contents, attach those images to surfaces, and pass
//! a list of surfaces to thundr for rendering.
//!
//! Thundr also supports multiple methods of drawing:
//! * `geometric` - This is a more "traditional" manner of drawing ui elements:
//! surfaces are drawn as textured quads in 3D space.
//!
//! ## Drawing API
//!
//! The general flow of a thundr client is as follows:
//! * Create an Image (`create_image_from*`)
//!   * Use a MemImage to load a texture from raw bits.
//!   * Use a dmabuf to load a image contents from a gpu buffer.
//! * Create a Surface (`create_surface`)
//!   * Assign it a location and a size
//! * Create a surface list (`SurfaceList::new()`)
//!   * Push the surfaces you'd like rendered into the list from front to
//!   back (`SurfaceList.push`)
//! * Tell Thundr to launch the work on the gpu (`draw_frame`)
//! * Present the rendering results on screen (`present`)
//!
//! ```
//! use thundr as th;
//!
//! let thund: th::Thundr = Thundr::new();
//!
//! // First load our texture into memory
//! let img = image::open("images/cursor.png").unwrap().to_rgba();
//! let pixels: Vec<u8> = img.into_vec();
//! let mimg = MemImage::new(
//!     pixels.as_slice().as_ptr() as *mut u8,
//!     4,  // width of a pixel
//!     64, // width of texture
//!     64  // height of texture
//! );
//!
//! // Create an image from our MemImage
//! let image = thund.create_image_from_bits(&mimg, None).unwrap();
//! // Now create a 16x16 surface at position (0, 0)
//! let surf = thund.create_surface(0.0, 0.0, 16.0, 16.0);
//! ```
//! ## Requirements
//!
//! Thundr requires a system with vulkan 1.2+ installed. The following
//! extensions are used:
//! * VK_KHR_surface
//! * VK_KHR_display
//! * VK_EXT_maintenance2
//! * VK_KHR_debug_report
//! * VK_KHR_descriptor_indexing
//! * VK_KHR_external_memory

extern crate lazy_static;
extern crate lluvia;
use lluvia as ll;

// Austin Shafer - 2020
use std::marker::PhantomData;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};

mod damage;
mod descpool;
mod device;
mod display;
mod image;
mod instance;
mod pipelines;
mod platform;
mod renderer;
mod surface;

pub use self::image::Image;
pub use self::image::{Dmabuf, DmabufPlane};
pub use damage::Damage;
pub use device::Device;
use display::Display;
use instance::Instance;
pub use renderer::Renderer;
pub use surface::Surface;

use renderer::RecordParams;

// Re-export some things from utils so clients
// can use them
extern crate utils;
pub use crate::utils::region::Rect;
pub use crate::utils::{anyhow, Context, MemImage};

pub type Result<T> = std::result::Result<T, ThundrError>;

#[cfg(feature = "wayland")]
extern crate wayland_client as wc;

#[macro_use]
extern crate memoffset;
use pipelines::*;

extern crate thiserror;
use thiserror::Error;

/// Thundr error codes
/// These signify that action should be taken by the app.
#[derive(Error, Eq, PartialEq, Debug)]
#[allow(non_camel_case_types)]
pub enum ThundrError {
    #[error("Operation timed out")]
    TIMEOUT,
    #[error("Allocation failure")]
    OUT_OF_MEMORY,
    #[error("Operation is not ready, it needs to be redone")]
    NOT_READY,
    #[error("Failed to acquire the next swapchain image")]
    COULD_NOT_ACQUIRE_NEXT_IMAGE,
    #[error("vkQueuePresent failed")]
    PRESENT_FAILED,
    #[error("The internal Vulkan swapchain is out of date")]
    OUT_OF_DATE,
    #[error("Vulkan surface does not support R8G8B8A8_UNORM")]
    VK_SURF_NOT_SUPPORTED,
    #[error("Vulkan surface does not support the necessary (bindless) extensions")]
    VK_NOT_ALL_EXTENSIONS_AVAILABLE,
    #[error("Please select a composition type in the thundr CreateInfo")]
    COMPOSITION_TYPE_NOT_SPECIFIED,
    #[error("Vulkan surface or subsurface could not be found")]
    SURFACE_NOT_FOUND,
    #[error("Thundr Usage Bug: Recording already in progress")]
    RECORDING_ALREADY_IN_PROGRESS,
    #[error("Thundr Usage Bug: Recording has not been started")]
    RECORDING_NOT_IN_PROGRESS,
    #[error("Invalid Operation")]
    INVALID,
    #[error("Could not create the Vulkan swapchain")]
    COULD_NOT_CREATE_SWAPCHAIN,
    #[error("Failed to create Vulkan image")]
    COULD_NOT_CREATE_IMAGE,
    #[error("Invalid format or no format found")]
    INVALID_FORMAT,
}

pub struct Thundr {
    /// The vulkan Instance
    th_inst: Arc<Instance>,
    /// Our primary device
    th_dev: Arc<Device>,
    /// Our core rendering resources
    ///
    /// This holds the majority of the vulkan objects, and allows them
    /// to be accessed by things in our ECS so they can tear down their
    /// vulkan allocations
    th_rend: Arc<Mutex<Renderer>>,
    /// vk_khr_display and vk_khr_surface wrapper.
    th_display: Display,

    /// Application specific stuff that will be set up after
    /// the original initialization
    pub(crate) _th_pipe_type: PipelineType,
    pub(crate) th_pipe: Box<dyn Pipeline>,

    /// The current draw calls parameters
    th_params: Option<RecordParams>,

    /// We keep a list of all the images allocated by this context
    /// so that Pipeline::draw doesn't have to dedup the surfacelist's images
    pub th_image_ecs: ll::Instance,
}

/// A region to display to
///
/// The viewport will control what section of the screen is rendered
/// to. You will specify it when performing draw calls.
#[derive(Debug, Clone)]
pub struct Viewport {
    /// This is the position of the viewport on the output
    pub offset: (i32, i32),
    /// Size of the viewport within the output
    pub size: (i32, i32),
    /// The scrolling region of this viewport, basically the maximum bounds
    /// within which it is valid to update `scroll_offset`. This is similar to
    /// the panning region in X11.
    pub scroll_region: (i32, i32),
    /// This is the amount to offset everything within this viewport by. It
    /// can be used to move around all internal elements without updating
    /// them.
    ///
    /// This may be in the [0, scroll_region] range
    pub scroll_offset: (i32, i32),
}

impl Viewport {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            offset: (x, y),
            size: (width, height),
            scroll_region: (width, height),
            scroll_offset: (0, 0),
        }
    }

    /// Update the valid scrolling region within this viewport
    pub fn set_scroll_region(&mut self, x: i32, y: i32) {
        self.scroll_region = (x, y);
    }

    /// Set the scrolling within this viewport. This is a global transform
    ///
    /// This performs bounds checking of `dx` and `dy` to ensure the are within
    /// `scroll_region`. If they are not, then no scrolling is performed.
    pub fn update_scroll_amount(&mut self, dx: i32, dy: i32) {
        // The min and max bounds here are weird. Think of it like moving the
        // scroll region, not moving the scroll area. It looks like this:
        //
        // R: scroll region
        // A: scroll area
        //
        // Here they are at zero, content has just been loaded:
        //              0
        //              R--------------------R
        //              A-------------A
        //
        // Now here they are with the scroll all the way complete:
        //              0
        //       R--------------------R
        //              A-------------A
        //
        // The offset is actually from [-(R - A), 0]
        let min_x = -1 * (self.scroll_region.0 - self.size.0);
        let max_x = 0;
        // now get the new offset
        let x_offset = self.scroll_offset.0 - dx;
        // clamp this offset within our bounds
        let x_clamped = x_offset.clamp(min_x, max_x);

        let min_y = -1 * (self.scroll_region.1 - self.size.1);
        let max_y = 0;
        let y_offset = self.scroll_offset.1 - dy;
        let y_clamped = y_offset.clamp(min_y, max_y);

        self.scroll_offset = (x_clamped, y_clamped);
    }
}

#[cfg(feature = "sdl")]
extern crate sdl2;

pub enum SurfaceType<'a> {
    /// it exists to make the lifetime parameter play nice with rust.
    /// Since the Display variant doesn't have a lifetime, we need one that
    /// does incase xcb/macos aren't enabled.
    Display(PhantomData<&'a usize>),
    #[cfg(feature = "sdl")]
    SDL2(&'a sdl2::VideoSubsystem, &'a sdl2::video::Window),
    #[cfg(feature = "wayland")]
    Wayland(wc::Display, wc::protocol::wl_surface::WlSurface),
}

/// Parameters for Renderer creation.
///
/// These will be set by Thundr based on the Pipelines that will
/// be enabled. See `Pipeline` for methods that drive the data
/// contained here.
pub struct CreateInfo<'a> {
    /// Enable the traditional quad rendering method. This is a bindless
    /// engine that draws on a set of quads to composite images. This
    /// is the default and recommended option
    pub enable_traditional_composition: bool,
    pub surface_type: SurfaceType<'a>,
}

impl<'a> CreateInfo<'a> {
    pub fn builder() -> CreateInfoBuilder<'a> {
        CreateInfoBuilder {
            ci: CreateInfo {
                // This should always be used
                enable_traditional_composition: true,
                surface_type: SurfaceType::Display(PhantomData),
            },
        }
    }
}

/// Implements the builder pattern for easier thundr creation
pub struct CreateInfoBuilder<'a> {
    ci: CreateInfo<'a>,
}
impl<'a> CreateInfoBuilder<'a> {
    pub fn enable_traditional_composition(mut self) -> Self {
        self.ci.enable_traditional_composition = true;
        self
    }
    pub fn surface_type(mut self, ty: SurfaceType<'a>) -> Self {
        self.ci.surface_type = ty;
        self
    }

    pub fn build(self) -> CreateInfo<'a> {
        self.ci
    }
}

/// Droppable trait that matches anything.
///
/// From <https://doc.rust-lang.org/rustc/lints/listing/warn-by-default.html#dyn-drop>
///
/// To work around passing dyn Drop we specify a trait that can accept anything. That
/// way this boxed object can be dropped when the last rendering resource references
/// it.
pub trait Droppable {}
impl<T> Droppable for T {}

// This is the public facing thundr api. Don't change it
impl Thundr {
    // TODO: make get_available_params and add customization
    pub fn new(info: &CreateInfo) -> Result<Thundr> {
        // Create our own ECS for the image resources
        let mut img_ecs = ll::Instance::new();

        let inst = Arc::new(Instance::new(&info));
        let dev = Arc::new(Device::new(inst.clone(), &mut img_ecs, info)?);

        // creates a context, swapchain, images, and others
        // initialize the pipeline, renderpasses, and display engine
        let (mut rend, display) = Renderer::new(inst.clone(), dev.clone(), info, img_ecs.clone())?;

        // Create the pipeline(s) requested
        // Record the type we are using so that we know which type to regenerate
        // on window resizing
        let (pipe, ty): (Box<dyn Pipeline>, PipelineType) = if info.enable_traditional_composition {
            (
                Box::new(GeomPipeline::new(dev.clone(), &display, &mut rend)?),
                PipelineType::GEOMETRIC,
            )
        } else {
            return Err(ThundrError::COMPOSITION_TYPE_NOT_SPECIFIED);
        };

        Ok(Thundr {
            th_inst: inst,
            th_dev: dev,
            th_rend: Arc::new(Mutex::new(rend)),
            th_display: display,
            _th_pipe_type: ty,
            th_pipe: pipe,
            th_params: None,
            th_image_ecs: img_ecs,
        })
    }

    /// Get the Dots Per Inch for this display.
    ///
    /// For VK_KHR_display we will calculate it ourselves, and for
    /// SDL we will ask SDL to tell us it.
    pub fn get_dpi(&self) -> Result<(i32, i32)> {
        self.th_display.get_dpi()
    }

    pub fn get_resolution(&self) -> (u32, u32) {
        (
            self.th_display.d_state.d_resolution.width,
            self.th_display.d_state.d_resolution.height,
        )
    }

    /// Update an existing image from a shm buffer
    pub fn update_image_from_bits(
        &mut self,
        image: &Image,
        data: &[u8],
        width: u32,
        height: u32,
        stride: u32,
        damage: Option<Damage>,
        release: Option<Box<dyn Droppable + Send + Sync>>,
    ) {
        self.th_rend.lock().unwrap().wait_for_prev_submit();
        self.th_dev
            .update_image_from_bits(image, data, width, height, stride, damage, release);

        self.update_image_vk_info(image.i_internal.read().as_ref().unwrap());
    }

    // release_pending_resources
    pub fn release_pending_resources(&mut self) {
        self.th_rend.lock().unwrap().release_pending_resources();
    }

    /// This is a candidate for an out of date error. We should
    /// let the application know about this so it can recalculate anything
    /// that depends on the window size, so we exit returning OOD.
    ///
    /// We have to destroy and recreate our pipeline along the way since
    /// it depends on the swapchain.
    pub fn handle_ood(&mut self) -> Result<()> {
        self.th_display.recreate_swapchain()?;
        self.th_pipe.handle_ood(&mut self.th_display.d_state);

        Ok(())
    }

    pub fn get_drm_dev(&self) -> (i64, i64) {
        self.th_dev.get_drm_dev()
    }

    /// Begin recording a frame
    ///
    /// This is first called when trying to draw a frame. It will set
    /// up the command buffers and resources that Thundr will use while
    /// recording draw commands.
    pub fn begin_recording(&mut self) -> Result<()> {
        if self.th_params.is_some() {
            return Err(ThundrError::RECORDING_ALREADY_IN_PROGRESS);
        }

        // Get our next swapchain image
        match self.th_display.get_next_swapchain_image() {
            Ok(()) => (),
            Err(ThundrError::OUT_OF_DATE) => {
                self.handle_ood()?;
                return Err(ThundrError::OUT_OF_DATE);
            }
            Err(e) => return Err(e),
        };
        let mut rend = self.th_rend.lock().unwrap();

        // record rendering commands
        let mut params = rend.begin_recording_one_frame()?;
        let res = self.get_resolution();
        params.push.width = res.0;
        params.push.height = res.1;

        rend.refresh_window_resources();
        self.th_pipe.begin_record(&self.th_display.d_state);
        self.th_params = Some(params);

        Ok(())
    }

    /// Set the viewport
    ///
    /// This restricts the draw operations to within the specified region
    pub fn set_viewport(&mut self, viewport: &Viewport) -> Result<()> {
        let params = self
            .th_params
            .as_mut()
            .ok_or(ThundrError::RECORDING_NOT_IN_PROGRESS)?;

        self.th_pipe
            .set_viewport(params, &self.th_display.d_state, viewport)
    }

    /// Draw a set of surfaces within a viewport
    ///
    /// This is the function for recording drawing of a set of surfaces. The surfaces
    /// in the list will be rendered withing the region specified by viewport.
    pub fn draw_surface(&mut self, surface: &Surface) -> Result<()> {
        let params = self
            .th_params
            .as_mut()
            .ok_or(ThundrError::RECORDING_NOT_IN_PROGRESS)?;

        {
            let mut rend = self.th_rend.lock().unwrap();
            self.th_pipe
                .draw(rend.deref_mut(), params, &self.th_display.d_state, surface);
        }

        Ok(())
    }

    /// This finishes all recording operations and submits the work to the GPU.
    ///
    /// This should only be called after a proper begin_recording + draw_surfaces sequence.
    pub fn end_recording(&mut self) -> Result<()> {
        let rend = self.th_rend.lock().unwrap();
        rend.wait_for_prev_submit();
        self.th_pipe.end_record(&self.th_display.d_state);
        self.th_params = None;

        Ok(())
    }

    // present
    pub fn present(&mut self) -> Result<()> {
        self.th_display.present()
    }
}
