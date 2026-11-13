//! libmpv requires standard GL defaults on entry. Isolate each render context
//! from egui and from the other video layers, then restore the caller's state.
use glow::HasContext;
use std::num::NonZeroU32;
const PIXEL_STORE: [u32; 8] = [
    glow::UNPACK_ALIGNMENT,
    glow::UNPACK_ROW_LENGTH,
    glow::UNPACK_SKIP_PIXELS,
    glow::UNPACK_SKIP_ROWS,
    glow::PACK_ALIGNMENT,
    glow::PACK_ROW_LENGTH,
    glow::PACK_SKIP_PIXELS,
    glow::PACK_SKIP_ROWS,
];
const ENABLES: [u32; 7] = [
    glow::BLEND,
    glow::DEPTH_TEST,
    glow::CULL_FACE,
    glow::STENCIL_TEST,
    glow::SCISSOR_TEST,
    glow::FRAMEBUFFER_SRGB,
    glow::DITHER,
];
pub struct Guard<'a> {
    gl: &'a glow::Context,
    draw: i32,
    read: i32,
    program: i32,
    vao: i32,
    array: i32,
    pack: i32,
    unpack: i32,
    active: i32,
    texture: i32,
    sampler: i32,
    viewport: [i32; 4],
    scissor: [i32; 4],
    mask: [i32; 4],
    blend: [i32; 6],
    pixels: [i32; 8],
    enabled: [bool; 7],
}
impl<'a> Guard<'a> {
    pub unsafe fn capture(gl: &'a glow::Context) -> Self {
        let active = gl.get_parameter_i32(glow::ACTIVE_TEXTURE);
        gl.active_texture(glow::TEXTURE0);
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        let mut scissor = [0; 4];
        gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor);
        let mut mask = [0; 4];
        gl.get_parameter_i32_slice(glow::COLOR_WRITEMASK, &mut mask);
        let guard = Self {
            gl,
            draw: gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING),
            read: gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING),
            program: gl.get_parameter_i32(glow::CURRENT_PROGRAM),
            vao: gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING),
            array: gl.get_parameter_i32(glow::ARRAY_BUFFER_BINDING),
            pack: gl.get_parameter_i32(glow::PIXEL_PACK_BUFFER_BINDING),
            unpack: gl.get_parameter_i32(glow::PIXEL_UNPACK_BUFFER_BINDING),
            active,
            texture: gl.get_parameter_i32(glow::TEXTURE_BINDING_2D),
            sampler: gl.get_parameter_i32(glow::SAMPLER_BINDING),
            viewport,
            scissor,
            mask,
            blend: [
                gl.get_parameter_i32(glow::BLEND_SRC_RGB),
                gl.get_parameter_i32(glow::BLEND_DST_RGB),
                gl.get_parameter_i32(glow::BLEND_SRC_ALPHA),
                gl.get_parameter_i32(glow::BLEND_DST_ALPHA),
                gl.get_parameter_i32(glow::BLEND_EQUATION_RGB),
                gl.get_parameter_i32(glow::BLEND_EQUATION_ALPHA),
            ],
            pixels: PIXEL_STORE.map(|p| gl.get_parameter_i32(p)),
            enabled: ENABLES.map(|p| gl.is_enabled(p)),
        };
        gl.active_texture(active as u32);
        guard
    }
    pub unsafe fn new(gl: &'a glow::Context) -> Self {
        let guard = Self::capture(gl);
        gl.active_texture(glow::TEXTURE0);
        gl.use_program(None);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_sampler(0, None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        for flag in ENABLES {
            gl.disable(flag);
        }
        for flag in PIXEL_STORE {
            gl.pixel_store_i32(
                flag,
                if flag == glow::UNPACK_ALIGNMENT || flag == glow::PACK_ALIGNMENT {
                    4
                } else {
                    0
                },
            );
        }
        gl.color_mask(true, true, true, true);
        gl.blend_func_separate(glow::ONE, glow::ZERO, glow::ONE, glow::ZERO);
        gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
        guard
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe {
            let gl = self.gl;
            gl.bind_framebuffer(
                glow::DRAW_FRAMEBUFFER,
                NonZeroU32::new(self.draw as u32).map(glow::NativeFramebuffer),
            );
            gl.bind_framebuffer(
                glow::READ_FRAMEBUFFER,
                NonZeroU32::new(self.read as u32).map(glow::NativeFramebuffer),
            );
            gl.use_program(NonZeroU32::new(self.program as u32).map(glow::NativeProgram));
            gl.bind_vertex_array(NonZeroU32::new(self.vao as u32).map(glow::NativeVertexArray));
            gl.bind_buffer(
                glow::ARRAY_BUFFER,
                NonZeroU32::new(self.array as u32).map(glow::NativeBuffer),
            );
            gl.bind_buffer(
                glow::PIXEL_PACK_BUFFER,
                NonZeroU32::new(self.pack as u32).map(glow::NativeBuffer),
            );
            gl.bind_buffer(
                glow::PIXEL_UNPACK_BUFFER,
                NonZeroU32::new(self.unpack as u32).map(glow::NativeBuffer),
            );
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(
                glow::TEXTURE_2D,
                NonZeroU32::new(self.texture as u32).map(glow::NativeTexture),
            );
            gl.bind_sampler(
                0,
                NonZeroU32::new(self.sampler as u32).map(glow::NativeSampler),
            );
            gl.active_texture(self.active as u32);
            gl.viewport(
                self.viewport[0],
                self.viewport[1],
                self.viewport[2],
                self.viewport[3],
            );
            gl.scissor(
                self.scissor[0],
                self.scissor[1],
                self.scissor[2],
                self.scissor[3],
            );
            gl.color_mask(
                self.mask[0] != 0,
                self.mask[1] != 0,
                self.mask[2] != 0,
                self.mask[3] != 0,
            );
            gl.blend_func_separate(
                self.blend[0] as u32,
                self.blend[1] as u32,
                self.blend[2] as u32,
                self.blend[3] as u32,
            );
            gl.blend_equation_separate(self.blend[4] as u32, self.blend[5] as u32);
            for (flag, value) in PIXEL_STORE.into_iter().zip(self.pixels) {
                gl.pixel_store_i32(flag, value);
            }
            for (flag, enabled) in ENABLES.into_iter().zip(self.enabled) {
                if enabled {
                    gl.enable(flag);
                } else {
                    gl.disable(flag);
                }
            }
        }
    }
}
