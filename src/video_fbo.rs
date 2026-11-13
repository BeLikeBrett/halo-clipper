// Offscreen framebuffer that mpv renders into. We then blit this FBO onto
// egui's framebuffer at whatever rect we want — so the video stays inside
// its allocated UI rectangle instead of mpv painting full-frame on top
// of the entire egui surface.
//
// The FBO is recreated whenever the target rect changes size.

use glow::HasContext;
use std::num::NonZeroU32;

pub struct VideoFbo {
    fbo: glow::NativeFramebuffer,
    tex: glow::NativeTexture,
    pub width: i32,
    pub height: i32,
}

impl VideoFbo {
    pub fn fbo_id_i32(&self) -> i32 {
        self.fbo.0.get() as i32
    }

    pub fn texture(&self) -> glow::NativeTexture {
        self.tex
    }

    pub fn fbo(&self) -> glow::NativeFramebuffer {
        self.fbo
    }

    pub fn ensure_size(slot: &mut Option<VideoFbo>, gl: &glow::Context, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        if let Some(existing) = slot {
            if existing.width == w && existing.height == h {
                return;
            }
            existing.destroy(gl);
        }
        unsafe {
            let Ok(tex) = gl.create_texture() else { return };
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w,
                h,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            let Ok(fbo) = gl.create_framebuffer() else {
                gl.delete_texture(tex);
                return;
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(tex);
                log::error!("video FBO incomplete: 0x{:x}", status);
                return;
            }

            *slot = Some(VideoFbo {
                fbo,
                tex,
                width: w,
                height: h,
            });
        }
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.tex);
        }
    }
}

pub fn fbo_from_raw(raw: i32) -> Option<glow::NativeFramebuffer> {
    NonZeroU32::new(raw as u32).map(glow::NativeFramebuffer)
}
