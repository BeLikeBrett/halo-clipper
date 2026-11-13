// Runtime lookup of OpenGL function pointers, handed to libmpv during
// render context initialization.
//
// libmpv doesn't link to GL itself — it asks us for each function it needs.
// On Linux we get those from EGL (Wayland, modern X11) or GLX (legacy X11).
// We open both libraries and try them in order; whichever returns non-null
// wins.

use anyhow::Result;
use libloading::Library;
use std::ffi::{c_char, c_void, CString};

type GetProcFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;

pub struct GlLoader {
    egl_lib: Option<Library>,
    glx_lib: Option<Library>,
    #[cfg(windows)]
    wgl_lib: Library,
}

impl GlLoader {
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        return Ok(Self {
            egl_lib: None,
            glx_lib: None,
            wgl_lib: unsafe { Library::new("opengl32.dll") }?,
        });
        #[cfg(not(windows))]
        let egl_lib = unsafe { Library::new("libEGL.so.1") }.ok();
        #[cfg(not(windows))]
        let glx_lib = unsafe { Library::new("libGL.so.1") }.ok();
        #[cfg(not(windows))]
        if egl_lib.is_none() && glx_lib.is_none() {
            anyhow::bail!(
                "Could not load libEGL.so.1 or libGL.so.1 — \
                 OpenGL function loading is not possible."
            );
        }
        #[cfg(not(windows))]
        Ok(Self { egl_lib, glx_lib })
    }

    pub fn get_proc(&self, name: &str) -> *mut c_void {
        let Ok(cname) = CString::new(name) else {
            return std::ptr::null_mut();
        };

        #[cfg(windows)]
        unsafe {
            type WglProc = unsafe extern "system" fn(*const c_char) -> *mut c_void;
            if let Ok(get) = self.wgl_lib.get::<WglProc>(b"wglGetProcAddress") {
                let p = get(cname.as_ptr());
                if !p.is_null() && ![1, 2, 3, usize::MAX].contains(&(p as usize)) {
                    return p;
                }
            }
            if let Ok(p) = self
                .wgl_lib
                .get::<unsafe extern "system" fn()>(cname.as_bytes())
            {
                return *p as *mut c_void;
            }
        }
        // Try EGL first — that's the path on Wayland and modern X11.
        if let Some(egl) = &self.egl_lib {
            if let Some(p) = lookup(egl, b"eglGetProcAddress", &cname) {
                if !p.is_null() {
                    return p;
                }
            }
        }

        // Fall back to GLX. ARB variant first (more reliable), then plain.
        if let Some(glx) = &self.glx_lib {
            for sym in [
                b"glXGetProcAddressARB" as &[u8],
                b"glXGetProcAddress" as &[u8],
            ] {
                if let Some(p) = lookup(glx, sym, &cname) {
                    if !p.is_null() {
                        return p;
                    }
                }
            }
            // Some functions are exported directly from libGL.so.1 (not
            // through a loader). Fall back to plain dlsym for those.
            unsafe {
                if let Ok(sym) = glx.get::<*mut c_void>(name.as_bytes()) {
                    return *sym;
                }
            }
        }

        std::ptr::null_mut()
    }
}

fn lookup(lib: &Library, symbol: &[u8], name: &std::ffi::CStr) -> Option<*mut c_void> {
    unsafe {
        lib.get::<GetProcFn>(symbol)
            .ok()
            .map(|sym| sym(name.as_ptr()))
    }
}

// Trampoline matching the signature libmpv2's OpenGLInitParams expects:
//   fn(ctx: &GLContext, name: &str) -> *mut c_void
pub fn get_proc_address(loader: &std::sync::Arc<GlLoader>, name: &str) -> *mut c_void {
    loader.get_proc(name)
}
