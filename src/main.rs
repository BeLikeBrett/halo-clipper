// Halo Clipper — entry point.
// Sets up logging, creates the eframe window, and hands off to ClipperApp.

mod app;
mod encode;
mod export;
mod gl_loader;
mod media;
mod player;
mod proxy;
mod settings;
mod theme;
mod timeline;
mod track;
mod video_fbo;

use app::ClipperApp;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // A path argument means we were launched via "Open With" / the
    // .desktop MimeType association (Exec=... %F) or a shell like
    // `halo-clipper clip.mp4`. Open it straight away.
    let initial_file = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file());

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("Halo Clipper")
        // Match what the .desktop file sets so X11/Wayland compositors
        // group the running window under our menu entry / icon.
        .with_app_id("halo-clipper");

    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Halo Clipper",
        native_options,
        Box::new(move |cc| {
            load_caption_fonts(&cc.egui_ctx);
            match ClipperApp::new(cc) {
                Ok(mut app) => {
                    if let Some(path) = initial_file {
                        app.open_path(path);
                    }
                    Ok(Box::new(app) as Box<dyn eframe::App>)
                }
                Err(e) => {
                    eprintln!("Failed to start Halo Clipper: {e:#}");
                    Err(format!("{e:#}").into())
                }
            }
        }),
    )
}

/// Embedded application icon. `include_bytes!` baked in at compile
/// time so the running app doesn't depend on the installed PNG file
/// being present — the icon shows in the dock/taskbar regardless of
/// whether the user ran install.sh. Returns None if the PNG fails to
/// decode (shouldn't happen for a known-good asset, but we don't want
/// to crash on a missing icon — we'd just launch without one).
fn load_window_icon() -> Option<egui::IconData> {
    static ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (w, h) = (img.width(), img.height());
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

/// Register DejaVu font variants with egui so the live caption preview
/// can render in the same family the user picks from the dropdown.
///
/// We probe a handful of common system paths (Arch, Debian/Ubuntu) and
/// silently skip any that aren't there — egui falls back to its built-in
/// Proportional family for missing names, so the worst case is "preview
/// shows base style but export still uses the right font."
fn load_caption_fonts(ctx: &egui::Context) {
    let candidates: &[(&str, &[&str])] = &[
        ("DejaVu Sans", &[
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        ]),
        ("DejaVu Sans Bold", &[
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ]),
        ("DejaVu Sans Oblique", &[
            "/usr/share/fonts/TTF/DejaVuSans-Oblique.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
        ]),
        ("DejaVu Serif", &[
            "/usr/share/fonts/TTF/DejaVuSerif.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
        ]),
        ("DejaVu Serif Bold", &[
            "/usr/share/fonts/TTF/DejaVuSerif-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
        ]),
        ("DejaVu Sans Mono", &[
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ]),
    ];

    let mut fonts = egui::FontDefinitions::default();
    for (name, paths) in candidates {
        let Some(bytes) = paths.iter().find_map(|p| std::fs::read(p).ok()) else {
            log::info!("caption font {name} not found on system");
            continue;
        };
        fonts.font_data.insert(
            (*name).to_string(),
            egui::FontData::from_owned(bytes),
        );
        fonts.families.insert(
            egui::FontFamily::Name((*name).into()),
            vec![(*name).to_string()],
        );
    }
    ctx.set_fonts(fonts);
}
