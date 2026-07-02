#[cfg(target_os = "linux")]
use std::{
	os::fd::{AsRawFd, IntoRawFd, OwnedFd},
	sync::Arc,
};

#[cfg(target_os = "linux")]
use ash::vk;
#[cfg(target_os = "linux")]
use germinal_ports::error::BoxResult;
#[cfg(target_os = "linux")]
use nix::{sys::stat::fstat, unistd::dup};
#[cfg(target_os = "linux")]
use thiserror::Error;

#[cfg(target_os = "linux")]
use crate::rendering::pty_surface::video_surface_frame::{
	WgpuVideoSurfaceDmaBufPlane, WgpuVideoSurfaceNv12DmaBufFrame, WgpuVideoSurfaceNv12GpuFrame,
};

#[cfg(target_os = "linux")]
#[derive(Debug, Error)]
enum VideoSurfaceImportError {
	#[error("dma_buf import requires a non-empty frame")]
	EmptyFrame,
	#[error("nv12 dma_buf import requires even width and height, got {width_px}x{height_px}")]
	OddDimensions { width_px: u32, height_px: u32 },
	#[error("dma_buf import requires the Vulkan backend")]
	RequiresVulkanBackend,
	#[error("no compatible Vulkan memory type for dma_buf import")]
	NoCompatibleMemoryType,
	#[error("dma_buf size must be non-negative")]
	NegativeDmaBufSize,
}

#[cfg(target_os = "linux")]
pub fn import_nv12_dmabuf_frame(
	device: &wgpu::Device,
	frame: &WgpuVideoSurfaceNv12DmaBufFrame,
) -> BoxResult<WgpuVideoSurfaceNv12GpuFrame> {
	if frame.width_px == 0 || frame.height_px == 0 {
		return Err(VideoSurfaceImportError::EmptyFrame.into());
	}
	if frame.width_px % 2 != 0 || frame.height_px % 2 != 0 {
		return Err(
			VideoSurfaceImportError::OddDimensions {
				width_px:  frame.width_px,
				height_px: frame.height_px,
			}
			.into(),
		);
	}

	let Some(hal_device) = (unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }) else {
		return Err(VideoSurfaceImportError::RequiresVulkanBackend.into());
	};

	let raw_device = hal_device.raw_device().clone();
	let raw_instance = hal_device.shared_instance().raw_instance().clone();
	let raw_physical_device = hal_device.raw_physical_device();
	let external_memory_fd = ash::khr::external_memory_fd::Device::new(&raw_instance, &raw_device);

	let y_texture = Arc::new(import_plane_texture(
		device,
		&raw_device,
		&raw_instance,
		raw_physical_device,
		&external_memory_fd,
		PlaneImportRequest {
			label:                Some("germinal.video_surface.nv12.y"),
			width_px:             frame.width_px,
			height_px:            frame.height_px,
			format:               wgpu::TextureFormat::R8Unorm,
			vk_format:            vk::Format::R8_UNORM,
			plane:                &frame.y_plane,
			estimated_plane_size: estimated_plane_size_bytes(frame.y_plane.stride, frame.height_px),
		},
	)?);
	let uv_texture = Arc::new(import_plane_texture(
		device,
		&raw_device,
		&raw_instance,
		raw_physical_device,
		&external_memory_fd,
		PlaneImportRequest {
			label:                Some("germinal.video_surface.nv12.uv"),
			width_px:             frame.width_px / 2,
			height_px:            frame.height_px / 2,
			format:               wgpu::TextureFormat::Rg8Unorm,
			vk_format:            vk::Format::R8G8_UNORM,
			plane:                &frame.uv_plane,
			estimated_plane_size: estimated_plane_size_bytes(frame.uv_plane.stride, frame.height_px / 2),
		},
	)?);

	let y_plane = y_texture.create_view(&wgpu::TextureViewDescriptor::default());
	let uv_plane = uv_texture.create_view(&wgpu::TextureViewDescriptor::default());
	let plane_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
		label: Some("germinal.video_surface.nv12.sampler"),
		address_mode_u: wgpu::AddressMode::ClampToEdge,
		address_mode_v: wgpu::AddressMode::ClampToEdge,
		address_mode_w: wgpu::AddressMode::ClampToEdge,
		mag_filter: wgpu::FilterMode::Linear,
		min_filter: wgpu::FilterMode::Linear,
		mipmap_filter: wgpu::MipmapFilterMode::Nearest,
		..Default::default()
	});

	Ok(WgpuVideoSurfaceNv12GpuFrame::new(
		frame.width_px,
		frame.height_px,
		frame.color_profile,
		y_texture,
		y_plane,
		uv_texture,
		uv_plane,
		plane_sampler,
	))
}

#[cfg(target_os = "linux")]
struct PlaneImportRequest<'a> {
	label:                Option<&'a str>,
	width_px:             u32,
	height_px:            u32,
	format:               wgpu::TextureFormat,
	vk_format:            vk::Format,
	plane:                &'a WgpuVideoSurfaceDmaBufPlane,
	estimated_plane_size: u64,
}

#[cfg(target_os = "linux")]
fn import_plane_texture(
	device: &wgpu::Device,
	raw_device: &ash::Device,
	raw_instance: &ash::Instance,
	raw_physical_device: vk::PhysicalDevice,
	external_memory_fd: &ash::khr::external_memory_fd::Device,
	request: PlaneImportRequest<'_>,
) -> BoxResult<wgpu::Texture> {
	let duplicated_fd = dup(&*request.plane.fd)?;

	let mut memory_fd_properties = vk::MemoryFdPropertiesKHR::default();
	unsafe {
		external_memory_fd.get_memory_fd_properties(
			vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
			duplicated_fd.as_raw_fd(),
			&mut memory_fd_properties,
		)?
	};

	let mut external_memory_image_info = vk::ExternalMemoryImageCreateInfo::default()
		.handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
	let plane_layouts = [subresource_layout_of(request.plane, request.estimated_plane_size)];
	let mut drm_format_modifier_info = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
		.drm_format_modifier(request.plane.modifier)
		.plane_layouts(&plane_layouts);
	let image_create_info = vk::ImageCreateInfo::default()
		.image_type(vk::ImageType::TYPE_2D)
		.format(request.vk_format)
		.extent(vk::Extent3D { width: request.width_px, height: request.height_px, depth: 1 })
		.mip_levels(1)
		.array_layers(1)
		.samples(vk::SampleCountFlags::TYPE_1)
		.tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
		.usage(vk::ImageUsageFlags::SAMPLED)
		.sharing_mode(vk::SharingMode::EXCLUSIVE)
		.initial_layout(vk::ImageLayout::UNDEFINED)
		.push_next(&mut drm_format_modifier_info)
		.push_next(&mut external_memory_image_info);
	let vk_image = unsafe { raw_device.create_image(&image_create_info, None)? };

	let image_requirements = unsafe { raw_device.get_image_memory_requirements(vk_image) };
	let memory_type_bits =
		image_requirements.memory_type_bits & memory_fd_properties.memory_type_bits;
	let memory_type_index = find_memory_type_index(
		raw_instance,
		raw_physical_device,
		memory_type_bits,
		vk::MemoryPropertyFlags::DEVICE_LOCAL,
	)
	.or_else(|| {
		find_memory_type_index(
			raw_instance,
			raw_physical_device,
			memory_type_bits,
			vk::MemoryPropertyFlags::empty(),
		)
	})
	.ok_or(VideoSurfaceImportError::NoCompatibleMemoryType)?;

	let dmabuf_size = dma_buf_size_bytes(&request.plane.fd)?;
	let allocation_size = image_requirements
		.size
		.max(dmabuf_size)
		.max(request.plane.offset.saturating_add(request.estimated_plane_size));
	let mut import_memory_info = vk::ImportMemoryFdInfoKHR::default()
		.handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
		.fd(dup(&*request.plane.fd)?.into_raw_fd());
	let memory_allocate_info = vk::MemoryAllocateInfo::default()
		.allocation_size(allocation_size)
		.memory_type_index(memory_type_index)
		.push_next(&mut import_memory_info);
	let vk_memory = unsafe { raw_device.allocate_memory(&memory_allocate_info, None)? };

	if let Err(error) = unsafe { raw_device.bind_image_memory(vk_image, vk_memory, 0) } {
		unsafe {
			raw_device.free_memory(vk_memory, None);
			raw_device.destroy_image(vk_image, None);
		}
		return Err(error.into());
	}

	let raw_device_for_drop = raw_device.clone();
	let drop_callback: wgpu::hal::DropCallback = Box::new(move || unsafe {
		raw_device_for_drop.destroy_image(vk_image, None);
		raw_device_for_drop.free_memory(vk_memory, None);
	});

	let hal_texture = unsafe {
		let Some(hal_device) = device.as_hal::<wgpu::hal::api::Vulkan>() else {
			return Err(VideoSurfaceImportError::RequiresVulkanBackend.into());
		};
		hal_device.texture_from_raw(
			vk_image,
			&hal_texture_descriptor(request.label, request.width_px, request.height_px, request.format),
			Some(drop_callback),
			wgpu::hal::vulkan::TextureMemory::External,
		)
	};

	Ok(unsafe {
		device.create_texture_from_hal::<wgpu::hal::api::Vulkan>(
			hal_texture,
			&wgpu_texture_descriptor(request.label, request.width_px, request.height_px, request.format),
		)
	})
}

#[cfg(target_os = "linux")]
fn subresource_layout_of(plane: &WgpuVideoSurfaceDmaBufPlane, size: u64) -> vk::SubresourceLayout {
	vk::SubresourceLayout::default()
		.offset(plane.offset)
		.size(size)
		.row_pitch(u64::from(plane.stride))
		.array_pitch(size)
		.depth_pitch(size)
}

#[cfg(target_os = "linux")]
fn hal_texture_descriptor<'a>(
	label: Option<&'a str>,
	width_px: u32,
	height_px: u32,
	format: wgpu::TextureFormat,
) -> wgpu::hal::TextureDescriptor<'a> {
	wgpu::hal::TextureDescriptor {
		label,
		size: wgpu::Extent3d {
			width:                 width_px,
			height:                height_px,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format,
		usage: wgpu::TextureUses::RESOURCE,
		memory_flags: wgpu::hal::MemoryFlags::empty(),
		view_formats: vec![],
	}
}

#[cfg(target_os = "linux")]
fn wgpu_texture_descriptor<'a>(
	label: Option<&'a str>,
	width_px: u32,
	height_px: u32,
	format: wgpu::TextureFormat,
) -> wgpu::TextureDescriptor<'a> {
	wgpu::TextureDescriptor {
		label,
		size: wgpu::Extent3d {
			width:                 width_px,
			height:                height_px,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format,
		usage: wgpu::TextureUsages::TEXTURE_BINDING,
		view_formats: &[],
	}
}

fn dma_buf_size_bytes(fd: &Arc<OwnedFd>) -> BoxResult<u64> {
	let stat = fstat(&**fd)?;
	u64::try_from(stat.st_size).map_err(|_| VideoSurfaceImportError::NegativeDmaBufSize.into())
}

#[cfg(target_os = "linux")]
fn estimated_plane_size_bytes(stride: u32, height_px: u32) -> u64 {
	u64::from(stride).saturating_mul(u64::from(height_px))
}

#[cfg(target_os = "linux")]
fn find_memory_type_index(
	raw_instance: &ash::Instance,
	raw_physical_device: vk::PhysicalDevice,
	type_bits_req: u32,
	flags_req: vk::MemoryPropertyFlags,
) -> Option<u32> {
	let memory_properties =
		unsafe { raw_instance.get_physical_device_memory_properties(raw_physical_device) };
	for (index, memory_type) in memory_properties.memory_types_as_slice().iter().enumerate() {
		let type_bit = 1u32 << index;
		if type_bits_req & type_bit == 0 {
			continue;
		}
		if memory_type.property_flags & flags_req == flags_req {
			return Some(index as u32);
		}
	}
	None
}
