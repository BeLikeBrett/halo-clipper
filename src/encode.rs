//! Video/audio encode settings + per-codec ffmpeg arg generation.
//!
//! Why this lives in its own module: both the export pipeline (export.rs)
//! and the UI / Auto-resolution logic (app.rs) need to know about codecs,
//! resolutions, and bitrates. Keeping the data types here lets us hand a
//! plain `EncodeSettings` value around without entangling Project state.

use std::process::Command;

/// Resolution preset for the Custom export panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionPreset {
    Source,
    P480,
    P720,
    P1080,
    P1440,
    P4K,
}

impl ResolutionPreset {
    pub const ALL: &'static [ResolutionPreset] = &[
        Self::Source,
        Self::P480,
        Self::P720,
        Self::P1080,
        Self::P1440,
        Self::P4K,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::P480 => "480p",
            Self::P720 => "720p",
            Self::P1080 => "1080p",
            Self::P1440 => "1440p",
            Self::P4K => "4K",
        }
    }

    /// Target output dimensions, preserving aspect ratio. `Source`
    /// returns None so the export pipeline knows to skip the scale step.
    pub fn dimensions(self, src_w: u32, src_h: u32) -> Option<(u32, u32)> {
        let target_h = match self {
            Self::Source => return None,
            Self::P480 => 480,
            Self::P720 => 720,
            Self::P1080 => 1080,
            Self::P1440 => 1440,
            Self::P4K => 2160,
        };
        if src_h == 0 {
            return Some((target_h * 16 / 9, target_h));
        }
        // Preserve aspect; ensure even for codecs that need it (yuv420p).
        let w = (target_h as u64 * src_w as u64 / src_h as u64) as u32;
        let w = (w / 2) * 2;
        let h = (target_h / 2) * 2;
        Some((w, h))
    }

    /// Default video bitrate for this preset (kbps). Roughly matches
    /// what YouTube recommends for SDR uploads — comfortable headroom
    /// without bloating files.
    pub fn default_bitrate_kbps(self) -> u32 {
        match self {
            Self::Source | Self::P1080 => 8_000,
            Self::P480 => 2_500,
            Self::P720 => 5_000,
            Self::P1440 => 16_000,
            Self::P4K => 35_000,
        }
    }
}

/// Video codecs we expose in the UI. The `*_Nvenc`, `*_Vaapi`, `*_Qsv`,
/// and `*_Amf` variants are only listed when `detect_hw_encoders` finds
/// them in `ffmpeg -encoders`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    /// libx264 — software, CRF-based. Most compatible default.
    X264,
    /// libx265 — software, CRF-based. Smaller files, slower.
    X265,
    NvencH264,
    NvencHevc,
    VaapiH264,
    VaapiHevc,
    QsvH264,
    QsvHevc,
    AmfH264,
    AmfHevc,
}

impl VideoCodec {
    pub fn display(self) -> &'static str {
        match self {
            Self::X264 => "libx264 (CPU, H.264)",
            Self::X265 => "libx265 (CPU, H.265)",
            Self::NvencH264 => "NVENC H.264 (NVIDIA GPU)",
            Self::NvencHevc => "NVENC H.265 (NVIDIA GPU)",
            Self::VaapiH264 => "VAAPI H.264 (AMD/Intel GPU)",
            Self::VaapiHevc => "VAAPI H.265 (AMD/Intel GPU)",
            Self::QsvH264 => "Intel QSV H.264",
            Self::QsvHevc => "Intel QSV H.265",
            Self::AmfH264 => "AMD AMF H.264",
            Self::AmfHevc => "AMD AMF H.265",
        }
    }

    /// The ffmpeg `-c:v` value.
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
            Self::X265 => "libx265",
            Self::NvencH264 => "h264_nvenc",
            Self::NvencHevc => "hevc_nvenc",
            Self::VaapiH264 => "h264_vaapi",
            Self::VaapiHevc => "hevc_vaapi",
            Self::QsvH264 => "h264_qsv",
            Self::QsvHevc => "hevc_qsv",
            Self::AmfH264 => "h264_amf",
            Self::AmfHevc => "hevc_amf",
        }
    }

    /// Hardware encoders need their frames in a GPU-visible pixel format
    /// before encode. The export pipeline appends `format=nv12,hwupload`
    /// to the final video filter chain when this returns true, and
    /// passes `-vaapi_device /dev/dri/renderD128` for VAAPI.
    pub fn needs_vaapi_upload(self) -> bool {
        matches!(self, Self::VaapiH264 | Self::VaapiHevc)
    }

    /// Human label for this codec's constant-quality knob. Every codec
    /// family calls it something different; the number's meaning (lower
    /// = better) is the same everywhere.
    pub fn quality_name(self) -> &'static str {
        match self {
            Self::X264 | Self::X265 => "CRF",
            Self::NvencH264 | Self::NvencHevc => "CQ",
            Self::QsvH264 | Self::QsvHevc => "ICQ",
            Self::VaapiH264 | Self::VaapiHevc | Self::AmfH264 | Self::AmfHevc => "QP",
        }
    }

    /// Sensible slider range for the constant-quality value. CRF and the
    /// HW quality scales don't line up numerically, so each family gets
    /// its own span.
    pub fn quality_range(self) -> std::ops::RangeInclusive<u32> {
        match self {
            Self::X264 | Self::X265 => 14..=32,
            Self::NvencH264 | Self::NvencHevc => 15..=35,
            Self::QsvH264 | Self::QsvHevc => 15..=35,
            Self::VaapiH264 | Self::VaapiHevc | Self::AmfH264 | Self::AmfHevc => 18..=35,
        }
    }

    /// Default constant-quality value — visually transparent output
    /// without bloating the file.
    pub fn default_quality(self) -> u32 {
        match self {
            Self::X264 | Self::X265 => 18,
            Self::NvencH264 | Self::NvencHevc => 21,
            Self::QsvH264 | Self::QsvHevc => 21,
            Self::VaapiH264 | Self::VaapiHevc | Self::AmfH264 | Self::AmfHevc => 22,
        }
    }

    /// Sensible default preset string for this codec.
    pub fn default_preset(self) -> &'static str {
        match self {
            Self::X264 | Self::X265 => "fast",
            Self::NvencH264 | Self::NvencHevc => "p4",
            Self::VaapiH264 | Self::VaapiHevc => "",
            Self::QsvH264 | Self::QsvHevc => "medium",
            Self::AmfH264 | Self::AmfHevc => "balanced",
        }
    }

    /// Input-side decode strategy that pairs cleanly with this encoder.
    /// VAAPI encoders → VAAPI decode (frames stay on the GPU end-to-end,
    /// saving an SW decode pass and a per-frame CPU→GPU upload). Other
    /// encoders stay on the safe software decode path — NVENC's CUDA
    /// decode hwaccel is a separate setup we don't currently wire up,
    /// and QSV/AMF mixing decode with VAAPI/non-VAAPI gets thorny fast.
    pub fn hw_decode(self) -> HwDecode {
        match self {
            Self::VaapiH264 | Self::VaapiHevc => HwDecode::Vaapi,
            _ => HwDecode::None,
        }
    }
}

/// How the input video is decoded. `None` = software decode + CPU frames
/// (the safe, universal path). `Vaapi` = `-hwaccel vaapi
/// -hwaccel_output_format vaapi` so frames arrive as VAAPI surfaces and
/// stay on the GPU until the encoder pulls them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwDecode {
    None,
    Vaapi,
}

/// Returns true when the named source codec is reliably VAAPI-decodable
/// across the AMD/Intel hardware we're likely to see. Used to gate the
/// HW-decode fast path: if the source isn't in this set we fall back to
/// software decode so an exotic codec (AV1 on an older card, VP8, etc.)
/// can't break the export.
///
/// The names match ffprobe's `codec_name` output, lower-cased. AV1 is
/// excluded for now because VAAPI AV1 decode requires recent silicon
/// (RDNA 3 / Arc) and a recent driver — safer to take the SW path.
pub fn is_vaapi_decodable(codec_name: &str) -> bool {
    matches!(
        codec_name.to_ascii_lowercase().as_str(),
        "h264" | "hevc" | "h265" | "vp9" | "mpeg2video" | "vc1"
    )
}

/// Audio codecs surfaced in the UI. Defaults to AAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCodec {
    Aac,
    Opus,
}

impl AudioCodec {
    pub const ALL: &'static [AudioCodec] = &[Self::Aac, Self::Opus];

    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "libopus",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Self::Aac => "AAC (universal)",
            Self::Opus => "Opus (better at low bitrates)",
        }
    }
}

/// Either constant quality (variable size) or CBR-style bitrate
/// (constant size target, variable quality). Size-targeting modes like
/// Discord need bitrate mode; quality-targeting modes like Auto and the
/// Custom default prefer constant quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeQuality {
    /// Codec-appropriate constant-quality value — CRF for x264/x265,
    /// QP for VAAPI/AMF, CQ for NVENC, ICQ for QSV. Lower = better
    /// quality (and a bigger file) on every scale.
    ConstantQuality(u32),
    /// kbps, with maxrate/bufsize applied for predictable size.
    Bitrate(u32),
}

/// Full encode configuration. Owned + Cloneable so the worker thread
/// can hold a copy independent of any UI state.
#[derive(Clone, Debug)]
pub struct EncodeSettings {
    /// None = keep source resolution (no scale filter).
    pub resolution: Option<(u32, u32)>,
    pub video_codec: VideoCodec,
    pub quality: EncodeQuality,
    pub video_preset: String,
    /// None = keep source frame rate. Some(n) caps the output at n fps
    /// via ffmpeg's output `-r` (works on both CPU and GPU frame paths,
    /// unlike the `fps` filter).
    pub fps_limit: Option<u32>,
    pub audio_codec: AudioCodec,
    pub audio_bitrate_kbps: u32,
}

impl EncodeSettings {
    /// Default "preserve quality" settings used by Auto and as the
    /// starting point for Custom mode.
    pub fn default_for_source(src_w: u32, src_h: u32) -> Self {
        let _ = (src_w, src_h); // for future bitrate-from-source heuristics
        Self {
            resolution: None,
            video_codec: VideoCodec::X264,
            quality: EncodeQuality::ConstantQuality(18),
            video_preset: VideoCodec::X264.default_preset().into(),
            fps_limit: None,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: 192,
        }
    }

    /// Args following `-c:v` that go on the ffmpeg command line.
    pub fn video_args(&self) -> Vec<String> {
        let mut args = Vec::with_capacity(12);
        args.push("-c:v".into());
        args.push(self.video_codec.ffmpeg_name().into());
        if !self.video_preset.is_empty() {
            args.push("-preset".into());
            args.push(self.video_preset.clone());
        }
        match self.quality {
            // Each codec family spells "constant quality" differently.
            // The flag sets below mirror what the encoders document as
            // their CQ/ICQ/CQP modes.
            EncodeQuality::ConstantQuality(q) => match self.video_codec {
                VideoCodec::X264 | VideoCodec::X265 => {
                    args.extend(["-crf".into(), q.to_string()]);
                }
                VideoCodec::NvencH264 | VideoCodec::NvencHevc => {
                    // -b:v 0 releases NVENC's default bitrate clamp so
                    // -cq actually drives the size.
                    args.extend([
                        "-rc".into(), "vbr".into(),
                        "-cq".into(), q.to_string(),
                        "-b:v".into(), "0".into(),
                    ]);
                }
                VideoCodec::VaapiH264 | VideoCodec::VaapiHevc => {
                    args.extend([
                        "-rc_mode".into(), "CQP".into(),
                        "-qp".into(), q.to_string(),
                    ]);
                }
                VideoCodec::QsvH264 | VideoCodec::QsvHevc => {
                    args.extend(["-global_quality".into(), q.to_string()]);
                }
                VideoCodec::AmfH264 | VideoCodec::AmfHevc => {
                    args.extend([
                        "-rc".into(), "cqp".into(),
                        "-qp_i".into(), q.to_string(),
                        "-qp_p".into(), q.to_string(),
                    ]);
                }
            },
            EncodeQuality::Bitrate(kbps) => {
                args.push("-b:v".into());
                args.push(format!("{kbps}k"));
                // Cap peak bitrate at 1.2× target so the file stays close
                // to the expected size, and give the encoder a sane VBV
                // buffer so it can flex within that cap.
                args.push("-maxrate".into());
                args.push(format!("{}k", kbps * 12 / 10));
                args.push("-bufsize".into());
                args.push(format!("{}k", kbps * 2));
            }
        }
        if let Some(fps) = self.fps_limit {
            args.push("-r".into());
            args.push(fps.to_string());
        }
        args
    }

    pub fn audio_args(&self) -> Vec<String> {
        vec![
            "-c:a".into(),
            self.audio_codec.ffmpeg_name().into(),
            "-b:a".into(),
            format!("{}k", self.audio_bitrate_kbps),
        ]
    }
}

/// Discord size-targeting math. Given a duration and a target file-size
/// budget (in MB), computes the bitrate + resolution that fits a clip
/// into that budget while picking the highest resolution the budget can
/// sustain.
///
/// The target is a *parameter*, not a constant — Discord's upload caps
/// differ per subscription tier (see [`DiscordNitro::TIERS`]), and users
/// can also dial the slider down for short clips where they'd rather
/// have a smaller file than max quality.
pub struct DiscordNitro;

impl DiscordNitro {
    /// 500 MB ceiling — Discord's hard upload cap for full Nitro.
    #[allow(dead_code)]
    pub const NITRO_LIMIT_BYTES: i64 = 500 * 1024 * 1024;
    /// Lowest the user can dial via the slider — 10 MB covers Discord's
    /// free-tier upload limit.
    pub const MIN_TARGET_MB: u32 = 10;
    /// Highest the slider goes. Sits 5% under Discord's 500 MB hard
    /// cap so container overhead can't push the final file over.
    pub const MAX_TARGET_MB: u32 = 475;
    /// One-click presets for Discord's subscription tiers. The Nitro
    /// entry is 475 (not 500) for the same container-overhead headroom
    /// reason as MAX_TARGET_MB.
    pub const TIERS: &'static [(&'static str, u32)] = &[
        ("Free · 10 MB", 10),
        ("Nitro Basic · 50 MB", 50),
        ("Nitro · 500 MB", Self::MAX_TARGET_MB),
    ];
    pub const AUDIO_KBPS: u32 = 128;

    /// Hard ceiling we cap the computed bitrate at, regardless of how
    /// much budget the duration would technically allow. 35 Mbps sits
    /// comfortably inside AMD VCN / NVENC sweet spots, lets short clips
    /// actually consume more of the user's target budget, and stops
    /// well short of the range where HW encoders get sluggish or where
    /// extra bitrate produces no perceptible quality gain at 1080p.
    pub const MAX_BITRATE_KBPS: u32 = 35_000;

    /// Convert MB → bytes for the target slider value.
    pub fn target_bytes_from_mb(mb: u32) -> i64 {
        (mb as i64) * 1024 * 1024
    }

    /// Total available kbps (video + audio) for a clip of the given
    /// duration at the given target size. Floor of 200 kbps so tiny-
    /// duration clips don't end up with absurd numbers; ceiling at
    /// MAX_BITRATE_KBPS so HW encoders don't sit at firmware-init
    /// forever just to chase pixels nobody can see.
    pub fn total_kbps_for(duration_ms: i64, target_bytes: i64) -> u32 {
        if duration_ms <= 0 {
            return Self::MAX_BITRATE_KBPS;
        }
        let duration_secs = (duration_ms as f64) / 1000.0;
        let kbps = (target_bytes as f64) * 8.0 / 1000.0 / duration_secs;
        kbps.clamp(200.0, Self::MAX_BITRATE_KBPS as f64) as u32
    }

    /// Audio bitrate for the given budget. 128 kbps normally, stepping
    /// down when the total budget is tight (small targets like the
    /// free-tier 10 MB on long clips) so audio doesn't eat the video's
    /// entire allowance.
    pub fn audio_kbps_for(duration_ms: i64, target_bytes: i64) -> u32 {
        let total = Self::total_kbps_for(duration_ms, target_bytes);
        if total >= 1_200 {
            Self::AUDIO_KBPS
        } else if total >= 500 {
            96
        } else {
            64
        }
    }

    pub fn video_kbps_for(duration_ms: i64, target_bytes: i64) -> u32 {
        let total = Self::total_kbps_for(duration_ms, target_bytes);
        total
            .saturating_sub(Self::audio_kbps_for(duration_ms, target_bytes))
            .max(150)
    }

    /// Resolution preset that the given video bitrate can support
    /// comfortably. Numbers are based on rule-of-thumb encoding guides
    /// (YouTube SDR upload recommendations + H.264 efficiency curves).
    pub fn resolution_for_bitrate(video_kbps: u32) -> ResolutionPreset {
        match video_kbps {
            0..=1_500 => ResolutionPreset::P480,
            1_501..=3_500 => ResolutionPreset::P720,
            3_501..=8_000 => ResolutionPreset::P1080,
            8_001..=18_000 => ResolutionPreset::P1440,
            _ => ResolutionPreset::Source,
        }
    }

    /// Build the EncodeSettings to ship to ffmpeg.
    pub fn settings_for(
        duration_ms: i64,
        target_bytes: i64,
        src_w: u32,
        src_h: u32,
        codec: VideoCodec,
    ) -> EncodeSettings {
        let video_kbps = Self::video_kbps_for(duration_ms, target_bytes);
        let resolution = Self::resolution_for_bitrate(video_kbps).dimensions(src_w, src_h);
        EncodeSettings {
            resolution,
            video_codec: codec,
            // CBR for predictable size, regardless of what the codec
            // would normally default to.
            quality: EncodeQuality::Bitrate(video_kbps),
            video_preset: codec.default_preset().into(),
            fps_limit: None,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: Self::audio_kbps_for(duration_ms, target_bytes),
        }
    }

    /// Estimated output file size in bytes for the chosen bitrate +
    /// duration. Real CBR encodes (especially HW) tend to land 5–10%
    /// under target because rate-control leaves headroom for peaks, so
    /// we report 92% of the theoretical max + 3% for muxer overhead.
    pub fn estimated_size_bytes(duration_ms: i64, target_bytes: i64) -> i64 {
        let video_kbps = Self::video_kbps_for(duration_ms, target_bytes) as i64;
        let total_kbps = video_kbps + Self::audio_kbps_for(duration_ms, target_bytes) as i64;
        let theoretical_bits = total_kbps * 1000 * duration_ms / 1000;
        // 92% rate-control realism × 103% muxer overhead ≈ 0.948×.
        theoretical_bits / 8 * 948 / 1000
    }
}

/// Identifies the dominant GPU vendor so we can recommend the right
/// hardware encoder family. Only the four cases the encoder list cares
/// about — everything else reads as "no preference, fall back to CPU".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl GpuVendor {
    pub fn display(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA",
            Self::Amd => "AMD",
            Self::Intel => "Intel",
            Self::Other => "Other",
        }
    }
}

/// Probe `/sys/class/drm` for the system's GPU vendor. Linux-specific
/// today; on other platforms (or when the path isn't readable) we just
/// return None and the recommendation falls back to libx264.
///
/// Strategy: walk `card0`..`card7`, read each one's `device/vendor`,
/// and prefer NVIDIA → AMD → Intel in that order (i.e. a dGPU shadowing
/// an iGPU still surfaces). The PCI vendor IDs are stable IEEE values
/// so the match is safe across distros.
pub fn detect_gpu() -> Option<GpuVendor> {
    let mut best: Option<GpuVendor> = None;
    for i in 0..8 {
        let path = format!("/sys/class/drm/card{i}/device/vendor");
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let v = match content.trim() {
            "0x10de" => GpuVendor::Nvidia,
            "0x1002" => GpuVendor::Amd,
            "0x8086" => GpuVendor::Intel,
            _ => GpuVendor::Other,
        };
        // Priority: NVIDIA > AMD > Intel > Other. A discrete card almost
        // always sits at a lower index than the iGPU, so this prefers
        // dGPU encode when both are present.
        best = Some(match (best, v) {
            (Some(GpuVendor::Nvidia), _) => GpuVendor::Nvidia,
            (_, GpuVendor::Nvidia) => GpuVendor::Nvidia,
            (Some(GpuVendor::Amd), _) => GpuVendor::Amd,
            (_, GpuVendor::Amd) => GpuVendor::Amd,
            (Some(GpuVendor::Intel), _) => GpuVendor::Intel,
            (_, GpuVendor::Intel) => GpuVendor::Intel,
            _ => v,
        });
    }
    best
}

/// Pick the codec we'd recommend given a detected GPU and the encoders
/// ffmpeg actually has. NVIDIA → NVENC, AMD → VAAPI, Intel → QSV; in
/// every case requires the matching HW encoder to be in `available`.
/// Otherwise falls through to libx264 (always shipped).
pub fn recommend_codec(gpu: Option<GpuVendor>, available: &[VideoCodec]) -> VideoCodec {
    let preferred = match gpu {
        Some(GpuVendor::Nvidia) => Some(VideoCodec::NvencH264),
        Some(GpuVendor::Amd) => Some(VideoCodec::VaapiH264),
        Some(GpuVendor::Intel) => Some(VideoCodec::QsvH264),
        _ => None,
    };
    if let Some(p) = preferred {
        if available.contains(&p) {
            return p;
        }
    }
    VideoCodec::X264
}

/// Probe `ffmpeg -encoders` to find which hardware encoders the local
/// ffmpeg build actually supports. Synchronous — the command is fast
/// (≲ 100ms) and only runs once at startup.
pub fn detect_hw_encoders() -> Vec<VideoCodec> {
    let Ok(out) = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
    else {
        log::warn!("ffmpeg -encoders probe failed; no HW codecs offered");
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let candidates = [
        ("h264_nvenc", VideoCodec::NvencH264),
        ("hevc_nvenc", VideoCodec::NvencHevc),
        ("h264_vaapi", VideoCodec::VaapiH264),
        ("hevc_vaapi", VideoCodec::VaapiHevc),
        ("h264_qsv", VideoCodec::QsvH264),
        ("hevc_qsv", VideoCodec::QsvHevc),
        ("h264_amf", VideoCodec::AmfH264),
        ("hevc_amf", VideoCodec::AmfHevc),
    ];
    candidates
        .iter()
        .filter(|(name, _)| text.contains(name))
        .map(|&(_, codec)| codec)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ResolutionPreset ----------------------------------------------

    #[test]
    fn source_preset_returns_no_dimensions() {
        assert_eq!(ResolutionPreset::Source.dimensions(1920, 1080), None);
    }

    #[test]
    fn presets_preserve_aspect_ratio() {
        let (w, h) = ResolutionPreset::P720
            .dimensions(1920, 1080)
            .expect("720p should produce dims");
        assert_eq!(h, 720);
        assert_eq!(w, 1280); // 1280:720 = 16:9

        let (w, h) = ResolutionPreset::P720
            .dimensions(4096, 2304)
            .expect("720p should produce dims");
        assert_eq!(h, 720);
        // 4096:2304 = 16:9, so width should be ~1280.
        assert!((w as i32 - 1280).abs() <= 2);
    }

    #[test]
    fn presets_round_to_even_widths() {
        // Pathological aspect that produces an odd raw width; we should
        // still snap to even so yuv420p encoders accept it.
        let (w, h) = ResolutionPreset::P480.dimensions(853, 480).unwrap();
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
    }

    #[test]
    fn default_bitrate_scales_with_resolution() {
        assert!(
            ResolutionPreset::P480.default_bitrate_kbps()
                < ResolutionPreset::P720.default_bitrate_kbps()
        );
        assert!(
            ResolutionPreset::P1080.default_bitrate_kbps()
                < ResolutionPreset::P1440.default_bitrate_kbps()
        );
        assert!(
            ResolutionPreset::P1440.default_bitrate_kbps()
                < ResolutionPreset::P4K.default_bitrate_kbps()
        );
    }

    // ---- EncodeSettings::video_args ------------------------------------

    fn settings_for(codec: VideoCodec) -> EncodeSettings {
        let mut s = EncodeSettings::default_for_source(1920, 1080);
        s.video_codec = codec;
        s.video_preset = codec.default_preset().into();
        s
    }

    #[test]
    fn x264_uses_crf_not_bitrate() {
        let args = settings_for(VideoCodec::X264).video_args();
        assert_eq!(args[0], "-c:v");
        assert_eq!(args[1], "libx264");
        assert!(args.iter().any(|a| a == "-crf"));
        assert!(!args.iter().any(|a| a == "-b:v"));
    }

    #[test]
    fn x265_uses_crf() {
        let args = settings_for(VideoCodec::X265).video_args();
        assert_eq!(args[1], "libx265");
        assert!(args.iter().any(|a| a == "-crf"));
    }

    #[test]
    fn nvenc_uses_bitrate_with_maxrate_cap() {
        let mut s = settings_for(VideoCodec::NvencH264);
        s.quality = EncodeQuality::Bitrate(5_000);
        let args = s.video_args();
        assert_eq!(args[1], "h264_nvenc");
        let bv_idx = args.iter().position(|a| a == "-b:v").unwrap();
        assert_eq!(args[bv_idx + 1], "5000k");
        let max_idx = args.iter().position(|a| a == "-maxrate").unwrap();
        assert_eq!(args[max_idx + 1], "6000k"); // 5000 * 1.2
        let buf_idx = args.iter().position(|a| a == "-bufsize").unwrap();
        assert_eq!(args[buf_idx + 1], "10000k"); // 5000 * 2
        assert!(!args.iter().any(|a| a == "-crf"));
    }

    #[test]
    fn vaapi_bitrate_mode_and_signals_hwupload() {
        let mut s = settings_for(VideoCodec::VaapiH264);
        s.quality = EncodeQuality::Bitrate(8_000);
        assert!(s.video_codec.needs_vaapi_upload());
        let args = s.video_args();
        assert_eq!(args[1], "h264_vaapi");
        assert!(args.iter().any(|a| a == "-b:v"));
    }

    #[test]
    fn forcing_bitrate_on_x264_skips_crf() {
        // Discord mode forces CBR on libx264 — make sure the
        // EncodeQuality::Bitrate path actually flips the flags.
        let mut s = settings_for(VideoCodec::X264);
        s.quality = EncodeQuality::Bitrate(3_000);
        let args = s.video_args();
        assert!(!args.iter().any(|a| a == "-crf"));
        assert!(args.iter().any(|a| a == "-b:v"));
        assert!(args.iter().any(|a| a == "-maxrate"));
        assert!(args.iter().any(|a| a == "-bufsize"));
    }

    // ---- Constant-quality flags per HW codec family --------------------
    //
    // Each family exposes constant quality behind different flags. These
    // pin the exact args so a refactor can't silently ship an encoder an
    // option it doesn't understand.

    fn cq_settings(codec: VideoCodec, q: u32) -> Vec<String> {
        let mut s = settings_for(codec);
        s.quality = EncodeQuality::ConstantQuality(q);
        s.video_args()
    }

    #[test]
    fn vaapi_constant_quality_uses_cqp() {
        let args = cq_settings(VideoCodec::VaapiH264, 22);
        let rc = args.iter().position(|a| a == "-rc_mode").unwrap();
        assert_eq!(args[rc + 1], "CQP");
        let qp = args.iter().position(|a| a == "-qp").unwrap();
        assert_eq!(args[qp + 1], "22");
        assert!(!args.iter().any(|a| a == "-b:v"));
    }

    #[test]
    fn nvenc_constant_quality_uses_cq_with_zero_bitrate() {
        let args = cq_settings(VideoCodec::NvencHevc, 21);
        let cq = args.iter().position(|a| a == "-cq").unwrap();
        assert_eq!(args[cq + 1], "21");
        // -b:v 0 releases NVENC's default clamp.
        let bv = args.iter().position(|a| a == "-b:v").unwrap();
        assert_eq!(args[bv + 1], "0");
    }

    #[test]
    fn qsv_constant_quality_uses_global_quality() {
        let args = cq_settings(VideoCodec::QsvH264, 23);
        let gq = args.iter().position(|a| a == "-global_quality").unwrap();
        assert_eq!(args[gq + 1], "23");
    }

    #[test]
    fn amf_constant_quality_uses_cqp_pair() {
        let args = cq_settings(VideoCodec::AmfH264, 24);
        assert!(args.iter().any(|a| a == "-qp_i"));
        assert!(args.iter().any(|a| a == "-qp_p"));
    }

    #[test]
    fn fps_limit_appends_output_rate() {
        let mut s = settings_for(VideoCodec::X264);
        s.fps_limit = Some(30);
        let args = s.video_args();
        let r = args.iter().position(|a| a == "-r").unwrap();
        assert_eq!(args[r + 1], "30");
        // No fps limit → no -r flag.
        s.fps_limit = None;
        assert!(!s.video_args().iter().any(|a| a == "-r"));
    }

    // ---- Discord Nitro -----------------------------------------------

    #[test]
    fn nitro_2min_clip_keeps_high_resolution() {
        let dur = 2 * 60 * 1000; // 2 minutes
        let target = DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB);
        let kbps = DiscordNitro::video_kbps_for(dur, target);
        // 475 MB across 120s ≈ 33 Mbps total, plenty of headroom.
        assert!(kbps > 18_000, "expected high bitrate, got {kbps}");
        assert_eq!(
            DiscordNitro::resolution_for_bitrate(kbps),
            ResolutionPreset::Source
        );
    }

    #[test]
    fn nitro_30min_clip_drops_to_720p_or_480p() {
        let dur = 30 * 60 * 1000;
        let target = DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB);
        let kbps = DiscordNitro::video_kbps_for(dur, target);
        let res = DiscordNitro::resolution_for_bitrate(kbps);
        // At 30min/475MB you've got ~2.1 Mbps total → 480p territory.
        assert!(matches!(
            res,
            ResolutionPreset::P480 | ResolutionPreset::P720
        ));
    }

    #[test]
    fn nitro_estimated_size_stays_under_500mb() {
        // Spot-check a range of durations; estimate should never exceed
        // the Nitro hard limit (500 MB).
        let target = DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB);
        for &dur_min in &[1, 5, 15, 30, 60, 120] {
            let dur_ms = dur_min * 60 * 1000;
            let est = DiscordNitro::estimated_size_bytes(dur_ms, target);
            assert!(
                est < DiscordNitro::NITRO_LIMIT_BYTES,
                "{}-min clip estimate {} > 500 MB limit",
                dur_min,
                est
            );
        }
    }

    #[test]
    fn nitro_settings_use_cbr_regardless_of_codec() {
        // Even libx264 (which normally uses CRF) should produce
        // -b:v / -maxrate args in Nitro mode.
        let s = DiscordNitro::settings_for(
            5 * 60 * 1000,
            DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB),
            1920,
            1080,
            VideoCodec::X264,
        );
        assert!(matches!(s.quality, EncodeQuality::Bitrate(_)));
        let args = s.video_args();
        assert!(args.iter().any(|a| a == "-b:v"));
        assert!(!args.iter().any(|a| a == "-crf"));
    }

    #[test]
    fn nitro_audio_uses_128k_aac() {
        let s = DiscordNitro::settings_for(
            60_000,
            DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB),
            1920,
            1080,
            VideoCodec::X264,
        );
        assert_eq!(s.audio_bitrate_kbps, 128);
        assert_eq!(s.audio_codec, AudioCodec::Aac);
    }

    #[test]
    fn small_target_steps_audio_down_and_stays_in_budget() {
        // 5-minute clip into the free tier's 10 MB. Audio must drop below
        // 128 kbps or it would eat most of the ~273 kbps total budget.
        let dur = 5 * 60 * 1000;
        let target = DiscordNitro::target_bytes_from_mb(10);
        let audio = DiscordNitro::audio_kbps_for(dur, target);
        assert!(audio < 128, "expected reduced audio bitrate, got {audio}");
        let est = DiscordNitro::estimated_size_bytes(dur, target);
        let est_mb = est as f64 / (1024.0 * 1024.0);
        assert!(
            est_mb <= 10.5,
            "10 MB target estimated at {est_mb:.1} MB"
        );
    }

    #[test]
    fn tier_presets_stay_within_slider_range() {
        for (_, mb) in DiscordNitro::TIERS {
            assert!(*mb >= DiscordNitro::MIN_TARGET_MB);
            assert!(*mb <= DiscordNitro::MAX_TARGET_MB);
        }
    }

    #[test]
    fn nitro_user_target_scales_bitrate_proportionally() {
        // 5-minute clip at 100 MB vs 475 MB target. Long enough that
        // the MAX_BITRATE_KBPS cap doesn't dominate, so the slider
        // value actually translates into different bitrates.
        let dur = 5 * 60 * 1000; // 5 minutes
        let kbps_small =
            DiscordNitro::video_kbps_for(dur, DiscordNitro::target_bytes_from_mb(100));
        let kbps_full = DiscordNitro::video_kbps_for(
            dur,
            DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB),
        );
        assert!(
            kbps_small < kbps_full,
            "smaller target should use less bitrate (small={kbps_small}, full={kbps_full})"
        );

        let est_100 = DiscordNitro::estimated_size_bytes(
            dur,
            DiscordNitro::target_bytes_from_mb(100),
        );
        let mb_100 = est_100 as f64 / (1024.0 * 1024.0);
        // Should be near (but not above) 100 MB.
        assert!(mb_100 < 100.0, "100 MB target produced {mb_100:.1} MB estimate");
        assert!(mb_100 > 70.0, "100 MB target should land near 100 MB ({mb_100:.1})");
    }

    #[test]
    fn audio_args_use_chosen_codec_and_bitrate() {
        let mut s = EncodeSettings::default_for_source(1920, 1080);
        assert_eq!(s.audio_args(), ["-c:a", "aac", "-b:a", "192k"]);
        s.audio_codec = AudioCodec::Opus;
        assert_eq!(s.audio_args(), ["-c:a", "libopus", "-b:a", "192k"]);
    }

    #[test]
    fn quality_defaults_sit_inside_ranges() {
        for c in [
            VideoCodec::X264,
            VideoCodec::X265,
            VideoCodec::NvencH264,
            VideoCodec::VaapiH264,
            VideoCodec::QsvH264,
            VideoCodec::AmfH264,
        ] {
            assert!(
                c.quality_range().contains(&c.default_quality()),
                "{c:?} default quality outside its slider range"
            );
        }
    }

    // ---- Recommendation ------------------------------------------------

    #[test]
    fn recommend_falls_back_to_x264_when_no_hw_available() {
        assert_eq!(recommend_codec(None, &[]), VideoCodec::X264);
        assert_eq!(recommend_codec(Some(GpuVendor::Nvidia), &[]), VideoCodec::X264);
    }

    #[test]
    fn recommend_picks_vendor_specific_when_present() {
        assert_eq!(
            recommend_codec(Some(GpuVendor::Nvidia), &[VideoCodec::NvencH264]),
            VideoCodec::NvencH264
        );
        assert_eq!(
            recommend_codec(Some(GpuVendor::Amd), &[VideoCodec::VaapiH264]),
            VideoCodec::VaapiH264
        );
        assert_eq!(
            recommend_codec(Some(GpuVendor::Intel), &[VideoCodec::QsvH264]),
            VideoCodec::QsvH264
        );
    }

    #[test]
    fn recommend_ignores_mismatched_vendor_encoder() {
        // An AMD system shouldn't get NVENC even if it happens to be
        // listed (e.g., proprietary driver+library installed but no GPU).
        assert_eq!(
            recommend_codec(Some(GpuVendor::Amd), &[VideoCodec::NvencH264]),
            VideoCodec::X264
        );
    }
}
