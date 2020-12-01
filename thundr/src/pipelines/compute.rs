// A compositor that uses compute kernels to blend windows
//
// Austin Shafer - 2020
#![allow(dead_code, non_camel_case_types)]
use serde::{Serialize, Deserialize};

use cgmath::{Vector3,Vector2,Matrix4};

use std::ffi::CString;
use std::io::Cursor;
use std::marker::Copy;
use std::mem;

use ash::version::DeviceV1_0;
use ash::{util,vk,Instance};

use crate::list::SurfaceList;
use crate::Surface;
use crate::renderer::{Renderer,RecordParams};
use super::Pipeline;
use crate::display::Display;

/// A compute pipeline
///
///
pub struct CompPipeline {
    /// A compute pipeline, which we will use to launch our shader
    cp_pipeline: vk::Pipeline,
    cp_pipeline_layout: vk::PipelineLayout,
    /// Our descriptor layout, specifying the format of data fed to the pipeline
    cp_descriptor_layout: vk::DescriptorSetLayout,
    /// The module for our compute shader
    cp_shader_modules: vk::ShaderModule,
    /// The pool that all descs in this struct are allocated from
    cp_desc_pool: vk::DescriptorPool,

    /// Our buffer containing our window locations
    cp_data: vk::Buffer,
    cp_data_mem: vk::DeviceMemory,

    /// The compute queue
    cp_queue: vk::Queue,
    /// Queue family index for `cp_queue`
    cp_queue_family: u32,
}

/// Our representation of window positions in the storage buffer
#[derive(Copy,Clone)]
struct StorageData {
    // TODO: implement me
    width: i32,
    height: i32,
}

impl CompPipeline {
    pub fn new(rend: &mut Renderer) -> Self {
        let layout = Self::create_descriptor_layout(rend);
        let pool = Self::create_descriptor_pool(rend);
        let descs = unsafe { rend.allocate_descriptor_sets(pool, &[layout]) };

        // create our data and a storage buffer
        let data = StorageData {
            width: rend.resolution.width as i32,
            height: rend.resolution.height as i32,
        };
        let (storage, storage_mem) = unsafe {
            rend.create_buffer_with_size(
                vk::BufferUsageFlags::STORAGE_BUFFER,
                vk::SharingMode::EXCLUSIVE,
                vk::MemoryPropertyFlags::DEVICE_LOCAL
                    | vk::MemoryPropertyFlags::HOST_VISIBLE
                    | vk::MemoryPropertyFlags::HOST_COHERENT,
                std::mem::size_of_val(&data) as u64,
            )
        };
        unsafe { rend.update_memory(storage_mem, &[data]); }

        // This is a really annoying issue with CString ptrs
        let program_entrypoint_name = CString::new("main").unwrap();
        // If the CString is created in `create_shaders`, and is inserted in
        // the return struct using the `.as_ptr()` method, then the CString
        // will still be dropped on return and our pointer will be garbage.
        // Instead we need to ensure that the CString will live long
        // enough. I have no idea why it is like this.
        let shader_stage = unsafe { CompPipeline::create_shader_stages(
            rend, program_entrypoint_name.as_ptr()
        )};

        let layouts = &[layout];
        let pipe_layout_info = vk::PipelineLayoutCreateInfo::builder()
            .set_layouts(layouts);
        let pipe_layout = unsafe {
            rend.dev.create_pipeline_layout(&pipe_layout_info, None)
                .unwrap()
        };

        let pipe_info = vk::ComputePipelineCreateInfo::builder()
            .stage(shader_stage)
            .layout(pipe_layout)
            .build();
        let pipeline = unsafe {
            rend.dev.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[pipe_info],
                None
            ).unwrap()[0]
        };

        let family = Self::get_queue_family(
            &rend.inst,
            &rend.display,
            rend.pdev,
        ).unwrap();
        let queue = unsafe { rend.dev.get_device_queue(family, 0) };

        CompPipeline {
            cp_pipeline: pipeline,
            cp_pipeline_layout: pipe_layout,
            cp_descriptor_layout: layout,
            cp_shader_modules: shader_stage.module,
            cp_desc_pool: pool,
            cp_data: storage,
            cp_data_mem: storage_mem,
            cp_queue: queue,
            cp_queue_family: family,
        }
    }

    /// Creates descriptor sets for our compute resources.
    /// For now this just includes a swapchain image to render things
    /// to, and a storage buffer.
    pub fn create_descriptor_layout(rend: &Renderer)
                                    -> vk::DescriptorSetLayout
    {
        let bindings = [
            // Our first binding is our destination image
            // this will be an image from the swapchain that we
            // are rendering into
            vk::DescriptorSetLayoutBinding::builder()
                .binding(0)
                .descriptor_type(
                    vk::DescriptorType::STORAGE_IMAGE)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .descriptor_count(1)
                .build(),
            // Our second descriptor will be the buffer containing
            // the pos/size of the windows
            vk::DescriptorSetLayoutBinding::builder()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .build(),
        ];
        let info = vk::DescriptorSetLayoutCreateInfo::builder()
            .bindings(&bindings);

        unsafe {
            rend.dev.create_descriptor_set_layout(&info, None)
                .unwrap()
        }
    }

    /// Create a descriptor pool to allocate from.
    /// The sizes in this must match `create_descriptor_layout`
    pub fn create_descriptor_pool(rend: &Renderer)
                                  -> vk::DescriptorPool
    {
        let size = [
            vk::DescriptorPoolSize::builder()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .build(),
            vk::DescriptorPoolSize::builder()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .build(),
        ];

        let info = vk::DescriptorPoolCreateInfo::builder()
            .pool_sizes(&size)
            .max_sets(1);

        unsafe {
            rend.dev.create_descriptor_pool(&info, None).unwrap()
        }
    }

    /// Get a queue family that this pipeline needs to support.
    /// This needs to be added to the renderer's `create_device`.
    pub fn get_queue_family(inst: &Instance,
                            display: &Display,
                            pdev: vk::PhysicalDevice)
                            -> Option<u32>
    {
        // get the properties per queue family
        Some(unsafe { Renderer::select_queue_family(
            inst, pdev, &display.surface_loader,
            display.surface,vk::QueueFlags::COMPUTE
        )})
            
    }

    /// Create the dynamic portions of the rendering pipeline
    ///
    /// Shader stages specify the usage of a shader module, such as the
    /// entrypoint name (usually main) and the type of shader. As of now,
    /// we only return two shader modules, vertex and fragment.
    ///
    /// `entrypoint`: should be a CString.as_ptr(). The CString that it
    /// represents should live as long as the return type of this method.
    ///  see: https://doc.rust-lang.org/std/ffi/struct.CString.html#method.as_ptr
    unsafe fn create_shader_stages(rend: &Renderer,
                                   entrypoint: *const i8)
                                   -> vk::PipelineShaderStageCreateInfo
    {
        let mut curse = Cursor::new(&include_bytes!("./shaders/comp.spv")[..]);
        let code = util::read_spv(&mut curse)
            .expect("Could not read spv file");

        let info = vk::ShaderModuleCreateInfo::builder()
            .code(&code);

        let shader = rend.dev.create_shader_module(&info, None)
            .expect("Could not create new shader module");

        vk::PipelineShaderStageCreateInfo {
            module: shader,
            p_name: entrypoint,
            stage: vk::ShaderStageFlags::COMPUTE,
            ..Default::default()
        }
    }
}

impl Pipeline for CompPipeline {
    fn is_ready(&self) -> bool { true }

    fn draw(&mut self,
            rend: &Renderer,
            params: &RecordParams,
            surfaces: &SurfaceList)
    {}

    fn destroy(&mut self, rend: &mut Renderer) {}
}