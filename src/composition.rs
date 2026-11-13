//! Shared compositor for layered exports and disposable, low-resolution previews.
//! Tracks are stored top first. Each clip has its own input seek, so a late cut
//! never decodes the entire recording and independently moved audio stays aligned.
use crate::{encode::EncodeSettings, text::TextItem, textrender::ExportLayer, track::Transform};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

#[derive(Clone, Debug)]
pub struct Segment {
    pub source: PathBuf,
    pub audio_index: Option<usize>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub src_start_ms: i64,
    pub src_end_ms: i64,
    pub speed: f32,
    pub gain: f32,
    pub transform: Transform,
    pub animation: crate::animation::Animation,
}
#[derive(Clone, Debug)]
pub struct Composition {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: i64,
    pub background: crate::framing::Background,
    /// Video in bottom-to-top paint order, then audio in any order.
    pub segments: Vec<Segment>,
    /// Text layers, painted above all video in list order.
    pub texts: Vec<TextItem>,
}
impl Composition {
    /// Output frame size and rate for `encode`.
    pub fn output_format(&self, encode: &EncodeSettings) -> (u32, u32, f64) {
        let (w, h) = encode.resolution.unwrap_or((self.width, self.height));
        let fps = encode
            .fps_limit
            .map(f64::from)
            .unwrap_or(self.fps)
            .clamp(1.0, 120.0);
        ((w / 2 * 2).max(2), (h / 2 * 2).max(2), fps)
    }

    /// Builds the ffmpeg command. `texts` are this composition's text
    /// layers already rendered at the output size
    /// (`textrender::write_export_layers`).
    pub fn command(
        &self,
        destination: &std::path::Path,
        encode: &EncodeSettings,
        texts: &[ExportLayer],
    ) -> anyhow::Result<Command> {
        anyhow::ensure!(
            self.duration_ms > 0,
            "Nothing to export — the timeline is empty."
        );
        let (w, h, fps) = self.output_format(encode);
        let duration = self.duration_ms as f64 / 1000.0;
        let mut cmd = crate::platform::command("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-progress",
            "pipe:1",
            "-nostats",
            "-filter_complex_threads",
            "2",
        ]);
        if encode.video_codec.needs_vaapi_upload() {
            cmd.args([
                "-init_hw_device",
                &format!("vaapi=vahw:{}", super::export::default_vaapi_device()),
                "-filter_hw_device",
                "vahw",
            ]);
        }
        let segments: Vec<_> = self
            .segments
            .iter()
            .filter(|s| s.src_end_ms > s.src_start_ms && s.end_ms > s.start_ms)
            .collect();
        for s in &segments {
            cmd.args([
                "-threads",
                "2",
                "-ss",
                &format!("{:.6}", s.src_start_ms as f64 / 1000.0),
                "-t",
                &format!("{:.6}", (s.src_end_ms - s.src_start_ms) as f64 / 1000.0),
            ])
            .arg("-i")
            .arg(&s.source);
        }
        for t in texts {
            cmd.args(["-f", "concat", "-safe", "0", "-i"]).arg(&t.list);
        }
        let mut graph =
            format!("color=c=0x{:02x}{:02x}{:02x}:s={w}x{h}:r={fps:.6}:d={duration:.6},format=yuv420p[base];", self.background.rgb()[0], self.background.rgb()[1], self.background.rgb()[2]);
        let mut current = "base".to_string();
        let mut audio = Vec::new();
        if self.background == crate::framing::Background::Blur {
            for (i, s) in segments
                .iter()
                .enumerate()
                .filter(|(_, s)| s.audio_index.is_none())
            {
                let speed = s.speed.clamp(0.25, 4.0);
                let start = s.start_ms as f64 / 1000.0;
                graph.push_str(&format!("[{i}:v:0]split=2[src{i}][back{i}];[back{i}]setpts=(PTS-STARTPTS)/{speed:.6},scale={w}:{h}:force_original_aspect_ratio=increase:force_divisible_by=2,crop={w}:{h},setsar=1,fps={fps:.6},gblur=sigma={:.3},setpts=PTS+{start:.6}/TB[bg{i}];", w.min(h) as f32 / 50.0));
            }
            // Paint backgrounds front-to-back so the lowest active video wins,
            // including when the bottom lane has a gap.
            for (i, s) in segments
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, s)| s.audio_index.is_none())
            {
                let start = s.start_ms as f64 / 1000.0;
                let end = s.end_ms as f64 / 1000.0;
                graph.push_str(&format!("[{current}][bg{i}]overlay=eof_action=pass:repeatlast=0:enable='gte(t,{start:.6})*lt(t,{end:.6})*{}'[canvas{i}];", if s.transform.opacity > 0.0 {1} else {0}));
                current = format!("canvas{i}");
            }
        }

        for (i, s) in segments.iter().enumerate() {
            let start = s.start_ms as f64 / 1000.0;
            let end = s.end_ms as f64 / 1000.0;
            let speed = s.speed.clamp(0.25, 4.0);
            if let Some(idx) = s.audio_index {
                graph.push_str(&format!("[{i}:a:{idx}]asetpts=PTS-STARTPTS,{},volume={:.6},aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,adelay={}:all=1[a{i}];", super::export::atempo_chain(speed), s.gain.max(0.0), s.start_ms));
                audio.push(format!("[a{i}]"));
            } else {
                let t = s.transform;
                let sw = ((w as f32 * t.scale.clamp(0.03, 8.0)) as u32 / 2 * 2).max(2);
                let sh = ((h as f32 * t.scale.clamp(0.03, 8.0)) as u32 / 2 * 2).max(2);
                let input = if self.background == crate::framing::Background::Blur {
                    format!("[src{i}]")
                } else {
                    format!("[{i}:v:0]")
                };
                let fit = if t.fit == crate::framing::Fit::Fill {
                    "increase"
                } else {
                    "decrease"
                };
                graph.push_str(&format!("{input}setpts=(PTS-STARTPTS)/{speed:.6},scale=w={sw}:h={sh}:force_original_aspect_ratio={fit}:force_divisible_by=2,setsar=1,fps={fps:.6},format=rgba"));
                graph.push_str(&s.animation.filters(s.end_ms - s.start_ms));
                let dx = s.animation.x_expression(s.start_ms, s.end_ms - s.start_ms);
                graph.push_str(&format!(",colorchannelmixer=aa={:.6},setpts=PTS+{start:.6}/TB[v{i}];[{current}][v{i}]overlay=x='W*{:.6}-w/2+W*({dx})':y='H*{:.6}-h/2':eof_action=pass:repeatlast=0:enable='gte(t,{start:.6})*lt(t,{end:.6})'[mix{i}];", t.opacity.clamp(0.0,1.0), t.x, t.y));
                current = format!("mix{i}");
            }
        }
        for (k, t) in texts.iter().enumerate() {
            let input = segments.len() + k;
            let start = t.start_ms as f64 / 1000.0;
            let end = t.end_ms as f64 / 1000.0;
            graph.push_str(&format!(
                "[{input}:v]format=rgba,setpts=PTS-STARTPTS+{start:.6}/TB[text{k}];[{current}][text{k}]overlay=x={}:y={}:eof_action=pass:repeatlast=0:enable='gte(t,{start:.6})*lt(t,{end:.6})'[titled{k}];",
                t.x, t.y,
            ));
            current = format!("titled{k}");
        }
        let bridge = if encode.video_codec.needs_vaapi_upload() {
            "format=nv12,hwupload"
        } else {
            "format=yuv420p"
        };
        graph.push_str(&format!("[{current}]{bridge}[vout]"));
        if !audio.is_empty() {
            // A silent bed preserves leading/trailing gaps. No normalization:
            // independent layer gain remains stable when another layer ends.
            graph.push_str(&format!(";anullsrc=r=48000:cl=stereo,atrim=duration={duration:.6}[silence];[silence]{}amix=inputs={}:normalize=0:duration=longest:dropout_transition=0,alimiter=limit=0.95:level=0:attack=0.1,atrim=duration={duration:.6}[aout]",audio.concat(),audio.len()+1));
        }
        cmd.args(["-filter_complex", &graph, "-map", "[vout]"]);
        if audio.is_empty() {
            cmd.arg("-an");
        } else {
            cmd.args(["-map", "[aout]"])
                .args(encode.audio_args())
                .args(super::export::audio_container_args(encode, destination));
        }
        cmd.args(encode.video_args()).args([
            "-t",
            &format!("{duration:.6}"),
            "-movflags",
            "+faststart",
        ]);
        if let Some(fmt) = super::export::output_format_for(destination) {
            cmd.args(["-f", fmt]);
        }
        cmd.arg(destination)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        encode::EncodeSettings,
        export::{self, ExportPhase, ExportPlan},
        track::Transform,
    };
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    fn dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "halo-compositor-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn video(dir: &std::path::Path, name: &str, color: &str) -> PathBuf {
        let path = dir.join(name);
        let out = crate::platform::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("color=c={color}:s=64x64:r=24:d=3"),
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        path
    }
    fn segment(path: PathBuf, start: i64, end: i64) -> Segment {
        Segment {
            source: path,
            audio_index: None,
            start_ms: start,
            end_ms: end,
            src_start_ms: 0,
            src_end_ms: end - start,
            speed: 1.0,
            gain: 1.0,
            transform: Transform::default(),
            animation: Default::default(),
        }
    }
    fn render(c: Composition, path: PathBuf) -> PathBuf {
        let mut encode = EncodeSettings::default_for_source(c.width, c.height);
        encode.video_preset = "ultrafast".into();
        let h = export::spawn(ExportPlan::Layered {
            composition: c,
            destination: path.clone(),
            encode,
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match h.progress().phase {
                ExportPhase::Done => break,
                ExportPhase::Running => {}
                other => panic!("{other:?}"),
            };
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        path
    }
    fn pixel(path: &std::path::Path, time: &str, x: usize, y: usize) -> [u8; 3] {
        let out = crate::platform::command("ffmpeg")
            .args(["-v", "error", "-ss", time, "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let width = crate::media::probe(path).unwrap().width as usize;
        let index = (y * width + x) * 3;
        out.stdout[index..index + 3].try_into().unwrap()
    }
    #[test]
    fn clip_animations_export_on_both_edges_of_a_delayed_segment() {
        use crate::animation::{Animation, Effect};
        let d = dir();
        let red = video(&d, "effect-red.mp4", "red");
        let blue = video(&d, "effect-blue.mp4", "blue");
        for effect in Effect::ALL.into_iter().filter(|e| *e != Effect::None) {
            let mut top = segment(red.clone(), 1000, 3000);
            top.animation = Animation {
                intro: effect,
                outro: effect,
                in_ms: 500.0,
                out_ms: 300.0,
                amount: 1.0,
            };
            let mut bottom = segment(blue.clone(), 0, 4000);
            bottom.src_end_ms = 2000;
            bottom.speed = 0.5;
            let c = Composition {
                width: 64,
                height: 64,
                fps: 30.0,
                duration_ms: 4000,
                background: Default::default(),
                segments: vec![bottom, top],
                texts: Vec::new(),
            };
            let path = render(c, d.join(format!("effect-{effect:?}.mp4")));
            // Layer before/after, settled middle, and no lingering white wash.
            assert!(pixel(&path, "0.5", 32, 32)[2] > 200, "{effect:?}");
            assert!(pixel(&path, "2.0", 32, 32)[0] > 200, "{effect:?}");
            assert!(pixel(&path, "3.5", 32, 32)[2] > 200, "{effect:?}");
            let start = pixel(&path, "1.0", 32, 32);
            let end = pixel(&path, "2.96", 32, 32);
            match effect {
                Effect::Flash => {
                    assert!(start.iter().all(|v| *v > 220), "{start:?}");
                    assert!(end[1] > 150, "{end:?}");
                }
                Effect::Fade | Effect::PullBack | Effect::Whip => {
                    assert!(start[2] > 200 && start[0] < 30, "{effect:?}: {start:?}");
                    assert!(end[2] > end[0], "{effect:?}: {end:?}");
                }
                _ => {}
            }
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn text_layers_burn_in_at_their_position_and_time() {
        use crate::text::{TextId, TextItem};
        let d = dir();
        let black = video(&d, "text_base.mp4", "black");
        // A white box with white text: solid, so the probe pixel at the
        // layer's center is unambiguous.
        let mut title = TextItem::new(TextId(1), 1000, 2000);
        title.text = "Hi".into();
        title.size = 0.3;
        title.x = 0.75;
        title.y = 0.75;
        title.style.color = [255, 255, 255];
        title.style.background = Some(([255, 255, 255], 1.0));
        title.style.shadow = crate::text::Shadow::None;
        title.intro = crate::text::Intro::None;
        title.outro = crate::text::Outro::None;
        let c = Composition {
            width: 64,
            height: 64,
            fps: 30.0,
            duration_ms: 3000,
            background: Default::default(),
            segments: vec![segment(black, 0, 3000)],
            texts: vec![title],
        };
        let path = render(c, d.join("text_out.mp4"));
        let lit = |time, x, y| pixel(&path, time, x, y).iter().all(|v| *v > 200);
        assert!(!lit("0.5", 48, 48), "text before its start");
        assert!(lit("1.5", 48, 48), "boxed text missing at its center");
        assert!(!lit("1.5", 16, 16), "text drawn away from its position");
        assert!(!lit("2.5", 48, 48), "text after its end");
    }

    #[test]
    fn layers_blend_in_order_and_reveal_base_after_end() {
        let d = dir();
        let red = video(&d, "red.mp4", "red");
        let blue = video(&d, "blue.mp4", "blue");
        let base = segment(red, 0, 3000);
        let mut top = segment(blue, 1000, 2000);
        top.transform.scale = 0.5;
        top.transform.opacity = 0.5;
        let c = Composition {
            width: 64,
            height: 64,
            fps: 30.0,
            duration_ms: 3000,
            background: Default::default(),
            segments: vec![base, top],
            texts: Vec::new(),
        };
        let path = render(c.clone(), d.join("out.mp4"));
        let before = pixel(&path, "0.5", 32, 32);
        let center = pixel(&path, "1.5", 32, 32);
        let corner = pixel(&path, "1.5", 2, 2);
        let after = pixel(&path, "2.5", 32, 32);
        assert!(before[0] > 200 && before[2] < 30, "{before:?}");
        assert!(center[0] > 90 && center[2] > 90, "{center:?}");
        assert!(corner[0] > 200 && corner[2] < 30, "{corner:?}");
        assert!(after[0] > 200 && after[2] < 30, "{after:?}");
        let mut reversed = c;
        reversed.segments.reverse();
        let path = render(reversed, d.join("reverse.mp4"));
        let center = pixel(&path, "1.5", 32, 32);
        assert!(center[0] > 200 && center[2] < 30, "{center:?}");
        let _ = std::fs::remove_dir_all(d);
    }
    #[test]
    fn portrait_fit_fill_zoom_and_backgrounds_are_exported_and_gaps_stay_empty() {
        let d = dir();
        let source = d.join("split-colors.mp4");
        let result = crate::platform::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=128x64:r=30:d=1,drawbox=x=64:y=0:w=64:h=64:color=blue:t=fill",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(result.status.success());
        let c = Composition {
            width: 64,
            height: 128,
            fps: 30.0,
            duration_ms: 2000,
            background: Default::default(),
            segments: vec![segment(source, 0, 1000)],
            texts: Vec::new(),
        };
        let fit = render(c.clone(), d.join("fit.mp4"));
        assert!(pixel(&fit, "0.5", 5, 5).iter().all(|v| *v < 20));
        assert!(pixel(&fit, "0.5", 5, 64)[0] > 200);
        let mut colored = c.clone();
        colored.background = crate::framing::Background::Color([0, 180, 0]);
        let path = render(colored, d.join("color.mp4"));
        assert!(pixel(&path, "0.5", 5, 5)[1] > 140);
        let mut blurred = c.clone();
        blurred.background = crate::framing::Background::Blur;
        let path = render(blurred, d.join("blur.mp4"));
        let seam = pixel(&path, "0.5", 31, 5);
        assert!(seam[0] > 25 && seam[2] > 25, "{seam:?}");
        assert!(pixel(&path, "1.5", 5, 5).iter().all(|v| *v < 20));
        let mut filled = c.clone();
        filled.segments[0].transform.fit = crate::framing::Fit::Fill;
        let path = render(filled, d.join("fill.mp4"));
        assert!(pixel(&path, "0.5", 5, 5)[0] > 200);
        let mut zoomed = c;
        zoomed.segments[0].transform.scale = 4.0;
        let path = render(zoomed, d.join("zoom.mp4"));
        assert!(pixel(&path, "0.5", 5, 5)[0] > 200);
        let info = crate::media::probe(&path).unwrap();
        assert_eq!((info.width, info.height), (64, 128));
        let _ = std::fs::remove_dir_all(d);
    }
    #[test]
    fn blur_uses_lowest_active_video_and_falls_back_through_gaps() {
        let d = dir();
        let red = video(&d, "red.mp4", "red");
        let blue = video(&d, "blue.mp4", "blue");
        let base = segment(red, 0, 1000);
        let top = segment(blue, 0, 2000);
        let c = Composition {
            width: 64,
            height: 128,
            fps: 30.0,
            duration_ms: 3000,
            background: crate::framing::Background::Blur,
            segments: vec![base, top],
            texts: Vec::new(),
        };
        let path = render(c, d.join("fallback.mp4"));
        assert!(pixel(&path, "0.5", 4, 4)[0] > 200); // behind the blue foreground, base remains the canvas
        assert!(pixel(&path, "1.5", 4, 4)[2] > 200); // base ended; use the remaining video
        assert!(pixel(&path, "2.5", 4, 4).iter().all(|v| *v < 20));
        let _ = std::fs::remove_dir_all(d);
    }
    #[test]
    fn moved_audio_obeys_gain_speed_and_silence_gaps() {
        let d = dir();
        let wav = d.join("tone.wav");
        let out = crate::platform::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=2",
            ])
            .arg(&wav)
            .output()
            .unwrap();
        assert!(out.status.success());
        let mut s = segment(wav, 1000, 2000);
        s.audio_index = Some(0);
        s.src_end_ms = 2000;
        s.speed = 2.0;
        s.gain = 0.5;
        let c = Composition {
            width: 64,
            height: 64,
            fps: 30.0,
            duration_ms: 3000,
            background: Default::default(),
            segments: vec![s],
            texts: Vec::new(),
        };
        let path = render(c, d.join("audio.mp4"));
        let out = crate::platform::command("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "-"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let samples: Vec<f32> = out
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let rms = |a: usize, b: usize| {
            (samples[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a) as f32).sqrt()
        };
        assert!(rms(4800, 24000) < 0.001);
        assert!(rms(57600, 86400) > 0.02);
        assert!(rms(115200, 134400) < 0.001);
        let info = crate::media::probe(&path).unwrap();
        assert!((info.duration_ms - 3000).abs() < 80, "{}", info.duration_ms);
        let _ = std::fs::remove_dir_all(d);
    }
}

#[cfg(test)]
mod window_placement_tests {
    use super::*;
    #[test]
    fn exported_window_coordinates_match_canvas_centers() {
        let c = Composition {
            width: 1920,
            height: 1080,
            fps: 30.0,
            duration_ms: 1000,
            background: Default::default(),
            segments: vec![Segment {
                source: PathBuf::from("window.mp4"),
                audio_index: None,
                start_ms: 0,
                end_ms: 1000,
                src_start_ms: 0,
                src_end_ms: 1000,
                speed: 1.0,
                gain: 1.0,
                animation: Default::default(),
                transform: Transform {
                    fit: Default::default(),
                    scale: 0.5,
                    x: 1.1,
                    y: -0.1,
                    opacity: 1.0,
                },
            }],
            texts: Vec::new(),
        };
        let settings = EncodeSettings::default_for_source(1920, 1080);
        let command = c
            .command(std::path::Path::new("out.mp4"), &settings, &[])
            .unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let graph = &args[args.iter().position(|s| s == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("W*1.100000-w/2"));
        assert!(graph.contains("H*-0.100000-h/2"));
        assert!(graph.contains("scale=w=960:h=540"));
    }
}
