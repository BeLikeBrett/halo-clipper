//! Independent decoders + GPU composition: transforms never encode a proxy.
use super::*;
use std::time::Instant;

type FrameBuffer = Arc<Mutex<Option<VideoFbo>>>;

struct Slot {
    player: Arc<Player>,
    fbo: FrameBuffer,
    path: PathBuf,
    clip: ClipId,
    seek: i64,
    playing: bool,
    speed: f32,
    volume: f64,
    muted: Option<bool>,
    /// lavfi-complex currently set on an audio player ("" = plain `aid`).
    graph: String,
    aid: Option<usize>,
    /// Clock epoch this decoder was last aligned to (see `LivePreview::epoch`).
    epoch: u64,
    corrected: Instant,
}
impl Slot {
    fn new(player: Player, clip: ClipId) -> Self {
        Self {
            player: Arc::new(player),
            fbo: Arc::new(Mutex::new(None)),
            path: PathBuf::new(),
            clip,
            seek: -1,
            playing: false,
            speed: 0.0,
            volume: -1.0,
            muted: None,
            graph: String::new(),
            aid: None,
            epoch: u64::MAX,
            corrected: Instant::now(),
        }
    }
}
#[derive(Clone)]
pub(super) struct FrameLayer {
    pub id: ClipId,
    pub player: Arc<Player>,
    pub fbo: FrameBuffer,
    pub width: u32,
    pub height: u32,
    pub transform: track::Transform,
    pub flash: f32,
    /// Index of the clip's track in `Project::tracks` (0 = top lane).
    pub depth: usize,
}

/// One audio stream that should be audible at the playhead.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Voice {
    track: TrackId,
    stream: usize,
    /// Linear clip × track gain, as in the export's `volume=` filter.
    gain: f32,
}

/// Audio clips that play the same file with the same timing (the game and
/// mic streams of one recording, typically) share a single decoder that
/// mixes them. Separate decoders drift apart by tens of milliseconds,
/// which sounds like an echo, and their sum reached the speakers without
/// the export's limiter, so loud moments clipped in the preview only.
struct AudioGroup<'a> {
    lead: TrackId,
    clip: &'a Clip,
    path: &'a std::path::Path,
    source_time: i64,
    voices: Vec<Voice>,
}

/// mpv `lavfi-complex` for several streams of one file: per-stream gain,
/// an unnormalized sum and the same limiter the export uses.
fn mix_graph(voices: &[Voice]) -> String {
    let inputs: String = voices
        .iter()
        .enumerate()
        .map(|(i, v)| format!("[aid{}]volume={:.3}[v{i}];", v.stream + 1, v.gain.max(0.0)))
        .collect();
    let labels: String = (0..voices.len()).map(|i| format!("[v{i}]")).collect();
    format!(
        "{inputs}{labels}amix=inputs={}:normalize=0:dropout_transition=0,alimiter=limit=0.95:level=0:attack=0.1[ao]",
        voices.len()
    )
}

fn trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HALO_CLIPPER_TRACE_LIVE").is_some())
}

/// Drift (ms) past which a playing decoder is re-seeked to the clock.
const DRIFT_MS: i64 = 120;
/// How far ahead of the playhead a lane's next cut gets a standby decoder.
const LOOKAHEAD_MS: i64 = 2_500;

/// What one lane should play at some moment.
struct Want<'a> {
    /// Decoder key: the video lane, or an audio group's first track.
    key: TrackId,
    clip: &'a Clip,
    path: &'a std::path::Path,
    source_time: i64,
    /// Video lanes: (depth, width, height).
    video: Option<(usize, u32, u32)>,
    voices: Vec<Voice>,
}

/// A standby video decoder whose first frame is drawn offscreen each
/// frame, so its framebuffer already holds the cut's image when it swaps in.
#[derive(Clone)]
pub(super) struct WarmLayer {
    player: Arc<Player>,
    fbo: FrameBuffer,
    width: u32,
    height: u32,
}

#[derive(Default)]
pub(super) struct LivePreview {
    slots: HashMap<TrackId, Slot>,
    /// Second decoder per lane, parked on the lane's next cut.
    standby: HashMap<TrackId, Slot>,
    warm: Vec<WarmLayer>,
    pub playing: bool,
    anchor_ms: i64,
    anchor: Option<Instant>,
    /// Bumped whenever the clock jumps (seek, play/pause). Decoders that
    /// haven't seen the current epoch are re-aligned at once instead of
    /// waiting out the drift guard, so a seek during playback never plays
    /// the old position for most of a second.
    epoch: u64,
    pub frames: Vec<FrameLayer>,
    gpu: Arc<Mutex<Option<CompositeGpu>>>,
    retired: Arc<Mutex<Vec<FrameBuffer>>>,
    /// Audio tracks silenced for monitoring only. Lives here, not on the
    /// project, so it never reaches undo history or the export.
    pub monitor_muted: HashSet<TrackId>,
}
impl LivePreview {
    pub fn position(&self) -> i64 {
        self.anchor_ms
            + self
                .anchor
                .map(|a| a.elapsed().as_millis() as i64)
                .unwrap_or(0)
    }
    pub fn reanchor(&mut self, ms: i64) {
        if ms != self.position() {
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.anchor_ms = ms;
        self.anchor = self.playing.then(Instant::now);
    }
    pub fn pause(&mut self) {
        self.anchor_ms = self.position();
        self.playing = false;
        self.anchor = None;
        self.epoch = self.epoch.wrapping_add(1);
    }
    pub fn toggle(&mut self, ms: i64) {
        self.playing = !self.playing;
        self.anchor_ms = ms;
        self.anchor = self.playing.then(Instant::now);
        self.epoch = self.epoch.wrapping_add(1);
    }
    pub fn clear(&mut self) {
        self.pause();
        for slot in self.slots.values().chain(self.standby.values()) {
            let _ = slot.player.set_paused(true);
        }
        self.retired.lock().unwrap().extend(
            self.slots
                .values()
                .chain(self.standby.values())
                .map(|s| s.fbo.clone()),
        );
        self.slots.clear();
        self.standby.clear();
        self.frames.clear();
        self.warm.clear();
    }

    /// Audio that should sound at `at`, grouped per decoder.
    fn audio_groups(p: &Project, at: i64) -> Vec<AudioGroup<'_>> {
        let mut groups: Vec<AudioGroup<'_>> = Vec::new();
        for t in p
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && !t.muted)
        {
            let Some(c) = track::clip_on_track_at(&p.clips, t.id, at) else {
                continue;
            };
            let (path, media) = p.clip_source(c);
            let stream = c.audio_stream.or(t.source_audio_index).unwrap_or(0);
            if stream >= media.audio_stream_count {
                continue;
            }
            let source_time = c.cut_to_src(at).unwrap_or(c.src_start_ms);
            let voice = Voice {
                track: t.id,
                stream,
                gain: c.gain * t.gain,
            };
            let shared = groups.iter_mut().find(|g| {
                g.path == path
                    && g.source_time == source_time
                    && (g.clip.speed - c.speed).abs() < 0.0001
                    && !g.voices.iter().any(|v| v.stream == stream)
            });
            match shared {
                Some(group) => group.voices.push(voice),
                None => groups.push(AudioGroup {
                    lead: t.id,
                    clip: c,
                    path,
                    source_time,
                    voices: vec![voice],
                }),
            }
        }
        groups
    }

    /// What every lane plays at `at`: video lanes bottom-to-top, then
    /// audio groups.
    fn wants(p: &Project, at: i64) -> Vec<Want<'_>> {
        let mut wants = Vec::new();
        for (depth, t) in p
            .tracks
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, t)| t.kind == TrackKind::Video && !t.muted)
        {
            let Some(c) = track::clip_on_track_at(&p.clips, t.id, at) else {
                continue;
            };
            let (path, media) = p.clip_source(c);
            let path = if path == p.source {
                p.proxy.as_deref().unwrap_or(path)
            } else {
                path
            };
            wants.push(Want {
                key: t.id,
                clip: c,
                path,
                source_time: c.cut_to_src(at).unwrap_or(c.src_start_ms),
                video: Some((depth, media.width, media.height)),
                voices: Vec::new(),
            });
        }
        wants.extend(Self::audio_groups(p, at).into_iter().map(|g| Want {
            key: g.lead,
            clip: g.clip,
            path: g.path,
            source_time: g.source_time,
            video: None,
            voices: g.voices,
        }));
        wants
    }

    /// Creates a decoder for a lane.
    fn new_slot(clip: ClipId, audio: bool, ctx: &egui::Context) -> Option<Slot> {
        let result = if audio {
            Player::new_audio(ctx.clone())
        } else {
            Player::new_layer(ctx.clone()).and_then(|player| {
                player.ensure_render_context()?;
                Ok(player)
            })
        };
        match result {
            Ok(player) => Some(Slot::new(player, clip)),
            Err(e) => {
                log::error!("Live layer initialization failed: {e:#}");
                None
            }
        }
    }

    /// Points `slot` at `clip`'s media and keeps it aligned with the clock.
    /// Returns false when the media could not be loaded.
    fn align(
        slot: &mut Slot,
        path: &std::path::Path,
        clip: &Clip,
        source_time: i64,
        playing: bool,
        epoch: u64,
    ) -> bool {
        let reload = slot.path != path;
        if reload {
            if let Err(e) = slot.player.set_file(path, source_time) {
                log::error!("Live layer load: {e:#}");
                return false;
            }
            slot.path = path.to_path_buf();
            slot.seek = source_time;
            slot.corrected = Instant::now();
            slot.epoch = epoch;
            slot.aid = None;
            slot.graph.clear();
        } else if slot.clip != clip.id {
            // Next piece of the same file without a standby decoder ready:
            // seek in the open decoder instead of reloading the file.
            let continuous =
                playing && (slot.player.state().estimated_ms(slot.speed) - source_time).abs() <= 40;
            if !continuous && slot.seek != source_time {
                let _ = slot.player.seek(source_time);
                slot.seek = source_time;
                slot.corrected = Instant::now();
            }
            slot.epoch = epoch;
        }
        slot.clip = clip.id;
        if (slot.speed - clip.speed).abs() > 0.0001 {
            let _ = slot.player.set_speed(clip.speed as f64);
            slot.speed = clip.speed;
        }
        if !playing {
            if slot.seek != source_time {
                let _ = slot.player.seek(source_time);
                slot.seek = source_time;
            }
            slot.epoch = epoch;
        } else if !reload {
            let drift = (slot.player.state().estimated_ms(slot.speed) - source_time).abs();
            // A clock jump re-aligns at once; ordinary drift waits for the
            // last accurate seek to finish decoding, because re-seeking to
            // a keyframe every frame starves a long-GOP decoder.
            let jumped = slot.epoch != epoch;
            if (jumped && drift > 40)
                || (drift > DRIFT_MS && slot.corrected.elapsed() > Duration::from_millis(600))
            {
                let _ = slot.player.seek(source_time);
                slot.seek = source_time;
                slot.corrected = Instant::now();
            }
            slot.epoch = epoch;
        }
        if reload || slot.playing != playing || slot.player.state().paused == playing {
            let _ = slot.player.set_paused(!playing);
            slot.playing = playing;
        }
        true
    }

    /// Audio routing for a slot: a lone stream plays through `aid` with its
    /// gain folded into mpv's cubic volume; several mix in one graph.
    fn route_audio(&self, slot: &mut Slot, voices: &[Voice], volume: f64, muted: bool) {
        let audible: Vec<Voice> = voices
            .iter()
            .copied()
            .filter(|v| !self.monitor_muted.contains(&v.track))
            .collect();
        let (graph, aid, gain) = match audible.as_slice() {
            [] => (String::new(), slot.aid, 1.0),
            [one] => (String::new(), Some(one.stream), one.gain),
            many => (mix_graph(many), None, 1.0),
        };
        if graph != slot.graph {
            let _ = slot.player.set_lavfi_complex(&graph);
            slot.graph = graph;
        }
        match aid {
            Some(aid) if slot.aid != Some(aid) => {
                let _ = slot.player.set_audio_stream(aid);
                slot.aid = Some(aid);
            }
            Some(_) => {}
            None => slot.aid = None,
        }
        let mute = muted || audible.is_empty();
        if slot.muted != Some(mute) {
            let _ = slot.player.set_mute(mute);
            slot.muted = Some(mute);
        }
        let volume = volume * f64::from(gain.max(0.0)).cbrt();
        if (slot.volume - volume).abs() > 0.001 {
            let _ = slot.player.set_volume(volume);
            slot.volume = volume;
        }
    }

    pub fn sync(&mut self, p: &Project, ctx: &egui::Context, volume: f64, muted: bool) {
        let mut active = HashSet::new();
        let (playing, epoch) = (self.playing, self.epoch);
        self.frames.clear();
        self.warm.clear();
        for want in Self::wants(p, p.playhead_ms) {
            // The standby decoder already sits on this clip: take it.
            let stale = self
                .slots
                .get(&want.key)
                .is_none_or(|s| s.clip != want.clip.id);
            let standby_ready = self
                .standby
                .get(&want.key)
                .is_some_and(|s| s.clip == want.clip.id && s.path == want.path);
            if let Some(ready) = (stale && standby_ready)
                .then(|| self.standby.remove(&want.key))
                .flatten()
            {
                if let Some(old) = self.slots.insert(want.key, ready) {
                    let _ = old.player.set_paused(true);
                    self.standby.insert(
                        want.key,
                        Slot {
                            playing: false,
                            ..old
                        },
                    );
                }
            }
            let mut slot = match self.slots.remove(&want.key) {
                Some(slot) => slot,
                None => match Self::new_slot(want.clip.id, want.video.is_none(), ctx) {
                    Some(slot) => slot,
                    None => continue,
                },
            };
            active.insert(want.key);
            if Self::align(
                &mut slot,
                want.path,
                want.clip,
                want.source_time,
                playing,
                epoch,
            ) {
                match want.video {
                    Some((depth, width, height)) => {
                        if slot.muted != Some(true) {
                            let _ = slot.player.set_mute(true);
                            slot.muted = Some(true);
                        }
                        let c = want.clip;
                        let (transform, flash) = c.animation.sample(
                            c.transform,
                            p.playhead_ms - c.cut_start_ms,
                            c.duration_ms(),
                        );
                        self.frames.push(FrameLayer {
                            id: c.id,
                            player: slot.player.clone(),
                            fbo: slot.fbo.clone(),
                            width,
                            height,
                            transform,
                            flash,
                            depth,
                        });
                    }
                    None => self.route_audio(&mut slot, &want.voices, volume, muted),
                }
            }
            self.slots.insert(want.key, slot);
        }
        for (id, slot) in &mut self.slots {
            if !active.contains(id) && slot.playing {
                let _ = slot.player.set_paused(true);
                slot.playing = false;
            }
        }
        self.prepare_standby(p, ctx, volume, muted);
        if trace_enabled() {
            self.trace(p);
        }
        // Keep only occupied timeline lanes; inactive decoders stay cached for scrubbing.
        let gone = |id: &TrackId| !p.tracks.iter().any(|t| t.id == *id);
        let removed: Vec<_> = self.slots.keys().copied().filter(gone).collect();
        for id in removed {
            if let Some(slot) = self.slots.remove(&id) {
                self.retired.lock().unwrap().push(slot.fbo);
            }
        }
        let removed: Vec<_> = self.standby.keys().copied().filter(gone).collect();
        for id in removed {
            if let Some(slot) = self.standby.remove(&id) {
                self.retired.lock().unwrap().push(slot.fbo);
            }
        }
    }

    /// One log line per decoder: which clip, playing, audible, and how far
    /// its position is from where the clock says it should be. Enabled by
    /// `HALO_CLIPPER_TRACE_LIVE=1`; for checking sync in test rigs.
    fn trace(&self, p: &Project) {
        let mut line = format!("live t={} playing={}", p.playhead_ms, self.playing);
        for (role, slots) in [("cur", &self.slots), ("standby", &self.standby)] {
            let mut keys: Vec<_> = slots.keys().copied().collect();
            keys.sort();
            for key in keys {
                let slot = &slots[&key];
                let state = slot.player.state();
                let expected = track::clip_by_id(&p.clips, slot.clip)
                    .and_then(|c| c.cut_to_src(p.playhead_ms));
                let drift = expected.map(|e| state.estimated_ms(slot.speed) - e);
                line.push_str(&format!(
                    " | {role} k{} c{} audio={} paused={} muted={:?} graph={} drift={:?}",
                    key.0,
                    slot.clip.0,
                    slot.aid.is_some() || !slot.graph.is_empty(),
                    state.paused,
                    slot.muted,
                    !slot.graph.is_empty(),
                    drift,
                ));
            }
        }
        log::info!("{line}");
    }

    /// Parks a second decoder on each lane's next cut, paused on its exact
    /// first frame, so playback crosses the cut by swapping decoders
    /// instead of reloading or seeking (which stalls for a GOP's decode).
    fn prepare_standby(&mut self, p: &Project, ctx: &egui::Context, volume: f64, muted: bool) {
        let at = p.playhead_ms;
        let mut cuts: Vec<i64> = p
            .clips
            .iter()
            .map(|c| c.cut_start_ms)
            .filter(|start| *start > at && *start - at <= LOOKAHEAD_MS)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut wanted = HashSet::new();
        for cut in cuts {
            for want in Self::wants(p, cut) {
                let current = self.slots.get(&want.key).map(|s| s.clip);
                if current == Some(want.clip.id) || !wanted.insert(want.key) {
                    continue;
                }
                let mut slot = match self.standby.remove(&want.key) {
                    Some(slot) => slot,
                    None => match Self::new_slot(want.clip.id, want.video.is_none(), ctx) {
                        Some(slot) => slot,
                        None => continue,
                    },
                };
                // Paused alignment loads/seeks to the clip's first frame.
                if Self::align(
                    &mut slot,
                    want.path,
                    want.clip,
                    want.source_time,
                    false,
                    self.epoch,
                ) {
                    if let Some((_, width, height)) = want.video {
                        if slot.muted != Some(true) {
                            let _ = slot.player.set_mute(true);
                            slot.muted = Some(true);
                        }
                        self.warm.push(WarmLayer {
                            player: slot.player.clone(),
                            fbo: slot.fbo.clone(),
                            width,
                            height,
                        });
                    } else {
                        self.route_audio(&mut slot, &want.voices, volume, muted);
                    }
                }
                self.standby.insert(want.key, slot);
            }
        }
    }
    /// Composites `frames[range]` onto the canvas. The `first` call of a
    /// frame also renders every decoder offscreen and clears the canvas;
    /// later calls stack more layers on top, so other content (text) can
    /// be painted between video lanes.
    pub fn paint_range(
        &self,
        ui: &egui::Ui,
        canvas: egui::Rect,
        background: crate::framing::Background,
        range: std::ops::Range<usize>,
        first: bool,
    ) {
        let layers = self.frames.clone();
        let warm = self.warm.clone();
        let gpu = self.gpu.clone();
        let retired = self.retired.clone();
        ui.painter().add(egui::PaintCallback {
            rect: canvas,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| unsafe {
                if first {
                    for fbo in retired.lock().unwrap().drain(..) {
                        if let Some(mut fbo) = fbo.lock().unwrap().take() {
                            fbo.destroy(painter.gl());
                        }
                    }
                }
                paint_layers(
                    &layers,
                    range.clone(),
                    first,
                    &warm,
                    &gpu,
                    canvas,
                    background,
                    info,
                    painter,
                );
            })),
        });
    }
    pub fn destroy(&mut self, gl: &glow::Context) {
        for slot in self.slots.values().chain(self.standby.values()) {
            if let Some(mut fbo) = slot.fbo.lock().unwrap().take() {
                fbo.destroy(gl);
            }
        }
        for fbo in self.retired.lock().unwrap().drain(..) {
            if let Some(mut fbo) = fbo.lock().unwrap().take() {
                fbo.destroy(gl);
            }
        }
        if let Some(gpu) = self.gpu.lock().unwrap().take() {
            unsafe {
                gl.delete_program(gpu.program);
                gl.delete_program(gpu.blur_program);
                gl.delete_vertex_array(gpu.vao);
            }
        }
        self.clear();
    }
}
struct CompositeGpu {
    program: glow::NativeProgram,
    blur_program: glow::NativeProgram,
    vao: glow::NativeVertexArray,
}
const VERTEX: &str = r#"#version 330 core
out vec2 uv;
void main() {
    vec2 p=vec2((gl_VertexID==1||gl_VertexID==3)?1.0:-1.0,gl_VertexID>=2?1.0:-1.0);
    uv=(p+1.0)*0.5;
    gl_Position=vec4(p,0,1);
}"#;
const FOREGROUND: &str = r#"#version 330 core
in vec2 uv; out vec4 color;
uniform sampler2D video;
uniform float opacity;
uniform float flash;
void main() { color=vec4(mix(texture(video,uv).rgb,vec3(1.0),flash),opacity); }
"#;
const BLUR: &str = r#"#version 330 core
in vec2 uv; out vec4 color;
uniform sampler2D video;
uniform vec2 blur_step;
void main() {
    vec3 rgb=vec3(0); float total=0;
    for(int x=-4;x<=4;x++) {
        for(int y=-4;y<=4;y++) {
            float weight=exp(-0.125*float(x*x+y*y));
            rgb+=texture(video,clamp(uv+vec2(x,y)*blur_step*0.5,vec2(0.001),vec2(0.999))).rgb*weight;
            total+=weight;
        }
    }
    color=vec4(rgb/total,1);
}"#;
impl CompositeGpu {
    unsafe fn new(gl: &glow::Context) -> anyhow::Result<Self> {
        let program = Self::program(gl, FOREGROUND)?;
        let blur_program = match Self::program(gl, BLUR) {
            Ok(p) => p,
            Err(e) => {
                gl.delete_program(program);
                return Err(e);
            }
        };
        let vao = gl.create_vertex_array().map_err(anyhow::Error::msg)?;
        Ok(Self {
            program,
            blur_program,
            vao,
        })
    }
    unsafe fn program(gl: &glow::Context, fragment: &str) -> anyhow::Result<glow::NativeProgram> {
        let program = gl.create_program().map_err(anyhow::Error::msg)?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(kind).map_err(anyhow::Error::msg)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let error = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for compiled in shaders {
                    gl.delete_shader(compiled);
                }
                gl.delete_program(program);
                anyhow::bail!("{error}");
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            anyhow::bail!("{error}");
        }
        Ok(program)
    }
}
#[allow(clippy::too_many_arguments)]
unsafe fn paint_layers(
    layers: &[FrameLayer],
    range: std::ops::Range<usize>,
    first: bool,
    warm: &[WarmLayer],
    gpu: &Mutex<Option<CompositeGpu>>,
    canvas: egui::Rect,
    background: crate::framing::Background,
    info: egui::PaintCallbackInfo,
    painter: &egui_glow::Painter,
) {
    let gl = painter.gl();
    let vp = info.viewport_in_pixels();
    if vp.width_px <= 0 || vp.height_px <= 0 {
        return;
    }
    let draw = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
    let read = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
    let program = gl.get_parameter_i32(glow::CURRENT_PROGRAM);
    let vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
    let mut viewport = [0; 4];
    gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
    let mut scissor = [0; 4];
    gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor);
    let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
    let blend = gl.is_enabled(glow::BLEND);
    let depth = gl.is_enabled(glow::DEPTH_TEST);
    let cull = gl.is_enabled(glow::CULL_FACE);
    let active = gl.get_parameter_i32(glow::ACTIVE_TEXTURE);
    gl.active_texture(glow::TEXTURE0);
    let texture = gl.get_parameter_i32(glow::TEXTURE_BINDING_2D);
    let sampler = gl.get_parameter_i32(glow::SAMPLER_BINDING);
    let blend_args = [
        gl.get_parameter_i32(glow::BLEND_SRC_RGB),
        gl.get_parameter_i32(glow::BLEND_DST_RGB),
        gl.get_parameter_i32(glow::BLEND_SRC_ALPHA),
        gl.get_parameter_i32(glow::BLEND_DST_ALPHA),
    ];
    let mut gpu = gpu.lock().unwrap();
    if gpu.is_none() {
        match CompositeGpu::new(gl) {
            Ok(g) => *gpu = Some(g),
            Err(e) => {
                log::error!("Layer compositor: {e:#}");
                return;
            }
        }
    }
    let gpu = gpu.as_ref().unwrap();
    let pixels = vp.width_px as f32 / canvas.width();
    let background_layer = layers
        .iter()
        .find(|l| l.transform.opacity > 0.0)
        .map(|l| l.id);
    // Complete every decoder render before touching the screen. A later mpv
    // context can reset/clear GL targets while preparing its own frame.
    // Standby decoders render too (never composited), so a swapped-in
    // lane shows the cut's frame on its very first composite.
    let decoders = layers
        .iter()
        .map(|l| (&l.player, &l.fbo, l.width, l.height))
        .chain(warm.iter().map(|l| (&l.player, &l.fbo, l.width, l.height)))
        .filter(|_| first);
    for (player, fbo, width, height) in decoders {
        let ratio = (960.0 / width.max(1) as f32)
            .min(540.0 / height.max(1) as f32)
            .min(1.0);
        let w = (width as f32 * ratio).round().max(2.0) as i32;
        let h = (height as f32 * ratio).round().max(2.0) as i32;
        let mut fbo = fbo.lock().unwrap();
        VideoFbo::ensure_size(&mut fbo, gl, w, h);
        let Some(fbo) = fbo.as_ref() else { continue };
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo.fbo()));
        gl.viewport(0, 0, w, h);
        if let Err(e) = player.render(fbo.fbo_id_i32(), w, h, true) {
            log::error!("Live frame render: {e:#}");
            continue;
        }
    }
    let rgb = background.rgb();
    let mut clear_color = [0.0; 4];
    gl.get_parameter_f32_slice(glow::COLOR_CLEAR_VALUE, &mut clear_color);
    gl.bind_framebuffer(glow::FRAMEBUFFER, video_fbo::fbo_from_raw(draw));
    gl.enable(glow::SCISSOR_TEST);
    gl.scissor(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
    gl.clear_color(
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    );
    if first {
        gl.clear(glow::COLOR_BUFFER_BIT);
    }
    gl.clear_color(
        clear_color[0],
        clear_color[1],
        clear_color[2],
        clear_color[3],
    );
    // Then paint the canvas and foregrounds without re-entering mpv.
    let passes = layers
        .iter()
        .filter(|l| {
            first
                && background == crate::framing::Background::Blur
                && Some(l.id) == background_layer
        })
        .map(|l| (l, true))
        .chain(
            layers
                .get(range)
                .unwrap_or_default()
                .iter()
                .map(|l| (l, false)),
        );
    for (layer, blur) in passes {
        let transform = if blur {
            track::Transform {
                fit: crate::framing::Fit::Fill,
                ..Default::default()
            }
        } else {
            layer.transform
        };
        let rect = super::canvas::layer_rect(canvas, layer.width, layer.height, transform);
        if !rect.intersects(canvas) {
            continue;
        }
        let fbo = layer.fbo.lock().unwrap();
        let Some(fbo) = fbo.as_ref() else { continue };
        gl.bind_framebuffer(glow::FRAMEBUFFER, video_fbo::fbo_from_raw(draw));
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
        gl.viewport(
            vp.left_px + ((rect.left() - canvas.left()) * pixels).round() as i32,
            vp.from_bottom_px + ((canvas.bottom() - rect.bottom()) * pixels).round() as i32,
            (rect.width() * pixels).round().max(1.0) as i32,
            (rect.height() * pixels).round().max(1.0) as i32,
        );
        let program = if blur { gpu.blur_program } else { gpu.program };
        gl.use_program(Some(program));
        gl.bind_vertex_array(Some(gpu.vao));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_sampler(0, None);
        gl.bind_texture(glow::TEXTURE_2D, Some(fbo.texture()));
        gl.uniform_1_i32(gl.get_uniform_location(program, "video").as_ref(), 0);
        gl.uniform_1_f32(
            gl.get_uniform_location(program, "opacity").as_ref(),
            transform.opacity,
        );
        gl.uniform_1_f32(
            gl.get_uniform_location(program, "flash").as_ref(),
            if blur { 0.0 } else { layer.flash },
        );
        let sigma = if blur {
            canvas.width().min(canvas.height()) / 50.0
        } else {
            0.0
        };
        gl.uniform_2_f32(
            gl.get_uniform_location(program, "blur_step").as_ref(),
            sigma / rect.width(),
            sigma / rect.height(),
        );
        gl.enable(glow::BLEND);
        gl.blend_func_separate(
            glow::SRC_ALPHA,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    }
    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, video_fbo::fbo_from_raw(read));
    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, video_fbo::fbo_from_raw(draw));
    gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
    gl.scissor(scissor[0], scissor[1], scissor[2], scissor[3]);
    for (state, enabled) in [
        (glow::SCISSOR_TEST, scissor_enabled),
        (glow::BLEND, blend),
        (glow::DEPTH_TEST, depth),
        (glow::CULL_FACE, cull),
    ] {
        if enabled {
            gl.enable(state);
        } else {
            gl.disable(state);
        }
    }
    gl.blend_func_separate(
        blend_args[0] as u32,
        blend_args[1] as u32,
        blend_args[2] as u32,
        blend_args[3] as u32,
    );
    use std::num::NonZeroU32;
    gl.use_program(NonZeroU32::new(program as u32).map(glow::NativeProgram));
    gl.bind_vertex_array(NonZeroU32::new(vao as u32).map(glow::NativeVertexArray));
    gl.active_texture(glow::TEXTURE0);
    gl.bind_texture(
        glow::TEXTURE_2D,
        NonZeroU32::new(texture as u32).map(glow::NativeTexture),
    );
    gl.bind_sampler(0, NonZeroU32::new(sampler as u32).map(glow::NativeSampler));
    gl.active_texture(active as u32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_streams_keep_their_gain_and_share_the_export_limiter() {
        let voices = [
            Voice {
                track: TrackId(1),
                stream: 0,
                gain: 1.0,
            },
            Voice {
                track: TrackId(2),
                stream: 1,
                gain: 0.5,
            },
        ];
        assert_eq!(
            mix_graph(&voices),
            "[aid1]volume=1.000[v0];[aid2]volume=0.500[v1];[v0][v1]amix=inputs=2:normalize=0:dropout_transition=0,alimiter=limit=0.95:level=0:attack=0.1[ao]"
        );
    }
}
