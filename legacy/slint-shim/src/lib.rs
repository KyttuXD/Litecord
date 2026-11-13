//! See Cargo.toml: a few Slint type names, backed by plain memory.

use std::marker::PhantomData;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Rgb8Pixel {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Rgba8Pixel {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Bytes per pixel of a pixel type.
pub trait Pixel: Copy + Default {
    const BYTES: usize;
}
impl Pixel for Rgb8Pixel {
    const BYTES: usize = 3;
}
impl Pixel for Rgba8Pixel {
    const BYTES: usize = 4;
}

#[derive(Clone, Debug)]
pub struct SharedPixelBuffer<P> {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    pixel: PhantomData<P>,
}

impl<P: Pixel> SharedPixelBuffer<P> {
    pub fn new(width: u32, height: u32) -> Self {
        Self { bytes: vec![0; width as usize * height as usize * P::BYTES], width, height, pixel: PhantomData }
    }
    pub fn clone_from_slice(bytes: &[u8], width: u32, height: u32) -> Self {
        Self { bytes: bytes.to_vec(), width, height, pixel: PhantomData }
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn make_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

/// A picture as RGBA bytes.
#[derive(Clone, Debug, Default)]
pub struct Image {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Image {
    pub fn from_rgb8(buffer: SharedPixelBuffer<Rgb8Pixel>) -> Self {
        let mut rgba = Vec::with_capacity(buffer.bytes.len() / 3 * 4);
        for p in buffer.bytes.chunks_exact(3) {
            rgba.extend_from_slice(&[p[0], p[1], p[2], 0xFF]);
        }
        Self { rgba, width: buffer.width, height: buffer.height }
    }
    pub fn from_rgba8(buffer: SharedPixelBuffer<Rgba8Pixel>) -> Self {
        Self { rgba: buffer.bytes, width: buffer.width, height: buffer.height }
    }
}
