//! Images decoded in a worker thread: the pixels travel to the main
//! thread, which makes a texture of them.

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, glib};

pub struct Pixels {
    width: i32,
    height: i32,
    stride: usize,
    alpha: bool,
    bytes: glib::Bytes,
}

impl Pixels {
    pub fn new(pixbuf: &Pixbuf) -> Option<Self> {
        Some(Self {
            width: pixbuf.width(),
            height: pixbuf.height(),
            stride: usize::try_from(pixbuf.rowstride()).ok()?,
            alpha: pixbuf.has_alpha(),
            bytes: pixbuf.read_pixel_bytes(),
        })
    }

    pub fn texture(&self) -> gdk::Texture {
        let format = if self.alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
        gdk::MemoryTexture::new(self.width, self.height, format, &self.bytes, self.stride).upcast()
    }
}
