use std::{path::PathBuf, process::Command};
/// Hide console windows for all media subprocesses in the GUI Windows build.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = &mut cmd;
    cmd
}
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(p).join("Halo Clipper");
    }
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".local/state")
        })
        .join("halo-clipper")
}
pub fn init() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let mut paths = vec![dir.to_path_buf()];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            if let Ok(path) = std::env::join_paths(paths) {
                std::env::set_var("PATH", path);
            }
        }
    }
}

/// Opens the folder holding `file`. On Windows, Explorer also selects the
/// file; elsewhere the parent directory goes to the desktop's opener.
pub fn reveal_in_file_manager(file: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `/select,` must be one argument with the path glued on; raw_arg
        // stops Rust from quoting the whole thing, which Explorer rejects.
        let _ = Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", file.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    if let Some(parent) = file.parent() {
        let _ = command("xdg-open").arg(parent).spawn();
    }
}

/// Tells the user why the app is closing. The Windows build is a GUI
/// subsystem exe with no console, so without this a startup failure (for
/// example a GPU driver without OpenGL 2.0, or a Remote Desktop session)
/// looks like "double-clicked it and nothing happened".
pub fn show_startup_error(error: &str) {
    let lower = error.to_ascii_lowercase();
    let hint = if lower.contains("opengl") || lower.contains("wgl") || lower.contains("glutin") {
        "Your graphics driver doesn't provide OpenGL 2.0 or newer, which Halo Clipper \
         needs to draw its window.\n\n\
         - Install the latest driver from NVIDIA, AMD or Intel (the generic \
         \"Microsoft Basic Display Adapter\" driver is not enough).\n\
         - Remote Desktop and some virtual machines don't provide OpenGL; run \
         Halo Clipper on the PC itself."
    } else {
        "Something went wrong while starting up."
    };
    let log = crate::diagnostics::log_dir().join("halo-clipper.log");
    let message = format!(
        "Halo Clipper couldn't start.\n\n{hint}\n\nDetails: {error}\n\nLog: {}",
        log.display()
    );
    log::error!("Startup failed: {error}");

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(
                hwnd: *mut std::ffi::c_void,
                text: *const u16,
                caption: *const u16,
                kind: u32,
            ) -> i32;
        }
        const MB_OK: u32 = 0x0;
        const MB_ICONERROR: u32 = 0x10;
        let wide = |s: &str| -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(Some(0))
                .collect()
        };
        let text = wide(&message);
        let caption = wide("Halo Clipper");
        // SAFETY: both buffers are NUL-terminated UTF-16 that outlive the
        // call; a null owner window is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{message}");
}

/// Pointer position over `window` in egui points, read from the OS. During
/// a file drag from Explorer or an X11 file manager the window gets no
/// pointer events (winit reports none either), so egui's pointer is stale;
/// this lets a drop target the timeline instead of only the media bin.
/// `None` where the platform can't tell (Wayland).
pub fn drag_cursor(
    window: &impl raw_window_handle::HasWindowHandle,
    pixels_per_point: f32,
) -> Option<egui::Pos2> {
    use raw_window_handle::RawWindowHandle;
    let handle = window.window_handle().ok()?;
    let (x, y) = match handle.as_raw() {
        #[cfg(windows)]
        RawWindowHandle::Win32(win32) => {
            #[repr(C)]
            struct Point {
                x: i32,
                y: i32,
            }
            #[link(name = "user32")]
            extern "system" {
                fn GetCursorPos(point: *mut Point) -> i32;
                fn ScreenToClient(hwnd: *mut std::ffi::c_void, point: *mut Point) -> i32;
            }
            let mut point = Point { x: 0, y: 0 };
            // SAFETY: plain Win32 calls with a valid out-pointer and our
            // own live window handle.
            let ok = unsafe {
                GetCursorPos(&mut point) != 0
                    && ScreenToClient(win32.hwnd.get() as *mut _, &mut point) != 0
            };
            ok.then_some((point.x as f32, point.y as f32))?
        }
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(xlib) => x11_pointer(xlib.window)?,
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(xcb) => x11_pointer(u64::from(xcb.window.get()))?,
        _ => return None,
    };
    Some(egui::pos2(x, y) / pixels_per_point.max(0.1))
}

/// Starts reading file drags on a native Wayland window (winit only
/// reports them on X11). `None` on X11 or when the protocol is missing.
#[cfg(target_os = "linux")]
pub fn wayland_drops(
    frame: &eframe::Frame,
    ctx: &egui::Context,
) -> Option<crate::wayland_drop::WaylandDrops> {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    let RawDisplayHandle::Wayland(handle) = frame.display_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: winit's display lives as long as the event loop; the app
    // stops the listener in `on_exit`, before the window is torn down.
    let drops =
        unsafe { crate::wayland_drop::WaylandDrops::start(handle.display.as_ptr(), ctx.clone()) };
    log::info!(
        "wayland file drops: {}",
        if drops.is_some() {
            "listening"
        } else {
            "unavailable"
        }
    );
    drops
}

#[cfg(target_os = "linux")]
fn x11_pointer(window: u64) -> Option<(f32, f32)> {
    use std::sync::OnceLock;
    use x11_dl::xlib;
    // One private connection, opened on first use.
    struct Conn(xlib::Xlib, usize);
    // SAFETY: the display is only touched from the UI thread.
    unsafe impl Send for Conn {}
    unsafe impl Sync for Conn {}
    static CONN: OnceLock<Option<Conn>> = OnceLock::new();
    let conn = CONN
        .get_or_init(|| {
            let lib = xlib::Xlib::open().ok()?;
            // SAFETY: XOpenDisplay(NULL) opens $DISPLAY or returns null.
            let display = unsafe { (lib.XOpenDisplay)(std::ptr::null()) };
            (!display.is_null()).then_some(Conn(lib, display as usize))
        })
        .as_ref()?;
    let (mut root, mut child) = (0, 0);
    let (mut rx, mut ry, mut wx, mut wy, mut mask) = (0, 0, 0, 0, 0);
    // SAFETY: valid display, our window id, and out-pointers on the stack.
    let same_screen = unsafe {
        (conn.0.XQueryPointer)(
            conn.1 as *mut xlib::Display,
            window as xlib::Window,
            &mut root,
            &mut child,
            &mut rx,
            &mut ry,
            &mut wx,
            &mut wy,
            &mut mask,
        )
    };
    (same_screen != 0).then_some((wx as f32, wy as f32))
}

/// Swaps the window's icons for the multi-size icon embedded in the
/// executable (resource 1, see `build.rs`). winit only accepts a single
/// RGBA image and lets Windows scale it, which smears the 16 px title-bar
/// icon; loading from the resource gives the hand-tuned small sizes.
#[cfg(windows)]
pub fn use_exe_icon(window: &impl raw_window_handle::HasWindowHandle) {
    use raw_window_handle::RawWindowHandle;
    use std::ffi::c_void;

    const IMAGE_ICON: u32 = 1;
    const SM_CXICON: i32 = 11;
    const SM_CXSMICON: i32 = 49;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    // MAKEINTRESOURCEW(1): a resource id smuggled through a pointer.
    const APP_ICON: *const u16 = std::ptr::without_provenance(1);

    #[link(name = "user32")]
    extern "system" {
        fn LoadImageW(
            instance: *mut c_void,
            name: *const u16,
            kind: u32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> *mut c_void;
        fn GetSystemMetrics(index: i32) -> i32;
        fn SendMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as *mut c_void;
    // SAFETY: plain Win32 calls on our own module and live window. Null
    // results (icon missing) are skipped; the window keeps winit's icon.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let size = GetSystemMetrics(metric);
            let icon = LoadImageW(module, APP_ICON, IMAGE_ICON, size, size, 0);
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, which, icon as isize);
            }
        }
    }
}
