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

#[cfg(windows)]
pub fn caption_font(display: &str) -> PathBuf {
    let (name, bytes): (&str, &[u8]) = match display {
        "Sans Bold" => (
            "DejaVuSans-Bold.ttf",
            include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf"),
        ),
        "Sans Italic" => (
            "DejaVuSans-Oblique.ttf",
            include_bytes!("../assets/fonts/DejaVuSans-Oblique.ttf"),
        ),
        "Serif" => (
            "DejaVuSerif.ttf",
            include_bytes!("../assets/fonts/DejaVuSerif.ttf"),
        ),
        "Serif Bold" => (
            "DejaVuSerif-Bold.ttf",
            include_bytes!("../assets/fonts/DejaVuSerif-Bold.ttf"),
        ),
        "Mono" => (
            "DejaVuSansMono.ttf",
            include_bytes!("../assets/fonts/DejaVuSansMono.ttf"),
        ),
        _ => (
            "DejaVuSans.ttf",
            include_bytes!("../assets/fonts/DejaVuSans.ttf"),
        ),
    };
    let folder = data_dir().join("fonts");
    let path = folder.join(name);
    if !path.exists() {
        let _ = std::fs::create_dir_all(folder);
        let _ = std::fs::write(&path, bytes);
    }
    path
}
