#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
// Halo Clipper — entry point.
// Sets up logging, creates the eframe window, and hands off to ClipperApp.

mod animation;
mod app;
mod composition;
mod diagnostics;
mod encode;
mod export;
mod framing;
mod gl_interop;
mod gl_loader;
mod media;
mod mpv_render;
mod picker;
mod platform;
mod player;
mod proxy;
mod settings;
mod startup;
mod text;
mod textrender;
mod theme;
mod timeline;
mod track;
mod updater;
mod video_fbo;
mod widgets;

use app::ClipperApp;

fn main() -> eframe::Result<()> {
    platform::init();
    if std::env::args_os().any(|arg| arg == "--self-check") {
        if let Err(error) = startup::check_runtime() {
            eprintln!("Halo Clipper startup check failed: {error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // HaloOS can force MangoHud into every Vulkan client. libmpv probes
    // Vulkan decoders even though our preview renders with OpenGL; the
    // overlay can crash inside vkCreateDevice during that probe. Opt this
    // editor (and its media subprocesses) out before any graphics threads
    // start. DISABLE_MANGOHUD also covers forced implicit-layer manifests.
    std::env::set_var("DISABLE_MANGOHUD", "1");
    std::env::set_var("MANGOHUD", "0");

    diagnostics::init();

    // A path argument means we were launched via "Open With" / the
    // .desktop MimeType association (Exec=... %F) or a shell like
    // `halo-clipper clip.mp4`. Open it straight away.
    let layer_args: Vec<_> = std::env::args_os().collect();
    let layers: Vec<_> = layer_args
        .windows(2)
        .filter(|pair| pair[0] == "--layer")
        .map(|pair| std::path::PathBuf::from(&pair[1]))
        .collect();
    let advanced = layer_args.iter().any(|arg| arg == "--advanced");
    let initial_file = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file());

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("Halo Clipper")
        .with_drag_and_drop(true)
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
            #[cfg(windows)]
            platform::use_exe_icon(cc);
            load_caption_fonts(&cc.egui_ctx);
            match ClipperApp::new(cc) {
                Ok(mut app) => {
                    if let Some(path) = initial_file {
                        app.open_path(path);
                    }
                    if advanced {
                        app.set_mode(app::EditMode::MultiTrack);
                    }
                    for layer in layers {
                        app.add_layer_path(layer);
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

/// Registers the interface fonts, the DejaVu fallbacks, and every caption
/// font as its own egui family (the text font menu shows each name in its
/// own face).
fn load_caption_fonts(ctx: &egui::Context) {
    let candidates: &[(&str, &[u8])] = &[
        (
            "DejaVu Sans",
            include_bytes!("../assets/fonts/DejaVuSans.ttf"),
        ),
        (
            "DejaVu Sans Mono",
            include_bytes!("../assets/fonts/DejaVuSansMono.ttf"),
        ),
    ];

    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in candidates {
        fonts
            .font_data
            .insert((*name).to_string(), egui::FontData::from_static(bytes));
        fonts.families.insert(
            egui::FontFamily::Name((*name).into()),
            vec![(*name).to_string()],
        );
    }
    // Interface typography. Inter carries the UI at three weights (egui
    // has no synthetic bold, so weight is a separate family); JetBrains
    // Mono gives timecodes even, slashed-zero figures. Both are Latin
    // subsets (OFL) — DejaVu stays behind them for symbol coverage.
    let ui_fonts: &[(&str, &'static [u8])] = &[
        (
            theme::FONT_REGULAR,
            include_bytes!("../assets/fonts/Inter-Regular.ttf"),
        ),
        (
            theme::FONT_MEDIUM,
            include_bytes!("../assets/fonts/Inter-Medium.ttf"),
        ),
        (
            theme::FONT_SEMIBOLD,
            include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
        ),
        (
            theme::FONT_MONO,
            include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
        ),
    ];
    for (name, bytes) in ui_fonts {
        fonts
            .font_data
            .insert((*name).to_string(), egui::FontData::from_static(bytes));
    }
    // Named weights fall back through Inter Regular → DejaVu → egui's emoji
    // fonts so a stray glyph never renders as tofu.
    let fallback: Vec<String> = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let chain = |first: &str| {
        let mut list = vec![
            first.to_string(),
            theme::FONT_REGULAR.to_string(),
            "DejaVu Sans".to_string(),
        ];
        list.extend(fallback.iter().cloned());
        list.dedup();
        list
    };
    for name in [theme::FONT_MEDIUM, theme::FONT_SEMIBOLD] {
        fonts
            .families
            .insert(egui::FontFamily::Name(name.into()), chain(name));
    }
    fonts
        .families
        .insert(egui::FontFamily::Proportional, chain(theme::FONT_REGULAR));
    let mono = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default();
    mono.insert(0, theme::FONT_MONO.into());
    mono.insert(1, "DejaVu Sans Mono".into());
    for font in text::TextFont::ALL {
        let family = font.egui_family();
        fonts
            .font_data
            .insert(family.clone(), egui::FontData::from_static(font.data()));
        fonts
            .families
            .insert(egui::FontFamily::Name(family.clone().into()), vec![family]);
    }
    ctx.set_fonts(fonts);
}
