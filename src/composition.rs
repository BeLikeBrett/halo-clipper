//! Shared compositor for layered exports and disposable, low-resolution previews.
//! Tracks are stored top first. Each clip has its own input seek, so a late cut
//! never decodes the entire recording and independently moved audio stays aligned.
use crate::{
    encode::EncodeSettings,
    track::{TextOverlay, Transform},
};
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
    pub caption: Option<TextOverlay>,
    pub source_height: u32,
}
#[derive(Clone, Debug)]
pub struct Composition {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: i64,
    /// Video in bottom-to-top paint order, then audio in any order.
    pub segments: Vec<Segment>,
}
impl Composition {
    pub fn command(
        &self,
        destination: &std::path::Path,
        encode: &EncodeSettings,
    ) -> anyhow::Result<Command> {
        anyhow::ensure!(
            self.duration_ms > 0,
            "Nothing to export — the timeline is empty."
        );
        let (w, h) = encode.resolution.unwrap_or((self.width, self.height));
        let (w, h) = ((w / 2 * 2).max(2), (h / 2 * 2).max(2));
        let fps = encode
            .fps_limit
            .map(f64::from)
            .unwrap_or(self.fps)
            .clamp(1.0, 120.0);
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
        let mut graph =
            format!("color=c=black:s={w}x{h}:r={fps:.6}:d={duration:.6},format=yuv420p[base];");
        let mut current = "base".to_string();
        let mut audio = Vec::new();
        for (i, s) in segments.iter().enumerate() {
            let start = s.start_ms as f64 / 1000.0;
            let end = s.end_ms as f64 / 1000.0;
            let speed = s.speed.clamp(0.25, 4.0);
            if let Some(idx) = s.audio_index {
                graph.push_str(&format!("[{i}:a:{idx}]asetpts=PTS-STARTPTS,{},volume={:.6},aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,adelay={}:all=1[a{i}];", super::export::atempo_chain(speed), s.gain.max(0.0), s.start_ms));
                audio.push(format!("[a{i}]"));
            } else {
                let t = s.transform;
                let sw = ((w as f32 * t.scale.clamp(0.1, 2.0)) as u32 / 2 * 2).max(2);
                let sh = ((h as f32 * t.scale.clamp(0.1, 2.0)) as u32 / 2 * 2).max(2);
                graph.push_str(&format!("[{i}:v:0]setpts=(PTS-STARTPTS)/{speed:.6},scale=w={sw}:h={sh}:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1,fps={fps:.6},format=rgba"));
                if let Some(c) = &s.caption {
                    if !c.text.trim().is_empty() {
                        graph.push(',');
                        graph.push_str(&super::export::build_drawtext(
                            c,
                            sh as f64 / s.source_height.max(1) as f64,
                        ));
                    }
                }
                graph.push_str(&format!(",colorchannelmixer=aa={:.6},setpts=PTS+{start:.6}/TB[v{i}];[{current}][v{i}]overlay=x='(W-w)*{:.6}':y='(H-h)*{:.6}':eof_action=pass:repeatlast=0:enable='gte(t,{start:.6})*lt(t,{end:.6})'[mix{i}];", t.opacity.clamp(0.0,1.0), t.x.clamp(0.0,1.0), t.y.clamp(0.0,1.0)));
                current = format!("mix{i}");
            }
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
            caption: None,
            source_height: 64,
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
        let index = (y * 64 + x) * 3;
        out.stdout[index..index + 3].try_into().unwrap()
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
            segments: vec![base, top],
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
            segments: vec![s],
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
