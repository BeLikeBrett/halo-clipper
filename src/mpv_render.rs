//! Narrow native render API adapter. Always pass mpv's generated C structures:
//! the high-level binding's FBO lacks internal_format and is not repr(C).
//! Stack-owned parameter arrays also avoid mismatched allocation/deallocation.
use crate::gl_loader::GlLoader;
use anyhow::{ensure, Result};
use libmpv2::Mpv;
use libmpv2_sys as sys;
use std::{
    ffi::{c_char, c_void, CStr},
    ptr::{self, NonNull},
    sync::Arc,
};
pub struct NativeRender {
    context: NonNull<sys::mpv_render_context>,
    _core: Arc<Mpv>,
    _loader: Arc<GlLoader>,
    _wake: Box<egui::Context>,
}
unsafe extern "C" fn get_proc(context: *mut c_void, name: *const c_char) -> *mut c_void {
    if context.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let loader = unsafe { &*context.cast::<GlLoader>() };
    let Ok(name) = (unsafe { CStr::from_ptr(name) }).to_str() else {
        return ptr::null_mut();
    };
    loader.get_proc(name)
}
unsafe extern "C" fn wake(context: *mut c_void) {
    if !context.is_null() {
        unsafe { &*context.cast::<egui::Context>() }.request_repaint();
    }
}
impl NativeRender {
    pub fn new(core: Arc<Mpv>, loader: Arc<GlLoader>, repaint: egui::Context) -> Result<Self> {
        let mut init = sys::mpv_opengl_init_params {
            get_proc_address: Some(get_proc),
            get_proc_address_ctx: Arc::as_ptr(&loader).cast_mut().cast(),
        };
        let mut params = [
            sys::mpv_render_param {
                type_: 1,
                data: c"opengl".as_ptr().cast_mut().cast(),
            },
            sys::mpv_render_param {
                type_: 2,
                data: (&mut init as *mut sys::mpv_opengl_init_params).cast(),
            },
            sys::mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];
        let mut context = ptr::null_mut();
        let code = unsafe {
            sys::mpv_render_context_create(&mut context, core.ctx.as_ptr(), params.as_mut_ptr())
        };
        ensure!(
            code >= 0,
            "mpv render initialization: {}",
            libmpv2::Error::Raw(code)
        );
        let context = NonNull::new(context)
            .ok_or_else(|| anyhow::anyhow!("mpv returned no render context"))?;
        let mut repaint = Box::new(repaint);
        unsafe {
            sys::mpv_render_context_set_update_callback(
                context.as_ptr(),
                Some(wake),
                (&mut *repaint as *mut egui::Context).cast(),
            );
        }
        Ok(Self {
            context,
            _core: core,
            _loader: loader,
            _wake: repaint,
        })
    }
    pub fn render(&self, fbo: i32, width: i32, height: i32, flip: bool) -> Result<()> {
        let mut target = sys::mpv_opengl_fbo {
            fbo,
            w: width,
            h: height,
            internal_format: glow::RGBA8 as i32,
        };
        let mut flip = i32::from(flip);
        let mut block = 0i32;
        let mut params = [
            sys::mpv_render_param {
                type_: 3,
                data: (&mut target as *mut sys::mpv_opengl_fbo).cast(),
            },
            sys::mpv_render_param {
                type_: 4,
                data: (&mut flip as *mut i32).cast(),
            },
            sys::mpv_render_param {
                type_: 12,
                data: (&mut block as *mut i32).cast(),
            },
            sys::mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];
        unsafe {
            sys::mpv_render_context_update(self.context.as_ptr());
        }
        let code =
            unsafe { sys::mpv_render_context_render(self.context.as_ptr(), params.as_mut_ptr()) };
        ensure!(code >= 0, "mpv frame render: {}", libmpv2::Error::Raw(code));
        Ok(())
    }
}
impl Drop for NativeRender {
    fn drop(&mut self) {
        unsafe {
            sys::mpv_render_context_set_update_callback(
                self.context.as_ptr(),
                None,
                ptr::null_mut(),
            );
            sys::mpv_render_context_free(self.context.as_ptr());
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn framebuffer_has_the_complete_native_abi() {
        use libmpv2_sys::mpv_opengl_fbo as Fbo;
        assert_eq!(std::mem::size_of::<Fbo>(), 16);
        assert_eq!(std::mem::offset_of!(Fbo, internal_format), 12);
    }
}
