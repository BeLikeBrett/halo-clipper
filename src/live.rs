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
    caption: String,
    speed: f32,
    volume: f64,
    muted: Option<bool>,
    corrected: Instant,
}
#[derive(Clone)]
pub(super) struct FrameLayer {
    pub id: ClipId,
    pub player: Arc<Player>,
    pub fbo: FrameBuffer,
    pub width: u32,
    pub height: u32,
    pub transform: track::Transform,
}
#[derive(Default)]
pub(super) struct LivePreview {
    slots: HashMap<TrackId, Slot>,
    pub playing: bool,
    anchor_ms: i64,
    anchor: Option<Instant>,
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
        self.anchor_ms = ms;
        self.anchor = self.playing.then(Instant::now);
    }
    pub fn pause(&mut self) {
        self.anchor_ms = self.position();
        self.playing = false;
        self.anchor = None;
    }
    pub fn toggle(&mut self, ms: i64) {
        self.playing = !self.playing;
        self.anchor_ms = ms;
        self.anchor = self.playing.then(Instant::now);
    }
    pub fn clear(&mut self) {
        self.pause();
        for slot in self.slots.values() {
            let _ = slot.player.set_paused(true);
        }
        self.retired
            .lock()
            .unwrap()
            .extend(self.slots.values().map(|s| s.fbo.clone()));
        self.slots.clear();
        self.frames.clear();
    }
    pub fn sync(&mut self, p: &Project, ctx: &egui::Context, volume: f64, muted: bool) {
        let mut active = HashSet::new();
        self.frames.clear();
        for t in p.tracks.iter().rev().filter(|t| !t.muted) {
            let Some(c) = track::clip_on_track_at(&p.clips, t.id, p.playhead_ms) else {
                continue;
            };
            let (path, media) = p.clip_source(c);
            let audio = t.kind == TrackKind::Audio;
            if audio
                && c.audio_stream.or(t.source_audio_index).unwrap_or(0) >= media.audio_stream_count
            {
                continue;
            }
            let path = if !audio && path == p.source {
                p.proxy.as_deref().unwrap_or(path)
            } else {
                path
            };
            active.insert(t.id);
            if let std::collections::hash_map::Entry::Vacant(entry) = self.slots.entry(t.id) {
                let result = if audio {
                    Player::new_audio(ctx.clone())
                } else {
                    Player::new_layer(ctx.clone())
                };
                match result.and_then(|player| {
                    if !audio {
                        player.ensure_render_context()?;
                    }
                    Ok(player)
                }) {
                    Ok(player) => {
                        entry.insert(Slot {
                            player: Arc::new(player),
                            fbo: Arc::new(Mutex::new(None)),
                            path: PathBuf::new(),
                            clip: c.id,
                            seek: -1,
                            playing: false,
                            caption: String::new(),
                            speed: 0.0,
                            volume: -1.0,
                            muted: None,
                            corrected: Instant::now(),
                        });
                    }
                    Err(e) => {
                        log::error!("Live layer initialization failed: {e:#}");
                        continue;
                    }
                }
            }
            let slot = self.slots.get_mut(&t.id).unwrap();
            let source_time = c.cut_to_src(p.playhead_ms).unwrap_or(c.src_start_ms);
            let changed = slot.path != path || slot.clip != c.id;
            if changed {
                if let Err(e) = slot.player.set_file(path, source_time) {
                    log::error!("Live layer load: {e:#}");
                    continue;
                }
                slot.path = path.to_path_buf();
                slot.clip = c.id;
                slot.seek = source_time;
                slot.corrected = Instant::now();
                if !slot.caption.is_empty() {
                    let _ = slot.player.set_video_filter("");
                }
                slot.caption = String::new();
                if audio {
                    let index = c.audio_stream.or(t.source_audio_index).unwrap_or(0);
                    let _ = slot.player.set_audio_stream(index);
                }
            }
            if (slot.speed - c.speed).abs() > 0.0001 {
                let _ = slot.player.set_speed(c.speed as f64);
                slot.speed = c.speed;
            }
            if !self.playing {
                if slot.seek != source_time {
                    let _ = slot.player.seek(source_time);
                    slot.seek = source_time;
                }
            } else if (slot.player.state().position_ms - source_time).abs() > 180
                && !changed
                && slot.corrected.elapsed() > Duration::from_millis(750)
            {
                // Give accurate seeks time to decode. Re-seeking to a keyframe
                // each UI frame prevents a long-GOP decoder from catching up.
                let _ = slot.player.seek(source_time);
                slot.seek = source_time;
                slot.corrected = Instant::now();
            }
            if changed || slot.playing != self.playing || slot.player.state().paused == self.playing
            {
                let _ = slot.player.set_paused(!self.playing);
                slot.playing = self.playing;
            }
            let mute = !audio || muted || self.monitor_muted.contains(&t.id);
            if slot.muted != Some(mute) {
                let _ = slot.player.set_mute(mute);
                slot.muted = Some(mute);
            }
            let volume = volume * c.gain as f64 * t.gain as f64;
            if audio {
                if (slot.volume - volume).abs() > 0.001 {
                    let _ = slot.player.set_volume(volume);
                    slot.volume = volume;
                }
            } else {
                let caption = p
                    .text_overlays
                    .get(&c.id)
                    .filter(|o| !o.text.trim().is_empty())
                    .map(|o| format!("lavfi=[{}]", export::build_drawtext(o, 1.0)))
                    .unwrap_or_default();
                if caption != slot.caption {
                    if let Err(e) = slot.player.set_video_filter(&caption) {
                        log::warn!("Live caption: {e:#}");
                    }
                    slot.caption = caption;
                }
                self.frames.push(FrameLayer {
                    id: c.id,
                    player: slot.player.clone(),
                    fbo: slot.fbo.clone(),
                    width: media.width,
                    height: media.height,
                    transform: c.transform,
                });
            }
        }
        for (id, slot) in &mut self.slots {
            if !active.contains(id) && slot.playing {
                let _ = slot.player.set_paused(true);
                slot.playing = false;
            }
        }
        // Keep only occupied timeline lanes; inactive decoders stay cached for scrubbing.
        let removed: Vec<_> = self
            .slots
            .keys()
            .filter(|id| !p.tracks.iter().any(|t| t.id == **id))
            .copied()
            .collect();
        for id in removed {
            if let Some(slot) = self.slots.remove(&id) {
                self.retired.lock().unwrap().push(slot.fbo);
            }
        }
    }
    pub fn paint(&self, ui: &egui::Ui, canvas: egui::Rect, background: crate::framing::Background) {
        let layers = self.frames.clone();
        let gpu = self.gpu.clone();
        let retired = self.retired.clone();
        ui.painter().add(egui::PaintCallback {
            rect: canvas,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| unsafe {
                for fbo in retired.lock().unwrap().drain(..) {
                    if let Some(mut fbo) = fbo.lock().unwrap().take() {
                        fbo.destroy(painter.gl());
                    }
                }
                paint_layers(&layers, &gpu, canvas, background, info, painter);
            })),
        });
    }
    pub fn destroy(&mut self, gl: &glow::Context) {
        for slot in self.slots.values() {
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
void main() { color=vec4(texture(video,uv).rgb,opacity); }
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
unsafe fn paint_layers(
    layers: &[FrameLayer],
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
    for layer in layers {
        let ratio = (960.0 / layer.width.max(1) as f32)
            .min(540.0 / layer.height.max(1) as f32)
            .min(1.0);
        let w = (layer.width as f32 * ratio).round().max(2.0) as i32;
        let h = (layer.height as f32 * ratio).round().max(2.0) as i32;
        let mut fbo = layer.fbo.lock().unwrap();
        VideoFbo::ensure_size(&mut fbo, gl, w, h);
        let Some(fbo) = fbo.as_ref() else { continue };
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo.fbo()));
        gl.viewport(0, 0, w, h);
        if let Err(e) = layer.player.render(fbo.fbo_id_i32(), w, h, true) {
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
    gl.clear(glow::COLOR_BUFFER_BIT);
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
            background == crate::framing::Background::Blur && Some(l.id) == background_layer
        })
        .map(|l| (l, true))
        .chain(layers.iter().map(|l| (l, false)));
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
