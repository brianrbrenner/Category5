// Meshes represent a textured quad used to draw 2D
// graphics
//
// Austin Shafer - 2020
#![allow(dead_code, non_camel_case_types)]
extern crate nix;
extern crate ash;

use crate::log;
use crate::category5::utils::{
    timing::*, logging::LogLevel
};

use crate::category5::utils::*;
use super::*;
use std::iter;

use nix::Error;
use nix::errno::Errno;
use nix::unistd::dup;
use ash::version::{DeviceV1_0,InstanceV1_1};
use ash::vk;

// A single 3D object, stored in indexed vertex form
//
// All 3D objects should be stored as a set of vertices, which
// are combined into a mesh by selecting indices. This is typical stuff.
//
// meshes are created with Renderer::create_mesh. The renderer is in
// charge of creating/destroying the meshes since all of the mesh
// resources are created from the Renderer.
#[derive(Debug)]
pub struct Mesh {
    // image containing the contents of the window
    pub image: vk::Image,
    pub image_view: vk::ImageView,
    pub image_mem: vk::DeviceMemory,
    pub image_resolution: vk::Extent2D,
    pub pool_handle: usize,
    pub sampler_descriptors: Vec<vk::DescriptorSet>,
    // specific to the type of image
    m_priv: MeshPrivate,
}

// Private data specific to a mesh type.
//
// There are two types of meshes: memimages, and dmabufs
// MemImages represent shared memory that is copied
// and used as the mesh's texture
//
// Dmabufs are GPU buffers passed by fd. They will be
// imported (copyless) and bound to the mesh's image
#[derive(Debug)]
enum MeshPrivate {
    dmabuf(DmabufPrivate),
    mem_image(MemImagePrivate),
}

// Private data for shm images
#[derive(Debug)]
struct MemImagePrivate {
    // The staging buffer for copies to mesh.image
    transfer_buf: vk::Buffer,
    transfer_mem: vk::DeviceMemory,
}

// Private data for gpu buffers
#[derive(Debug)]
struct DmabufPrivate {
    // we need to cache the params to import memory with
    //
    // memory reqs for the mesh image
    dp_mem_reqs: vk::MemoryRequirements,
    // the type of memory to use
    dp_memtype_index: u32,
    // Stuff to release when we are no longer using
    // this gpu buffer (release the wl_buffer)
    dp_release_info: ReleaseInfo,
}

impl Mesh {
    // Create a mesh and its needed data
    //
    // All resources will be allocated by
    // rend
    pub fn new(rend: &mut Renderer,
               texture: WindowContents,
               release: ReleaseInfo)
               -> Option<Mesh>
    {
        match texture {
            WindowContents::mem_image(m) =>
                Mesh::from_mem_image(rend, m),
            WindowContents::dmabuf(d) =>
                Mesh::from_dmabuf(rend, d, release),
        }
    }

    // Create a new mesh from a shm buffer
    fn from_mem_image(rend: &mut Renderer,
                      img: &MemImage)
                      -> Option<Mesh>
    {
        unsafe {
            let tex_res = vk::Extent2D {
                width: img.width as u32,
                height: img.height as u32,
            };

            // The image is created with DEVICE_LOCAL memory types,
            // so we need to make a staging buffer to copy the data from.
            let (buffer, buf_mem) = rend.create_buffer(
                vk::BufferUsageFlags::TRANSFER_SRC,
                vk::SharingMode::EXCLUSIVE,
                vk::MemoryPropertyFlags::HOST_VISIBLE
                    | vk::MemoryPropertyFlags::HOST_COHERENT,
                img.as_slice(),
            );

            // This image will back the contents of the on-screen
            // client window.
            // TODO: this should eventually just use the image reported from
            // wayland.
            let (image, view, img_mem) = rend.create_image_with_contents(
                &tex_res,
                vk::Format::R8G8B8A8_SRGB,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_DST,
                vk::ImageAspectFlags::COLOR,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                buffer,
            );

            return Mesh::create_common(rend,
                                       MeshPrivate::mem_image(
                                           MemImagePrivate {
                                               transfer_buf: buffer,
                                               transfer_mem: buf_mem,
                                           }),
                                       &tex_res,
                                       image,
                                       img_mem,
                                       view,);
        }
    }

    // returns the index of the memory type to use
    // similar to Renderer::find_memory_type_index
    fn find_memtype_for_dmabuf(dmabuf_type_bits: u32,
                               props: &vk::PhysicalDeviceMemoryProperties,
                               reqs: &vk::MemoryRequirements)
                               -> Option<u32>
    {
        // and find the first type which matches our image
        for (i, ref mem_type) in props.memory_types.iter().enumerate() {
            // Bit i of memoryBitTypes will be set if the resource supports
            // the ith memory type in props.
            //
            // if this index is supported by dmabuf
            if (dmabuf_type_bits >> i) & 1 == 1
                // and by the image
                && (reqs.memory_type_bits >> i) & 1 == 1
                // make sure it is device local
                &&  mem_type.property_flags
                .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            {
                return Some(i as u32);
            }
        }

        return None;
    }

    // Create a new mesh from a dmabuf
    //
    // This is used during the first update of window
    // contents on an app. It will import the dmabuf
    // and create an image/view pair representing it.
    fn from_dmabuf(rend: &mut Renderer,
                   dmabuf: &Dmabuf,
                   release: ReleaseInfo)
                   -> Option<Mesh>
    {
        log!(LogLevel::profiling, "Creating mesh from dmabuf {:?}", dmabuf);
        // A lot of this is duplicated from Renderer::create_image
        unsafe {
            // According to the mesa source, this supports all modifiers
            let target_format = vk::Format::B8G8R8A8_SRGB;
            // get_physical_device_format_properties2
            let mut format_props = vk::FormatProperties2::builder()
                .build();
            let mut drm_fmt_props = vk::DrmFormatModifierPropertiesListEXT::builder()
                .build();
            format_props.p_next = &drm_fmt_props
                as *const _ as *mut std::ffi::c_void;

            // get the number of drm format mods props
            rend.inst.get_physical_device_format_properties2(
                rend.pdev, target_format, &mut format_props,
            );
            let mut mods: Vec<_> =
                iter::repeat(vk::DrmFormatModifierPropertiesEXT::default())
                .take(drm_fmt_props.drm_format_modifier_count as usize)
                .collect();

            drm_fmt_props.p_drm_format_modifier_properties = mods.as_mut_ptr();
            rend.inst.get_physical_device_format_properties2(
                rend.pdev, target_format, &mut format_props,
            );

            for m in mods.iter() {
                log!(LogLevel::debug, "dmabuf {} found mod {:#?}",
                     dmabuf.db_fd, m);
            }

            // the parameters to use for image creation
            let mut img_fmt_info = vk::PhysicalDeviceImageFormatInfo2::builder()
                .format(target_format)
                .ty(vk::ImageType::TYPE_2D)
                .usage(vk::ImageUsageFlags::SAMPLED)
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .build();
            let drm_img_props =
                vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::builder()
                .drm_format_modifier(dmabuf.db_mods)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .queue_family_indices(&[rend.graphics_family_index])
                .build();
            img_fmt_info.p_next = &drm_img_props
                as *const _ as *mut std::ffi::c_void;
            // the returned properties
            // the dimensions of the image will be returned here
            let mut img_fmt_props = vk::ImageFormatProperties2::builder()
                .build();
            rend.inst.get_physical_device_image_format_properties2(
                rend.pdev, &img_fmt_info, &mut img_fmt_props,
            ).unwrap();
            log!(LogLevel::debug, "dmabuf {} image format properties {:#?} {:#?}",
                 dmabuf.db_fd, img_fmt_props, drm_img_props);

            // we create the image now, but will have to bind
            // some memory to it later.
            let mut image_info = vk::ImageCreateInfo::builder()
                .image_type(vk::ImageType::TYPE_2D)
                // TODO: add other formats
                .format(target_format)
                .extent(vk::Extent3D {
                    width: dmabuf.db_width as u32,
                    height: dmabuf.db_height as u32,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                // we are only doing the linear format for now
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .usage(vk::ImageUsageFlags::SAMPLED)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let mut ext_mem_info = vk::ExternalMemoryImageCreateInfo::builder()
                .handle_types(vk::ExternalMemoryHandleTypeFlags
                              ::EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF)
                .build();
            // ???: Mesa doesn't use this one?
            // let drm_create_info =
            //     vk::ImageDrmFormatModifierExplicitCreateInfoEXT::builder()
            //     .drm_format_modifier(dmabuf.db_mods)
            //     .plane_layouts(&[
            //         vk::SubresourceLayout::builder()
            //             .size(
            //                 (dmabuf.db_stride * dmabuf.db_height as u32) as u64
            //             )
            //             .row_pitch(
            //                 dmabuf.db_stride as u64
            //                     - (dmabuf.db_width * 4) as u64
            //             )
            //             .build()
            //     ])
            //     .build();
            let drm_create_info =
                vk::ImageDrmFormatModifierListCreateInfoEXT::builder()
                .drm_format_modifiers(&[dmabuf.db_mods])
                .build();
            ext_mem_info.p_next = &drm_create_info
                as *const _ as *mut std::ffi::c_void;
            image_info.p_next = &ext_mem_info
                as *const _ as *mut std::ffi::c_void;
            let image = rend.dev.create_image(&image_info, None).unwrap();

            // we need to find a memory type that matches the type our
            // new image needs
            let mem_reqs = rend.dev.get_image_memory_requirements(image);
            let mem_props = Renderer::get_pdev_mem_properties(&rend.inst,
                                                              rend.pdev);
            // supported types we can import as
            let dmabuf_type_bits = rend.external_mem_fd_loader
                .get_memory_fd_properties_khr(
                    vk::ExternalMemoryHandleTypeFlags
                        ::EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF,
                    dmabuf.db_fd)
                .expect("Could not get memory fd properties")
                // bitmask set for each supported memory type
                .memory_type_bits;

            let memtype_index = Mesh::find_memtype_for_dmabuf(
                dmabuf_type_bits,
                &mem_props,
                &mem_reqs,
            ).expect("Could not find a memtype for the dmabuf");

            // use some of these to verify dmabuf imports:
            //
            // VkPhysicalDeviceExternalBufferInfo
            // VkPhysicalDeviceExternalImageInfo

            // This is where we differ from create_image
            //
            // We need to import from the dmabuf fd, so we will
            // add a VkImportMemoryFdInfoKHR struct to the next ptr
            // here to tell vulkan that we should import mem
            // instead of allocating it.
            let mut alloc_info = vk::MemoryAllocateInfo::builder()
                .allocation_size(mem_reqs.size)
                .memory_type_index(memtype_index);

            alloc_info.p_next = &vk::ImportMemoryFdInfoKHR::builder()
                .handle_type(vk::ExternalMemoryHandleTypeFlags
                             ::EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF)
                // need to dup the fd since it seems the implementation will
                // internally free it
                .fd(dup(dmabuf.db_fd).unwrap())
                as *const _ as *const std::ffi::c_void;

            // perform the import
            let image_memory = rend.dev.allocate_memory(&alloc_info, None)
                .unwrap();
            rend.dev.bind_image_memory(image, image_memory, 0)
                .expect("Unable to bind device memory to image");

            // finally make a view to wrap the image
            let view_info = vk::ImageViewCreateInfo::builder()
                .subresource_range(
                    vk::ImageSubresourceRange::builder()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1)
                        .build()
                )
                .image(image)
                .format(image_info.format)
                .view_type(vk::ImageViewType::TYPE_2D);

            let view = rend.dev.create_image_view(&view_info, None).unwrap();

            return Mesh::create_common(rend,
                                       MeshPrivate::dmabuf(DmabufPrivate {
                                           dp_mem_reqs: mem_reqs,
                                           dp_memtype_index: memtype_index,
                                           dp_release_info: release,
                                       }),
                                       &vk::Extent2D {
                                           width: dmabuf.db_width as u32,
                                           height: dmabuf.db_height as u32,
                                       },
                                       image,
                                       image_memory,
                                       view);
        }
    }

    // Create a mesh
    //
    // This logic is the same no matter what type of
    // resources the mesh was made from. It allocates
    // descriptors and constructs the mesh struct
    fn create_common(rend: &mut Renderer,
                     private: MeshPrivate,
                     res: &vk::Extent2D,
                     image: vk::Image,
                     image_mem: vk::DeviceMemory,
                     view: vk::ImageView)
                     -> Option<Mesh>
    {
        if let Some(ctx) = &mut *rend.app_ctx.borrow_mut() {
            // each mesh holds a set of descriptors that it will
            // bind before drawing itself. This set holds the
            // image sampler.
            //
            // right now they only hold an image sampler
            let (handle, descriptors) = ctx.desc_pool.allocate_samplers(
                &rend,
                rend.fb_count,
            );

            for i in 0..rend.fb_count {
                unsafe {
                    // bind the texture for our window
                    rend.update_sampler_descriptor_set(
                        descriptors[i],
                        1, //n binding
                        0, // element
                        ctx.image_sampler,
                        view,
                    );
                }
            }

            return Some(Mesh {
                image: image,
                image_view: view,
                image_mem: image_mem,
                image_resolution: *res,
                pool_handle: handle,
                sampler_descriptors: descriptors,
                m_priv: private,
            });
        }
        return None;
    }

    // Create a mesh and its needed data
    //
    // All resources will be allocated by
    // rend
    pub fn update_contents(&mut self,
                           rend: &mut Renderer,
                           data: WindowContents,
                           release: ReleaseInfo)
    {
        match data {
            WindowContents::mem_image(m) =>
                self.update_from_mem_image(rend, m),
            WindowContents::dmabuf(d) =>
                self.update_from_dmabuf(rend, d, release),
        };
    }

    // Update mesh contents from a shm buffer
    fn update_from_mem_image(&mut self,
                             rend: &mut Renderer,
                             img: &MemImage)
    {
        if let MeshPrivate::mem_image(mp) = &mut self.m_priv {
            unsafe {
                log!(LogLevel::debug,
                     "update_fr_mem_img: new img is {}x{} and image_resolution is {:?}",
                     img.width, img.height,
                     self.image_resolution);
                // resize the transfer mem if needed
                if img.width != self.image_resolution.width as usize
                    || img.height != self.image_resolution.height as usize
                {
                    // Out with the old TODO: make this a drop impl
                    rend.dev.destroy_buffer(mp.transfer_buf, None);
                    rend.free_memory(mp.transfer_mem);
                    // in with the new
                    let (buffer, buf_mem) = rend.create_buffer(
                        vk::BufferUsageFlags::TRANSFER_SRC,
                        vk::SharingMode::EXCLUSIVE,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                        img.as_slice(),
                    );
                    *mp = MemImagePrivate {
                        transfer_buf: buffer,
                        transfer_mem: buf_mem,
                    };

                    // update our mesh's resolution
                    self.image_resolution.width = img.width as u32;
                    self.image_resolution.height = img.height as u32;
                    rend.dev.free_memory(self.image_mem, None);
                    rend.dev.destroy_image_view(self.image_view, None);
                    rend.dev.destroy_image(self.image, None);
                    // we need to re-create & resize the image since we changed
                    // the resolution
                    let (image, view, img_mem) = rend.create_image_with_contents(
                        &vk::Extent2D {
                            width: img.width as u32,
                            height: img.height as u32,
                        },
                        vk::Format::R8G8B8A8_SRGB,
                        vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_DST,
                        vk::ImageAspectFlags::COLOR,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                        buffer,
                    );
                    self.image = image;
                    self.image_view = view;
                    self.image_mem = img_mem;
                } else {
                    // copy the data into the staging buffer
                    rend.update_memory(mp.transfer_mem,
                                       img.as_slice());
                    // copy the staging buffer into the image
                    rend.update_image_contents_from_buf(
                        mp.transfer_buf,
                        self.image,
                        self.image_resolution.width,
                        self.image_resolution.height,
                    );
                }
            }
        }
    }

    // Update mesh contents from a GPU buffer
    //
    // GPU buffers are passed as dmabuf fds, we will perform
    // an import using vulkan's external memory extensions
    fn update_from_dmabuf(&mut self,
                          rend: &mut Renderer,
                          dmabuf: &Dmabuf,
                          release: ReleaseInfo)
    {
        log!(LogLevel::profiling, "Updating mesh with dmabuf {:?}", dmabuf);
        if let MeshPrivate::dmabuf(dp) = &mut self.m_priv {
            // Since we are VERY async/threading friendly here, it is
            // possible that the fd may be bad since the program that
            // owns it was killed. If that is the case just return and
            // don't update the texture.
            let fd = match dup(dmabuf.db_fd) {
                Ok(f) => f,
                Err(Error::Sys(Errno::EBADF)) => return,
                Err(e) => {
                    log!(LogLevel::debug, "could not dup fd {:?}", e);
                    return;
                },
            };

            unsafe {
                // We need to update and rebind the memory
                // for image
                //
                // see from_dmabuf for a complete example
                let mut alloc_info = vk::MemoryAllocateInfo::builder()
                    .allocation_size(dp.dp_mem_reqs.size)
                    .memory_type_index(dp.dp_memtype_index)
                    .build();

                let import_info = vk::ImportMemoryFdInfoKHR::builder()
                    .handle_type(vk::ExternalMemoryHandleTypeFlags
                                 ::EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF)
                    // Need to dup the fd, since I think the implementation
                    // will internally free whatever we give it
                    .fd(fd)
                    .build();

                alloc_info.p_next = &import_info
                    as *const _ as *const std::ffi::c_void;

                // perform the import
                let image_memory = rend.dev.allocate_memory(&alloc_info,
                                                            None)
                    .unwrap();

                // Release the old frame's resources
                //
                // Free the old memory and replace it with the new one
                rend.free_memory(self.image_mem);
                self.image_mem = image_memory;

                // update the image header with the new import
                rend.dev.bind_image_memory(self.image, self.image_mem, 0)
                    .expect("Unable to rebind device memory to image");

                // the old release info will be implicitly dropped
                // after it has been drawn and presented
                let mut old_release = release;
                // swap our new release info into dp
                mem::swap(&mut dp.dp_release_info, &mut old_release);
                rend.register_for_release(old_release);
            }
        }
    }

    // A simple teardown function. The renderer is needed since
    // it allocated all these objects.
    pub fn destroy(&self, rend: &Renderer) {
        unsafe {
            rend.dev.destroy_image(self.image, None);
            rend.dev.destroy_image_view(self.image_view, None);
            rend.free_memory(self.image_mem);
            match &self.m_priv {
                // dma has nothing dynamic to free
                MeshPrivate::dmabuf(_) => {},
                MeshPrivate::mem_image(m) => {
                    rend.dev.destroy_buffer(m.transfer_buf, None);
                    rend.free_memory(m.transfer_mem);
                },
            }
            // get the descriptor pool
            if let Some(ctx) = &mut *rend.app_ctx.borrow_mut() {
                // free our descriptors
                ctx.desc_pool.destroy_samplers(rend,
                                               self.pool_handle,
                                               self.sampler_descriptors
                                               .as_slice());
            }
        }
    }

    // Generate draw calls for this mesh
    //
    // It is a very common operation to draw a mesh, this
    // helper draws itself at the locations passed by `push`
    //
    // First all descriptor sets and input assembly is bound
    // before the call to vkCmdDrawIndexed. The descriptor
    // sets should be updated whenever window contents are
    // changed, and then cbufs should be regenerated using this.
    //
    // Must be called while recording a cbuf
    pub fn record_draw(&self,
                       rend: &Renderer,
                       params: &RecordParams,
                       push: &PushConstants)
    {
        unsafe {
            if let Some(ctx) = &*rend.app_ctx.borrow() {
                // Descriptor sets can be updated elsewhere, but
                // they must be bound before drawing
                //
                // We need to bind both the uniform set, and the per-Mesh
                // set for the image sampler
                rend.dev.cmd_bind_descriptor_sets(
                    params.cbuf,
                    vk::PipelineBindPoint::GRAPHICS,
                    ctx.pipeline_layout,
                    0, // first set
                    &[
                        ctx.ubo_descriptor,
                        self.sampler_descriptors[params.image_num],
                    ],
                    &[], // dynamic offsets
                );

                // Set the z-ordering of the window we want to render
                // (this sets the visible window ordering)
                rend.dev.cmd_push_constants(
                    params.cbuf,
                    ctx.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX,
                    0, // offset
                    // get a &[u8] from our struct
                    // TODO: This should go. It is showing up as a noticeable
                    // hit in profiling. Idk if there is a safe way to
                    // replace it.
                    bincode::serialize(push).unwrap().as_slice(),
                );

                // Here is where everything is actually drawn
                // technically 3 vertices are being drawn
                // by the shader
                rend.dev.cmd_draw_indexed(
                    params.cbuf, // drawing command buffer
                    ctx.vert_count, // number of verts
                    1, // number of instances
                    0, // first vertex
                    0, // vertex offset
                    1, // first instance
                );
            }
        }
    }
}