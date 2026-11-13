// Embedded video player built on libmpv linked directly (no subprocess).
//
// Architecture:
//   * `Mpv` is initialized once with `vo=libmpv` (so it doesn't create its
//     own window) and `hwdec=auto-safe`. We hold it in an `Arc<Mpv>` because
//     it's Send+Sync and we share it with a background event thread.
//   * A background thread loops on `mpv.wait_event()` and pushes property
//     changes (time-pos / pause / volume / mute / duration) into a shared
//     `PlayerState` the UI polls each frame.
//   * The `RenderContext` is created while eframe initializes the app,
//     before any media is loaded. Because it borrows
//     from `Mpv`, we use a transmute to give it `'static` lifetime; drop
//     order is enforced manually in `Player::drop`.
//   * `Player` is shared with the egui paint callback as `Arc<Player>`,
//     which requires Send + Sync. `RenderContext` is `!Send`, so we wrap
//     it in `MainThreadOnly` and assert Send+Sync — justified because egui
//     paint callbacks are always invoked from the main thread.

use crate::gl_loader::{self, GlLoader};

use anyhow::{Context, Result};
use libmpv2::events::{Event, PropertyData};
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::{Format, Mpv};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct PlayerState {
    pub paused: bool,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub volume: f64,
    pub muted: bool,
    pub loaded_file: Option<PathBuf>,
    pub video_size: Option<(i32, i32)>,
    pub last_update: Option<Instant>,
}

impl PlayerState {
    fn fresh() -> Self {
        Self {
            paused: true,
            position_ms: 0,
            duration_ms: 0,
            volume: 100.0,
            muted: false,
            loaded_file: None,
            video_size: None,
            last_update: None,
        }
    }
}

// SAFETY wrapper: `RenderContext` is !Send. We need Send+Sync on the Arc
// holding the player so it can be captured by egui paint callbacks (which
// require those bounds). In practice paint callbacks ALWAYS run on the
// main thread, so cross-thread access never happens — the bounds are an
// API requirement, not a usage one.
struct MainThreadOnly<T>(T);
unsafe impl<T> Send for MainThreadOnly<T> {}
unsafe impl<T> Sync for MainThreadOnly<T> {}

struct RenderState {
    ctx: MainThreadOnly<RenderContext<'static>>,
    _gl_loader: Arc<GlLoader>,
}

struct Inner {
    desired_file: Option<PathBuf>,
    desired_position_ms: i64,
}

pub struct Player {
    // Declared FIRST so it drops FIRST — RenderContext borrows from Mpv via
    // a transmute, and dropping in the wrong order is undefined behavior.
    render: Mutex<Option<RenderState>>,
    inner: Mutex<Inner>,
    state: Arc<Mutex<PlayerState>>,
    mpv: Arc<Mpv>,
    event_stop: Arc<AtomicBool>,
    _event_thread: Mutex<Option<JoinHandle<()>>>,
    repaint_ctx: egui::Context,
}

impl Player {
    pub fn new(repaint_ctx: egui::Context) -> Result<Self> {
        Self::create(repaint_ctx, false, false)
    }
    pub fn new_audio(repaint_ctx: egui::Context) -> Result<Self> {
        Self::create(repaint_ctx, true, false)
    }
    pub fn new_layer(repaint_ctx: egui::Context) -> Result<Self> {
        Self::create(repaint_ctx, false, true)
    }
    fn create(repaint_ctx: egui::Context, audio_only: bool, video_only: bool) -> Result<Self> {
        let mpv = Mpv::with_initializer(|init| {
            if log::log_enabled!(log::Level::Debug) {
                init.set_property("terminal", "yes")?;
                init.set_property("msg-level", "all=v")?;
            }
            init.set_property("vo", if audio_only { "null" } else { "libmpv" })?;
            init.set_property("hwdec", "auto-safe")?;
            init.set_property("audio-display", "no")?;
            if audio_only {
                init.set_property("vid", "no")?;
            }
            if video_only {
                init.set_property("aid", "no")?;
            }
            init.set_property("input-default-bindings", "no")?;
            init.set_property("input-vo-keyboard", "no")?;
            init.set_property("video-timing-offset", 0i64)?;
            init.set_property("keep-open", "yes")?;
            init.set_property("pause", "yes")?;
            init.set_property("volume-max", 1000i64)?;

            // ─────────── Editing-friendly tuning ───────────
            // mpv's defaults target one-pass playback ("press play and
            // watch"). An editor scrubs back and forth across the same
            // ranges repeatedly, so we lean into mpv's cache + decode
            // pipeline to make seeks feel instant. Mirrors what
            // kdenlive / Shotcut / Olive set on their mpv instances.

            // Force the demuxer cache on (default is "auto", which can
            // disable for local files on some builds) so re-scrubbing
            // already-visited regions doesn't re-read from disk.
            init.set_property("cache", "yes")?;
            // Keep ~30 s of decoded frames around — backward scrubs
            // within the window become instant.
            init.set_property("cache-secs", 30i64)?;
            // Read ahead aggressively so forward scrubs stay decoded.
            init.set_property("demuxer-readahead-secs", 20i64)?;
            // Generous demuxer cache so big 4K source files stay
            // resident in RAM rather than streaming from disk each seek.
            init.set_property("demuxer-max-bytes", "2GiB")?;
            init.set_property("demuxer-max-back-bytes", "2GiB")?;
            // During hr-seek (precise seek on click / release), let the
            // decoder drop frames if it's behind — fast catch-up to the
            // target instead of waiting on a full decode pass.
            init.set_property("hr-seek-framedrop", "yes")?;
            // Use all CPU cores for software decode fallback paths.
            init.set_property("vd-lavc-threads", 0i64)?;
            // Allow the decoder to skip safety checks for ~10% more
            // throughput; matters when the source is high-bitrate 4K.
            init.set_property("vd-lavc-fast", "yes")?;
            // Looser timing constraints — perceptible to nobody during
            // scrubbing, shaves a frame or two of latency on seeks.
            init.set_property("video-latency-hacks", "yes")?;
            if audio_only || video_only {
                init.set_property("cache-secs", 8i64)?;
                init.set_property("demuxer-readahead-secs", 5i64)?;
                init.set_property("demuxer-max-bytes", "128MiB")?;
                init.set_property("demuxer-max-back-bytes", "64MiB")?;
            }
            Ok(())
        })
        .context("mpv init failed (is libmpv installed?)")?;

        let mpv = Arc::new(mpv);
        let state = Arc::new(Mutex::new(PlayerState::fresh()));
        let event_stop = Arc::new(AtomicBool::new(false));

        mpv.observe_property("time-pos", Format::Double, 1)?;
        mpv.observe_property("pause", Format::Flag, 2)?;
        mpv.observe_property("volume", Format::Double, 3)?;
        mpv.observe_property("mute", Format::Flag, 4)?;
        mpv.observe_property("duration", Format::Double, 5)?;
        mpv.observe_property("dwidth", Format::Int64, 6)?;
        mpv.observe_property("dheight", Format::Int64, 7)?;

        let event_thread = {
            let mpv = mpv.clone();
            let state = state.clone();
            let stop = event_stop.clone();
            let repaint_ctx = repaint_ctx.clone();
            thread::spawn(move || event_loop(mpv, state, stop, repaint_ctx))
        };

        Ok(Self {
            render: Mutex::new(None),
            inner: Mutex::new(Inner {
                desired_file: None,
                desired_position_ms: 0,
            }),
            state,
            mpv,
            event_stop,
            _event_thread: Mutex::new(Some(event_thread)),
            repaint_ctx,
        })
    }

    pub fn state(&self) -> PlayerState {
        self.state.lock().unwrap().clone()
    }

    pub fn set_file(&self, source: &Path, start_ms: i64) -> Result<()> {
        let path = source.to_str().context("non-utf8 path")?;
        {
            let mut i = self.inner.lock().unwrap();
            i.desired_file = Some(source.to_path_buf());
            i.desired_position_ms = start_ms;
        }
        let start_s = (start_ms as f64 / 1000.0).max(0.0);
        let options = format!("start={start_s},pause=yes");
        self.mpv
            .command("loadfile", &[path, "replace", "0", &options])?;
        let mut s = self.state.lock().unwrap();
        s.loaded_file = Some(source.to_path_buf());
        s.position_ms = start_ms;
        s.paused = true;
        Ok(())
    }

    pub fn toggle_play(&self) -> Result<()> {
        self.mpv.command("cycle", &["pause"])?;
        Ok(())
    }

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.mpv.set_property("pause", paused)?;
        Ok(())
    }

    /// Frame-accurate seek. Slow on long GOPs because mpv decodes from the
    /// previous keyframe forward. Use on click and on drag-release.
    pub fn seek(&self, position_ms: i64) -> Result<()> {
        let s = (position_ms as f64 / 1000.0).max(0.0).to_string();
        self.mpv.command("seek", &[&s, "absolute", "exact"])?;
        Ok(())
    }

    /// Snap-to-nearest-keyframe seek. Effectively free (no decode). Use
    /// while the user is actively dragging the scrubber/timeline so the
    /// preview keeps up; reissue an exact seek on release.
    pub fn seek_fast(&self, position_ms: i64) -> Result<()> {
        let s = (position_ms as f64 / 1000.0).max(0.0).to_string();
        self.mpv.command("seek", &[&s, "absolute", "keyframes"])?;
        Ok(())
    }

    pub fn set_volume(&self, vol: f64) -> Result<()> {
        self.mpv.set_property("volume", vol.clamp(0.0, 130.0))?;
        Ok(())
    }

    pub fn toggle_mute(&self) -> Result<()> {
        self.mpv.command("cycle", &["mute"])?;
        Ok(())
    }

    pub fn set_mute(&self, muted: bool) -> Result<()> {
        self.mpv.set_property("mute", muted)?;
        Ok(())
    }

    /// Set mpv's playback speed multiplier. 1.0 = normal. Used by the
    /// per-clip speed feature to mirror the active clip's speed.
    pub fn set_speed(&self, speed: f64) -> Result<()> {
        self.mpv.set_property("speed", speed.max(0.01))?;
        Ok(())
    }

    /// Set mpv's complex filter graph. Used by MultiTrack mode to mix
    /// multiple source audio streams (e.g. `[aid1][aid2]amix=inputs=2[ao]`).
    /// Pass an empty string to clear and fall back to default routing.
    pub fn set_audio_stream(&self, index: usize) -> Result<()> {
        self.mpv.set_property("aid", (index + 1) as i64)?;
        Ok(())
    }

    pub fn set_video_filter(&self, expr: &str) -> Result<()> {
        self.mpv.set_property("vf", expr)?;
        Ok(())
    }

    pub fn set_lavfi_complex(&self, expr: &str) -> Result<()> {
        self.mpv.set_property("lavfi-complex", expr)?;
        Ok(())
    }

    /// Initialize the OpenGL render context if it hasn't been already.
    /// Idempotent and cheap to call.
    pub fn ensure_render_context(&self) -> Result<()> {
        let mut slot = self.render.lock().unwrap();
        if slot.is_some() {
            return Ok(());
        }
        let loader = Arc::new(GlLoader::new().context("GL function loader init failed")?);
        let init_params = OpenGLInitParams {
            get_proc_address: gl_loader::get_proc_address,
            ctx: loader.clone(),
        };
        let params = vec![
            RenderParam::ApiType(RenderParamApiType::OpenGl),
            RenderParam::InitParams(init_params),
        ];

        // Lifetime extension: mpv lives in the same struct alongside the
        // render context. Drop order is enforced manually below and via
        // field ordering above.
        let mpv_ref: &Mpv = &self.mpv;
        let mpv_static: &'static Mpv = unsafe { std::mem::transmute(mpv_ref) };
        let mut ctx: RenderContext<'static> = mpv_static
            .create_render_context(params)
            .context("create_render_context failed")?;

        // Decoding and seeks finish asynchronously, often after egui has
        // gone idle (especially the first paused frame after import).
        // Wake the UI when mpv has a frame instead of leaving a black or
        // stale preview until the next mouse/keyboard event. The callback
        // only schedules painting; all GL/mpv rendering stays on the UI thread.
        let repaint_ctx = self.repaint_ctx.clone();
        ctx.set_update_callback(move || {
            log::debug!("mpv frame ready");
            repaint_ctx.request_repaint();
        });

        *slot = Some(RenderState {
            ctx: MainThreadOnly(ctx),
            _gl_loader: loader,
        });
        Ok(())
    }

    /// Render the current frame into `fbo` at the given pixel size.
    /// Caller must be on the main (UI) thread with a valid GL context bound.
    pub fn render(&self, fbo: i32, width: i32, height: i32, flip: bool) -> Result<()> {
        let slot = self.render.lock().unwrap();
        let rs = slot.as_ref().context("render context not initialized")?;
        // Acknowledge frame notifications so mpv can notify us again.
        let _ = rs.ctx.0.update();
        rs.ctx
            .0
            .render::<Arc<GlLoader>>(fbo, width, height, flip)
            .context("mpv render failed")?;
        Ok(())
    }
}

fn event_loop(
    mpv: Arc<Mpv>,
    state: Arc<Mutex<PlayerState>>,
    stop: Arc<AtomicBool>,
    repaint_ctx: egui::Context,
) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match mpv.wait_event(0.25) {
            None => continue,
            Some(Err(e)) => {
                log::warn!("mpv event error: {e:?}");
                continue;
            }
            Some(Ok(Event::Shutdown)) => return,
            Some(Ok(Event::EndFile(reason))) if reason == libmpv2::mpv_end_file_reason::Error => {
                log::error!("mpv could not decode the selected media");
            }
            Some(Ok(Event::LogMessage {
                prefix,
                level,
                text,
                ..
            })) => {
                log::warn!("mpv {prefix} ({level}): {}", text.trim());
            }
            Some(Ok(Event::PropertyChange { name, change, .. })) => {
                log::debug!("mpv property {name}: {change:?}");
                let mut s = state.lock().unwrap();
                s.last_update = Some(Instant::now());
                match (name, change) {
                    ("time-pos", PropertyData::Double(v)) => {
                        s.position_ms = (v * 1000.0).max(0.0) as i64;
                    }
                    ("duration", PropertyData::Double(v)) => {
                        s.duration_ms = (v * 1000.0).max(0.0) as i64;
                    }
                    ("pause", PropertyData::Flag(v)) => s.paused = v,
                    ("volume", PropertyData::Double(v)) => s.volume = v,
                    ("mute", PropertyData::Flag(v)) => s.muted = v,
                    ("dwidth", PropertyData::Int64(w)) => {
                        let h = s.video_size.map(|(_, h)| h).unwrap_or(0);
                        s.video_size = Some((w as i32, h));
                    }
                    ("dheight", PropertyData::Int64(h)) => {
                        let w = s.video_size.map(|(w, _)| w).unwrap_or(0);
                        s.video_size = Some((w, h as i32));
                    }
                    _ => {}
                }
                // egui may read PlayerState while holding its own context
                // lock. Release our lock before scheduling a repaint.
                drop(s);
                repaint_ctx.request_repaint();
            }
            Some(Ok(_)) => {}
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Explicit drop of render context first — its lifetime was extended
        // to 'static via transmute on top of `mpv`, so it must not outlive
        // the Mpv Arc.
        self.event_stop.store(true, Ordering::SeqCst);
        let _ = self.mpv.command("stop", &[]);
        self.render.lock().unwrap().take();
    }
}
