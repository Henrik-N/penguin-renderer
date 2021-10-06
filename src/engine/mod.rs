use std::borrow::Borrow;
use std::ops::{Deref, DerefMut};
use std::thread::current;
use ash::vk;
use anyhow::*;
use render_backend::RenderContext;
use log;
use crate::engine::pe::command_buffers::record_submit_command_buffer;
use crate::engine::render_backend::Core;


pub struct Renderer {
    core: Core,
    context: RenderContext,
    frame_num: usize,
}

impl Renderer {
    pub(crate) fn create(window: &winit::window::Window) -> Result<Self> {
        let core = Core::create(&window)?;
        let context = RenderContext::create(&core)?;


        Ok(Self {
            core,
            context,
            frame_num: 0,
        })
    }

    pub fn draw(&mut self, delta_time: f32) {
        self.frame_num += 1;

        self.context.submit_render_commands(|device, command_buffer, frame_buffer| {

            let flash = f32::abs(f32::sin(self.frame_num as f32 / 120_f32));
            log::debug!("Flash value: {}", flash);

            let color = [0.0_f32, 0.0_f32, flash, 1.0_f32];
            log::debug!("Color: {:?}", color);

            let clear_value = [vk::ClearValue {
                color: vk::ClearColorValue { float32: color }
            }];

            let render_pass_begin_info = vk::RenderPassBeginInfo::builder()
                .render_pass(self.context.render_pass)
                .framebuffer(frame_buffer)
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D {x: 0, y: 0},
                    extent: self.context.swapchain_extent,
                })
                .clear_values(&clear_value);

            unsafe {
                device.cmd_begin_render_pass(command_buffer, &render_pass_begin_info, vk::SubpassContents::INLINE);

                // actual render commands area


                device.cmd_end_render_pass(command_buffer);
            }

        });
    }


    pub fn shutdown(&mut self) {
        // this function call ensures the renderer doesn't get dropped until the event loop has ended
        log::debug!("Shutting down.")
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        log::debug!("Destroying renderer");
        self.context.shutdown();
        self.core.shutdown();
    }
}


struct Mesh;

struct Texture;

struct Material;

pub(crate) struct PRenderPass;

struct RenderGraph;

impl PRenderPass {
    pub fn create_default_render_pass(device: &ash::Device, swapchain_format: &vk::Format) -> (vk::RenderPass, usize) {
        // description of image for writing render commands into
        let render_pass_attachments = [
            // color attachment
            vk::AttachmentDescription::builder()
                .format(*swapchain_format)
                // 1 sample, no MSAA
                .samples(vk::SampleCountFlags::TYPE_1)
                // clear image on attachment load
                .load_op(vk::AttachmentLoadOp::CLEAR)
                // store image for being read later
                .store_op(vk::AttachmentStoreOp::STORE)
                // no stencil
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                // starting layout doesn't matter
                .initial_layout(vk::ImageLayout::UNDEFINED)
                // layout ready for display
                .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .build(),
        ];

        let color_attachment_ref = [vk::AttachmentReference::builder()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .build()]; // layout optimal to be written into by rendering commands


        let subpass = [vk::SubpassDescription::builder()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_attachment_ref)
            .build()];


        let render_pass_create_info = vk::RenderPassCreateInfo::builder()
            .attachments(&render_pass_attachments)
            .subpasses(&subpass);


        (unsafe {
            device.create_render_pass(&render_pass_create_info, None)
        }.expect("Couldn't create render pass!"),
         render_pass_attachments.len())
    }
}

mod pe;

mod render_backend {
    use ash::vk;
    use anyhow::*;
    use crate::core;
    use crate::core::logger;
    use crate::engine::{pe, PRenderPass};
    use pe::device::{SurfaceDetails};
    use pe::swapchain::PSwapchain;
    use pe::swapchain::SwapchainImage;
    use pe::pipeline::create_graphics_pipeline;
    use crate::engine::pe::command_buffers::record_submit_command_buffer;


    pub struct Core {
        debug_utils_loader: ash::extensions::ext::DebugUtils,
        debug_messenger: vk::DebugUtilsMessengerEXT,
        pub(super) instance: ash::Instance,
        pub(super) physical_device: vk::PhysicalDevice,
        pub(super) surface: vk::SurfaceKHR,
        pub(super) surface_loader: ash::extensions::khr::Surface,
    }

    impl Core {
        pub fn create(window: &winit::window::Window) -> Result<Self> {
            let entry = unsafe { ash::Entry::new() }?;
            let required_surface_extensions = ash_window::enumerate_required_extensions(window)?;

            log::trace!("Creating Vulkan instance.");
            let instance: ash::Instance = pe::instance::create_ash_instance(&entry, &required_surface_extensions)?;

            let (debug_utils_loader, debug_messenger) = {
                let (loader, messenger) = logger::init::init_vk_debug_messenger(&entry, &instance)?;

                if core::config::DEBUG.is_enabled {
                    log::trace!("Initializing vulkan debug messenger.");
                    (loader, messenger)
                } else {
                    log::trace!("Vulkan debug messenger disabled.");
                    (loader, vk::DebugUtilsMessengerEXT::null())
                }
            };


            let surface = unsafe { ash_window::create_surface(&entry, &instance, window, None)? };
            let surface_loader = ash::extensions::khr::Surface::new(&entry, &instance);

            log::trace!("Selecting physical device");
            let physical_device = pe::device::select_physical_device(&instance, surface, &surface_loader)?;

            Ok(Self {
                debug_utils_loader,
                debug_messenger,
                instance,
                physical_device,
                surface,
                surface_loader,
            })
        }

        pub fn shutdown(&mut self) {
            unsafe {
                log::debug!("Dropping core");

                self.surface_loader.destroy_surface(self.surface, None);

                if core::config::DEBUG.is_enabled {
                    self.debug_utils_loader.destroy_debug_utils_messenger(self.debug_messenger, None);
                }

                self.instance.destroy_instance(None);
            }
        }
    }

    /// Struct containing most Vulkan object handles and global states.
    pub(super) struct RenderContext {
        pub(super) device: ash::Device,
        pub(super) graphics_queue_index: u32,
        pub(super) queue_handle: vk::Queue,

        pub(super) swapchain_loader: ash::extensions::khr::Swapchain,
        pub(super) swapchain: vk::SwapchainKHR,
        pub(super) swapchain_format: vk::Format,
        pub(super) swapchain_extent: vk::Extent2D,
        pub(super) swapchain_images: Vec<vk::Image>,
        pub(super) swapchain_image_views: Vec<vk::ImageView>,

        pub(super) command_pool: vk::CommandPool,
        pub(super) command_buffer: vk::CommandBuffer,

        pub(super) render_fence: vk::Fence,
        pub(super) rendering_complete_semaphore: vk::Semaphore,
        pub(super) presenting_complete_semaphore: vk::Semaphore,

        pub(super) render_pass: vk::RenderPass,
        pub(super) frame_buffers: Vec<vk::Framebuffer>,
    }

    impl RenderContext {

        pub fn submit_render_commands<RenderPassFn: FnOnce(&ash::Device, vk::CommandBuffer, vk::Framebuffer)>(&self,
        render_pass_fn: RenderPassFn
        ) {

            let wait_semaphores = [self.presenting_complete_semaphore];

            // request new image from swapchain
            let (image_index, _is_suboptimal) = unsafe {
                log::trace!("Aquiring next swapchain image");
                // timeout 1 sec, specified in nanoseconds
                self.swapchain_loader.acquire_next_image(self.swapchain, 1000000000, wait_semaphores[0], vk::Fence::null())
            }.expect("Couldn't acquire next swapchain image");
            log::trace!("Swapchain image {} aquired!", image_index);

            let frame_buffer = self.frame_buffers.get(image_index as usize).expect("Couldn't get frame buffer at the given index");

            record_submit_command_buffer(
                &self.device,
                self.command_buffer,
                self.render_fence,
                self.queue_handle,
                &[vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT],
                &wait_semaphores,
                &[self.rendering_complete_semaphore],
                self.swapchain,
                &self.swapchain_loader,
                *frame_buffer,
                render_pass_fn);

            // after commands are submitted, wait for rending to complete and then display the image to the screen
            let swapchains = [self.swapchain];
            let wait_semaphores = [self.rendering_complete_semaphore];
            let image_indices = [image_index];
            let present_info = vk::PresentInfoKHR::builder()
                .swapchains(&swapchains)
                .wait_semaphores(&wait_semaphores)
                .image_indices(&image_indices);

            unsafe {
                self.swapchain_loader.queue_present(self.queue_handle, &present_info).expect("Couldn't submit to present queue");
            }

        }


        pub fn create(core: &Core) -> Result<Self> {
            log::trace!("Finding queue family indices");
            let queue_index = pe::device::find_graphics_queue_family(&core.instance, core.physical_device, core.surface, &core.surface_loader);

            let queue_index = match queue_index {
                Some(queue) => queue,
                None => {
                    log::error!("Graphics queue index was None");
                    panic!("Graphics queue index was None");
                }
            };


            log::trace!("Creating logical device");
            let device = pe::device::create_logical_device(&core.instance, core.physical_device, queue_index);

            log::trace!("Getting graphics queue handle");
            let queue_handle: vk::Queue = pe::device::get_graphics_queue_handle(&device, queue_index);


            log::trace!("Quering device for swapchain support");
            let swapchain_support_details = pe::device::query_swapchain_support(core.physical_device, core.surface, &core.surface_loader);

            log::trace!("Creating swapchain");
            let swapchain = pe::swapchain::PSwapchain::create(&core.instance, &device, core.surface, swapchain_support_details, queue_index);

            let pe::swapchain::PSwapchain {
                swapchain_loader,
                swapchain,
                swapchain_format,
                swapchain_extent,
                swapchain_images,
                swapchain_image_views,
            } = swapchain;


            let (command_pool, setup_command_buffer) = pe::command_buffers::init::create_command_pool_and_buffer(&device, queue_index);

            // fences ---------
            let render_fence_create_info = vk::FenceCreateInfo::builder()
                .flags(vk::FenceCreateFlags::SIGNALED); // start signaled, to wait for it before the first gpu command

            let render_fence = unsafe { device.create_fence(&render_fence_create_info, None) }.expect("Failed to create render fence.");

            // semaphores --------------

            let semaphore_create_info = vk::SemaphoreCreateInfo::default();

            let rendering_complete_semaphore = unsafe { device.create_semaphore(&semaphore_create_info, None) }.expect("Failed to create semaphore");
            let presenting_complete_semaphore = unsafe { device.create_semaphore(&semaphore_create_info, None) }.expect("Failed to create semaphore");

            // render pass --------------

            let (render_pass, _attachment_count) = PRenderPass::create_default_render_pass(&device, &swapchain_format);

            // frame buffers --------------
            let frame_buffers: Vec<vk::Framebuffer> = swapchain_image_views.iter().map(|&image| {
                let attachments = [image];
                let create_info = vk::FramebufferCreateInfo::builder()
                    .render_pass(render_pass)
                    .attachments(&attachments)
                    .width(swapchain_extent.width)
                    .height(swapchain_extent.height)
                    .layers(1);

                unsafe {
                    device.create_framebuffer(&create_info, None)
                }.expect("Couldn't create framebuffer")
            }).collect();


            Ok(Self {
                device,
                graphics_queue_index: queue_index,
                queue_handle,
                swapchain,
                swapchain_loader,
                swapchain_format,
                swapchain_extent,
                swapchain_images,
                swapchain_image_views,
                command_pool,
                command_buffer: setup_command_buffer,
                rendering_complete_semaphore,
                presenting_complete_semaphore,
                render_fence,
                render_pass,
                frame_buffers,
            })
        }

        pub fn update(&mut self, delta: f32) {}

        pub fn render(&mut self, delta: f32) {}

        pub fn shutdown(&mut self) {
            unsafe {
                log::debug!("Dropping render context");
                self.device.wait_for_fences(&[self.render_fence], true, u64::MAX);


                self.frame_buffers.iter().for_each(|&frame_buffer| {
                    self.device.destroy_framebuffer(frame_buffer, None);
                });

                self.device.destroy_render_pass(self.render_pass, None);


                self.device.destroy_semaphore(self.presenting_complete_semaphore, None);
                self.device.destroy_semaphore(self.rendering_complete_semaphore, None);
                self.device.destroy_fence(self.render_fence, None);


                self.swapchain_image_views.iter().for_each(|&image_view| {
                    self.device.destroy_image_view(image_view, None);
                });


                self.device.destroy_command_pool(self.command_pool, None);

                self.swapchain_loader.destroy_swapchain(self.swapchain, None);

                self.device.destroy_device(None);
            }
        }
    }
}