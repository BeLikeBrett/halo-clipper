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
