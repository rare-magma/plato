#![allow(unused)]

use std::mem;
use std::ptr;
use nix::{ioctl_write_ptr, ioctl_readwrite, ioctl_read_bad, ioctl_write_ptr_bad};

ioctl_read_bad!(read_variable_screen_info, FBIOGET_VSCREENINFO, VarScreenInfo);
ioctl_write_ptr_bad!(write_variable_screen_info, FBIOPUT_VSCREENINFO, VarScreenInfo);
ioctl_read_bad!(read_fixed_screen_info, FBIOGET_FSCREENINFO, FixScreenInfo);

pub const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
pub const FBIOPUT_VSCREENINFO: libc::c_ulong = 0x4601;
pub const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

ioctl_write_ptr!(send_update_v1, b'F', 0x2E, MxcfbUpdateDataV1);
ioctl_write_ptr!(send_update_v2, b'F', 0x2E, MxcfbUpdateDataV2);
ioctl_write_ptr!(wait_for_update_v1, b'F', 0x2F, u32);
ioctl_readwrite!(wait_for_update_v2, b'F', 0x2F, MxcfbUpdateMarkerData);

#[repr(C)]
#[derive(Clone, Debug)]
pub struct FixScreenInfo {
    pub id: [u8; 16],
    pub smem_start: usize,
    pub smem_len: u32,
    pub kind: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: u32,
    pub mmio_start: usize,
    pub mmio_len: u32,
    pub accel: u32,
    pub capabilities: u16,
    pub reserved: [u16; 2],
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct VarScreenInfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub red: Bitfield,
    pub green: Bitfield,
    pub blue: Bitfield,
    pub transp: Bitfield,
    pub nonstd: u32,
    pub activate: u32,
    pub height: u32,
    pub width: u32,
    pub accel_flags: u32,
    pub pixclock: u32,
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    pub sync: u32,
    pub vmode: u32,
    pub rotate: u32,
    pub colorspace: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct Bitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

impl Default for Bitfield {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl Default for VarScreenInfo {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl Default for FixScreenInfo {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct MxcfbRect {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for MxcfbRect {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct MxcfbAltBufferDataV1 {
    pub virt_addr: *const libc::c_void,
    pub phys_addr: u32,
    pub width: u32,
    pub height: u32,
    pub alt_update_region: MxcfbRect,
}

impl Default for MxcfbAltBufferDataV1 {
    fn default() -> Self {
        MxcfbAltBufferDataV1 {
            virt_addr: ptr::null(),
            phys_addr: 0,
            width: 0,
            height: 0,
            alt_update_region: MxcfbRect::default(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct MxcfbUpdateDataV1 {
    pub update_region: MxcfbRect,
    pub waveform_mode: u32,
    pub update_mode: u32,
    pub update_marker: u32,
    pub temp: libc::c_int,
    pub flags: libc::c_uint,
    pub alt_buffer_data: MxcfbAltBufferDataV1,
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct MxcfbUpdateMarkerData {
    pub update_marker: u32,
    pub collision_test: u32,
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct MxcfbAltBufferDataV2 {
    pub phys_addr: u32,
    pub width: u32,
    pub height: u32,
    pub alt_update_region: MxcfbRect,
}

impl Default for MxcfbAltBufferDataV2 {
    fn default() -> Self {
        MxcfbAltBufferDataV2 {
            phys_addr: 0,
            width: 0,
            height: 0,
            alt_update_region: MxcfbRect::default(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct MxcfbUpdateDataV2 {
    pub update_region: MxcfbRect,
    pub waveform_mode: u32,
    pub update_mode: u32,
    pub update_marker: u32,
    pub temp: libc::c_int,
    pub flags: libc::c_uint,
    pub dither_mode: libc::c_int,
    pub quant_bit: libc::c_int,
    pub alt_buffer_data: MxcfbAltBufferDataV2,
}

pub const WAVEFORM_MODE_AUTO: u32 = 0x101;

pub const NTX_WFM_MODE_INIT: u32  = 0;
pub const NTX_WFM_MODE_DU: u32    = 1;
pub const NTX_WFM_MODE_GC16: u32  = 2;
pub const NTX_WFM_MODE_GC4: u32   = 3;
pub const NTX_WFM_MODE_A2: u32    = 4;
pub const NTX_WFM_MODE_GL16: u32  = 5;
pub const NTX_WFM_MODE_GLR16: u32 = 6;
pub const NTX_WFM_MODE_GLD16: u32 = 7;

pub const UPDATE_MODE_PARTIAL: u32 = 0x0;
pub const UPDATE_MODE_FULL: u32    = 0x1;

pub const TEMP_USE_AMBIENT: libc::c_int = 0x1000;

pub const EPDC_FLAG_ENABLE_INVERSION: libc::c_uint = 0x01;
pub const EPDC_FLAG_FORCE_MONOCHROME: libc::c_uint = 0x02;

pub const EPDC_FLAG_TEST_COLLISION: libc::c_uint = 0x200;
pub const EPDC_FLAG_GROUP_UPDATE: libc::c_uint = 0x400;

pub const EPDC_FLAG_USE_AAD: libc::c_uint = 0x1000;
pub const EPDC_FLAG_USE_REGAL: libc::c_uint = 0x8000;

pub const EPDC_FLAG_USE_DITHERING_Y1: libc::c_uint = 0x2000;
pub const EPDC_FLAG_USE_DITHERING_Y4: libc::c_uint = 0x4000;
pub const EPDC_FLAG_USE_DITHERING_NTX_D8: libc::c_uint = 0x100000;

pub const EPDC_FLAG_USE_DITHERING_PASSTHROUGH: libc::c_int = 0;
// pub const EPDC_FLAG_USE_DITHERING_FLOYD_STEINBERG: libc::c_int = 1;
// pub const EPDC_FLAG_USE_DITHERING_ATKINSON: libc::c_int = 2;
pub const EPDC_FLAG_USE_DITHERING_ORDERED: libc::c_int = 3;
// pub const EPDC_FLAG_USE_DITHERING_QUANT_ONLY: libc::c_int = 4;
