//! Video/audio encode settings + per-codec ffmpeg arg generation.
//!
//! Why this lives in its own module: both the export pipeline (export.rs)
//! and the UI / Auto-resolution logic (app.rs) need to know about codecs,
//! resolutions, and bitrates. Keeping the data types here lets us hand a
//! plain `EncodeSettings` value around without entangling Project state.

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

/// Video codecs we expose in the UI. Everything except the two software
/// H.26x encoders is only listed when `detect_available_codecs` finds it
/// in `ffmpeg -encoders`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    /// libx264 — software, CRF-based. Most compatible default.
    X264,
    /// libx265 — software, CRF-based. Smaller files, slower.
    X265,
    /// SVT-AV1 — software AV1. Needs roughly 40% less bitrate than
    /// H.264 for the same picture, which is what lets a tight size
    /// budget hold a real resolution. Slower than x264 at low presets;
    /// `svt_preset_for` scales the preset by clip length.
    SvtAv1,
    NvencH264,
    NvencHevc,
    NvencAv1,
    VaapiH264,
    VaapiHevc,
    VaapiAv1,
    QsvH264,
    QsvHevc,
    QsvAv1,
    AmfH264,
    AmfHevc,
    AmfAv1,
}

impl VideoCodec {
    pub fn display(self) -> &'static str {
        match self {
            Self::X264 => "libx264 (CPU, H.264)",
            Self::X265 => "libx265 (CPU, H.265)",
            Self::SvtAv1 => "SVT-AV1 (CPU, AV1)",
            Self::NvencH264 => "NVENC H.264 (NVIDIA GPU)",
            Self::NvencHevc => "NVENC H.265 (NVIDIA GPU)",
            Self::NvencAv1 => "NVENC AV1 (NVIDIA GPU)",
            Self::VaapiH264 => "VAAPI H.264 (AMD/Intel GPU)",
            Self::VaapiHevc => "VAAPI H.265 (AMD/Intel GPU)",
            Self::VaapiAv1 => "VAAPI AV1 (AMD/Intel GPU)",
            Self::QsvH264 => "Intel QSV H.264",
            Self::QsvHevc => "Intel QSV H.265",
            Self::QsvAv1 => "Intel QSV AV1",
            Self::AmfH264 => "AMD AMF H.264",
            Self::AmfHevc => "AMD AMF H.265",
            Self::AmfAv1 => "AMD AMF AV1",
        }
    }

    /// The ffmpeg `-c:v` value.
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::X264 => "libx264",
            Self::X265 => "libx265",
            Self::SvtAv1 => "libsvtav1",
            Self::NvencH264 => "h264_nvenc",
            Self::NvencHevc => "hevc_nvenc",
            Self::NvencAv1 => "av1_nvenc",
            Self::VaapiH264 => "h264_vaapi",
            Self::VaapiHevc => "hevc_vaapi",
            Self::VaapiAv1 => "av1_vaapi",
            Self::QsvH264 => "h264_qsv",
            Self::QsvHevc => "hevc_qsv",
            Self::QsvAv1 => "av1_qsv",
            Self::AmfH264 => "h264_amf",
            Self::AmfHevc => "hevc_amf",
            Self::AmfAv1 => "av1_amf",
        }
    }

    /// Hardware encoders need their frames in a GPU-visible pixel format
    /// before encode. The export pipeline appends `format=nv12,hwupload`
    /// to the final video filter chain when this returns true, and
    /// passes `-vaapi_device /dev/dri/renderD128` for VAAPI.
    pub fn needs_vaapi_upload(self) -> bool {
        matches!(self, Self::VaapiH264 | Self::VaapiHevc | Self::VaapiAv1)
    }

    /// True for every AV1 encoder, hardware or software.
    /// Software (CPU) encoders — always runnable when listed, and the
    /// fallback when a hardware encode fails.
    pub fn is_software(self) -> bool {
        matches!(self, Self::X264 | Self::X265 | Self::SvtAv1)
    }

    /// The CPU encoder that makes the same format, for when this
    /// hardware encoder fails mid-export. `None` for software encoders
    /// (nothing slower to fall back to). AV1 falls back to SVT-AV1 when
    /// this ffmpeg has it, otherwise to x264 — a working H.264 file
    /// beats a failed AV1 one.
    pub fn cpu_fallback(self, available: &[VideoCodec]) -> Option<VideoCodec> {
        if self.is_software() {
            return None;
        }
        Some(if self.is_av1() {
            if available.contains(&Self::SvtAv1) {
                Self::SvtAv1
            } else {
                Self::X264
            }
        } else if matches!(
            self,
            Self::NvencHevc | Self::VaapiHevc | Self::QsvHevc | Self::AmfHevc
        ) {
            Self::X265
        } else {
            Self::X264
        })
    }

    pub fn is_av1(self) -> bool {
        matches!(
            self,
            Self::SvtAv1 | Self::NvencAv1 | Self::VaapiAv1 | Self::QsvAv1 | Self::AmfAv1
        )
    }

    /// Bitrate this codec needs, as a fraction of what H.264 needs for
    /// the same perceived quality. Lower = more efficient.
    ///
    /// The numbers are the conservative end of the published codec
    /// comparisons (SVT-AV1 vs x264 BD-rate lands around −45%; fixed-
    /// function encoders give up a chunk of that to hit real-time), and
    /// every H.264 variant is pinned at 1.0 so the H.264 size math this
    /// tool has always used doesn't shift underneath the user.
    pub fn bitrate_efficiency(self) -> f32 {
        match self {
            Self::X264 | Self::NvencH264 | Self::VaapiH264 | Self::QsvH264 | Self::AmfH264 => 1.0,
            Self::X265 => 0.72,
            Self::NvencHevc | Self::VaapiHevc | Self::QsvHevc | Self::AmfHevc => 0.80,
            Self::SvtAv1 => 0.58,
            Self::NvencAv1 | Self::VaapiAv1 | Self::QsvAv1 | Self::AmfAv1 => 0.65,
        }
    }

    /// Human label for this codec's constant-quality knob. Every codec
    /// family calls it something different; the number's meaning (lower
    /// = better) is the same everywhere.
    pub fn quality_name(self) -> &'static str {
        match self {
            Self::X264 | Self::X265 | Self::SvtAv1 => "CRF",
            Self::NvencH264 | Self::NvencHevc | Self::NvencAv1 => "CQ",
            Self::QsvH264 | Self::QsvHevc | Self::QsvAv1 => "ICQ",
            Self::VaapiH264 | Self::VaapiHevc | Self::AmfH264 | Self::AmfHevc => "QP",
            // AV1's quantizer index is a 0..255 scale, not H.26x's 0..51.
            Self::VaapiAv1 | Self::AmfAv1 => "QIndex",
        }
    }

    /// Sensible slider range for the constant-quality value. CRF and the
    /// HW quality scales don't line up numerically, so each family gets
    /// its own span.
    pub fn quality_range(self) -> std::ops::RangeInclusive<u32> {
        match self {
            Self::X264 | Self::X265 => 14..=32,
            // SVT-AV1's CRF scale runs 0..63 and sits numerically higher
            // than x264's for the same look — CRF 30 ≈ x264 CRF 21.
            Self::SvtAv1 => 18..=45,
            Self::NvencH264 | Self::NvencHevc => 15..=35,
            Self::NvencAv1 => 15..=45,
            Self::QsvH264 | Self::QsvHevc => 15..=35,
            Self::QsvAv1 => 15..=45,
            Self::VaapiH264 | Self::VaapiHevc | Self::AmfH264 | Self::AmfHevc => 18..=35,
            // 0..255 quantizer index — 60 is near-transparent, 180 rough.
            Self::VaapiAv1 | Self::AmfAv1 => 60..=180,
        }
    }

    /// Default constant-quality value — visually transparent output
    /// without bloating the file.
    pub fn default_quality(self) -> u32 {
        match self {
            Self::X264 | Self::X265 => 18,
            Self::SvtAv1 => 26,
            Self::NvencH264 | Self::NvencHevc => 21,
            Self::NvencAv1 => 26,
            Self::QsvH264 | Self::QsvHevc => 21,
            Self::QsvAv1 => 26,
            Self::VaapiH264 | Self::VaapiHevc | Self::AmfH264 | Self::AmfHevc => 22,
            Self::VaapiAv1 | Self::AmfAv1 => 110,
        }
    }

    /// Sensible default preset string for this codec.
    pub fn default_preset(self) -> &'static str {
        match self {
            Self::X264 | Self::X265 => "fast",
            // SVT-AV1's preset is a number, 0 (slowest) .. 13 (fastest).
            Self::SvtAv1 => "8",
            Self::NvencH264 | Self::NvencHevc | Self::NvencAv1 => "p4",
            Self::VaapiH264 | Self::VaapiHevc | Self::VaapiAv1 => "",
            Self::QsvH264 | Self::QsvHevc => "medium",
            Self::AmfH264 | Self::AmfHevc => "balanced",
            // av1_qsv / av1_amf take a numeric preset with no named
            // aliases, so leave it at the encoder's own default.
            Self::QsvAv1 | Self::AmfAv1 => "",
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
            Self::VaapiH264 | Self::VaapiHevc | Self::VaapiAv1 => HwDecode::Vaapi,
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
                VideoCodec::X264 | VideoCodec::X265 | VideoCodec::SvtAv1 => {
                    args.extend(["-crf".into(), q.to_string()]);
                }
                VideoCodec::NvencH264 | VideoCodec::NvencHevc | VideoCodec::NvencAv1 => {
                    // -b:v 0 releases NVENC's default bitrate clamp so
                    // -cq actually drives the size.
                    args.extend([
                        "-rc".into(),
                        "vbr".into(),
                        "-cq".into(),
                        q.to_string(),
                        "-b:v".into(),
                        "0".into(),
                    ]);
                }
                VideoCodec::VaapiH264 | VideoCodec::VaapiHevc => {
                    args.extend(["-rc_mode".into(), "CQP".into(), "-qp".into(), q.to_string()]);
                }
                VideoCodec::VaapiAv1 => {
                    // av1_vaapi has no -qp of its own (unlike the H.26x
                    // VAAPI encoders); it reads the generic global
                    // quality field, which -q:v sets.
                    args.extend([
                        "-rc_mode".into(),
                        "CQP".into(),
                        "-q:v".into(),
                        q.to_string(),
                    ]);
                }
                VideoCodec::QsvH264 | VideoCodec::QsvHevc | VideoCodec::QsvAv1 => {
                    args.extend(["-global_quality".into(), q.to_string()]);
                }
                VideoCodec::AmfH264 | VideoCodec::AmfHevc | VideoCodec::AmfAv1 => {
                    args.extend([
                        "-rc".into(),
                        "cqp".into(),
                        "-qp_i".into(),
                        q.to_string(),
                        "-qp_p".into(),
                        q.to_string(),
                    ]);
                }
            },
            EncodeQuality::Bitrate(kbps) => {
                args.push("-b:v".into());
                args.push(format!("{kbps}k"));
                // Cap peak bitrate at 1.2× target so the file stays close
                // to the expected size, and give the encoder a sane VBV
                // buffer so it can flex within that cap.
                //
                // libsvtav1 is the exception: it hard-errors with "Max
                // Bitrate only supported with CRF mode" if -maxrate is
                // present in a bitrate-targeting encode, so it gets the
                // bitrate + buffer only and rate-controls to the target
                // on its own.
                if !matches!(self.video_codec, VideoCodec::SvtAv1) {
                    args.push("-maxrate".into());
                    args.push(format!("{}k", kbps * 12 / 10));
                }
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
    pub const NITRO_LIMIT_BYTES: i64 = 500 * 1_000_000;
    /// Lowest the user can dial via the slider. Discord's free tier is
    /// 20 MB (raised from 10 in 2026), but the slider goes below it so
    /// short clips can be made deliberately small.
    pub const MIN_TARGET_MB: u32 = 10;
    /// Highest the slider goes: Discord's 500 MB Nitro cap. The slider
    /// value is always a hard *cap*; the headroom for rate-control
    /// overshoot and container overhead lives in
    /// [`budget_bytes`](Self::budget_bytes), not in these numbers.
    pub const MAX_TARGET_MB: u32 = 500;
    /// One-click presets for Discord's subscription tiers (their caps).
    pub const TIERS: &'static [(&'static str, u32)] = &[
        ("Free · 20 MB", 20),
        ("Nitro Basic · 50 MB", 50),
        ("Nitro · 500 MB", Self::MAX_TARGET_MB),
    ];
    /// Share of the cap the encoder is asked to fill. Encoders don't hit
    /// a bitrate target exactly — measured on a high-motion 1440p60
    /// game clip at a 0% margin, h264_vaapi/av1_vaapi on VCN 4 landed
    /// 98–100% of the cap and libx264 landed 100.7% (over). Other GPU
    /// generations and drivers drift further, so aim 5% low; anything
    /// that still overshoots is caught by the post-encode size check
    /// in `export` and re-encoded smaller.
    pub const BUDGET_PERCENT: i64 = 95;
    pub const AUDIO_KBPS: u32 = 128;

    /// Hard ceiling we cap the computed bitrate at, regardless of how
    /// much budget the duration would technically allow. 35 Mbps sits
    /// comfortably inside AMD VCN / NVENC sweet spots, lets short clips
    /// actually consume more of the user's target budget, and stops
    /// well short of the range where HW encoders get sluggish or where
    /// extra bitrate produces no perceptible quality gain at 1080p.
    pub const MAX_BITRATE_KBPS: u32 = 35_000;

    /// Convert the slider's MB value → the hard cap in bytes. Decimal
    /// megabytes on purpose: it is the smaller reading of "20 MB", so the
    /// file fits whether the uploader counts in MB or MiB.
    pub fn target_bytes_from_mb(mb: u32) -> i64 {
        (mb as i64) * 1_000_000
    }

    /// Bytes the encode actually aims for under a hard `cap_bytes`.
    pub fn budget_bytes(cap_bytes: i64) -> i64 {
        cap_bytes * Self::BUDGET_PERCENT / 100
    }

    /// Video bitrate for a re-encode after an export came out at
    /// `actual_bytes`, over `cap_bytes`. Scales the video share by how
    /// far the encoder overshot, aiming at the same budget as the first
    /// pass — so an encoder that runs 8% hot gets asked for 8% less.
    /// Audio is left alone (AAC/Opus at a fixed rate land on target).
    /// `None` when there's no room left to shrink into.
    pub fn refit_video_kbps(
        video_kbps: u32,
        audio_kbps: u32,
        duration_ms: i64,
        actual_bytes: i64,
        cap_bytes: i64,
    ) -> Option<u32> {
        let audio_bytes = audio_kbps as i64 * duration_ms / 8;
        let actual_video = actual_bytes - audio_bytes;
        let goal_video = Self::budget_bytes(cap_bytes) - audio_bytes;
        if actual_video <= 0 || goal_video <= 0 {
            return None;
        }
        let kbps = (video_kbps as f64 * goal_video as f64 / actual_video as f64) as u32;
        // Never ask for *more* than last time, and stop at a rate no
        // encoder can make a picture from.
        let kbps = kbps.min(video_kbps.saturating_sub(1));
        (kbps >= 50).then_some(kbps)
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
        let kbps = (Self::budget_bytes(target_bytes) as f64) * 8.0 / 1000.0 / duration_secs;
        kbps.clamp(200.0, Self::MAX_BITRATE_KBPS as f64) as u32
    }

    /// Audio bitrate for the given budget. 128 kbps normally, stepping
    /// down when the total budget is tight (a long clip squeezed into
    /// the low end of the slider) so audio doesn't eat the video's
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

    /// Bits per pixel per frame below which H.264 visibly falls apart —
    /// blocking, smeared texture, mushy motion. This is the line where
    /// switching to AV1 buys a whole resolution step rather than a
    /// marginal quality bump, so it's what `pick_codec` gates on.
    pub const AV1_BPP_THRESHOLD: f64 = 0.10;

    /// Resolution the budget sustains *for this codec*. A codec that
    /// needs less bitrate for the same picture effectively has a bigger
    /// budget, so the same 20 MB that only carries 720p on H.264 can
    /// carry 1080p on AV1 — which is the entire point of switching.
    ///
    /// Never returns a preset larger than the source: upscaling spends
    /// bits inventing pixels that were never shot.
    pub fn resolution_for(video_kbps: u32, codec: VideoCodec, src_h: u32) -> ResolutionPreset {
        let effective = (video_kbps as f32 / codec.bitrate_efficiency().max(0.01)) as u32;
        let preset = Self::resolution_for_bitrate(effective);
        Self::clamp_to_source(preset, src_h)
    }

    /// Collapse any preset that meets or exceeds the source height down
    /// to `Source` — no upscaling, and no scale filter at all when the
    /// budget can carry the native resolution.
    pub fn clamp_to_source(preset: ResolutionPreset, src_h: u32) -> ResolutionPreset {
        if src_h == 0 {
            return preset;
        }
        match preset.dimensions(16, 9) {
            // `dimensions` on a dummy 16:9 source still reports the
            // preset's own target height, which is all we need here.
            Some((_, h)) if h >= src_h => ResolutionPreset::Source,
            _ => preset,
        }
    }

    /// SVT-AV1 preset (0 slowest … 13 fastest) scaled by clip length, so
    /// a 30-second highlight gets a slow, efficient encode and a 20-minute
    /// VOD doesn't turn into a coffee break. Hardware AV1 encoders are
    /// fixed-function and ignore this.
    pub fn svt_preset_for(duration_ms: i64) -> &'static str {
        match duration_ms {
            ..=60_000 => "6",
            60_001..=300_000 => "8",
            300_001..=900_000 => "10",
            _ => "12",
        }
    }

    /// Best AV1 encoder available, preferring the one matching the
    /// detected GPU, then any other hardware AV1 encoder, then software
    /// SVT-AV1. `None` when this ffmpeg build has no AV1 encoder at all.
    pub fn best_av1(gpu: Option<GpuVendor>, available: &[VideoCodec]) -> Option<VideoCodec> {
        let vendor_match = match gpu {
            Some(GpuVendor::Nvidia) => Some(VideoCodec::NvencAv1),
            // VAAPI, not AMF: AMF needs the proprietary runtime installed,
            // while VAAPI AV1 encode comes free with Mesa on RDNA 3+.
            Some(GpuVendor::Amd) => Some(VideoCodec::VaapiAv1),
            Some(GpuVendor::Intel) => Some(VideoCodec::QsvAv1),
            _ => None,
        };
        if let Some(v) = vendor_match {
            if available.contains(&v) {
                return Some(v);
            }
        }
        [
            VideoCodec::VaapiAv1,
            VideoCodec::NvencAv1,
            VideoCodec::QsvAv1,
            VideoCodec::SvtAv1,
        ]
        .into_iter()
        .find(|c| available.contains(c))
    }

    /// Pick the codec that gets the most picture out of a fixed byte
    /// budget.
    ///
    /// The rule: stay on H.264 unless the budget is actually squeezing
    /// the image. "Squeezing" is measured in bits per pixel per frame at
    /// the resolution H.264 would be forced down to — below
    /// [`AV1_BPP_THRESHOLD`](Self::AV1_BPP_THRESHOLD) H.264 starts
    /// smearing detail, and that's exactly where AV1's bitrate advantage
    /// converts into a real resolution step. Above it, H.264 wins on the
    /// things that actually matter there: it embeds in every Discord
    /// client, and it encodes several times faster.
    ///
    /// Note that a 60 fps source hits the threshold at twice the bitrate
    /// a 30 fps one does — same pixels, twice as many of them per second.
    pub fn pick_codec(
        duration_ms: i64,
        target_bytes: i64,
        src_w: u32,
        src_h: u32,
        src_fps: f64,
        gpu: Option<GpuVendor>,
        available: &[VideoCodec],
    ) -> VideoCodec {
        let h264 = recommend_codec(gpu, available);
        let video_kbps = Self::video_kbps_for(duration_ms, target_bytes);
        let baseline = Self::resolution_for(video_kbps, h264, src_h);
        let (w, h) = baseline.dimensions(src_w, src_h).unwrap_or((src_w, src_h));
        // A 0 fps probe (or a still) shouldn't make the math explode;
        // 30 is the conservative assumption because it makes the budget
        // look roomier, i.e. it biases toward leaving H.264 alone.
        let fps = if src_fps > 1.0 { src_fps } else { 30.0 };
        let pixels_per_sec = w as f64 * h as f64 * fps;
        if pixels_per_sec <= 0.0 {
            return h264;
        }
        let bpp = (video_kbps as f64 * 1000.0) / pixels_per_sec;
        if bpp >= Self::AV1_BPP_THRESHOLD {
            return h264;
        }
        Self::best_av1(gpu, available).unwrap_or(h264)
    }

    /// Apply the user's codec row to the automatic choice. A forced AV1
    /// with no AV1 encoder on the machine falls back to H.264 rather
    /// than failing the export — the UI disables that button in the
    /// first place, so this is belt-and-braces.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_codec(
        choice: CodecChoice,
        duration_ms: i64,
        target_bytes: i64,
        src_w: u32,
        src_h: u32,
        src_fps: f64,
        gpu: Option<GpuVendor>,
        available: &[VideoCodec],
    ) -> VideoCodec {
        match choice {
            CodecChoice::Auto => Self::pick_codec(
                duration_ms,
                target_bytes,
                src_w,
                src_h,
                src_fps,
                gpu,
                available,
            ),
            CodecChoice::H264 => recommend_codec(gpu, available),
            CodecChoice::Av1 => {
                Self::best_av1(gpu, available).unwrap_or_else(|| recommend_codec(gpu, available))
            }
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
        let resolution = Self::resolution_for(video_kbps, codec, src_h).dimensions(src_w, src_h);
        let video_preset = if matches!(codec, VideoCodec::SvtAv1) {
            Self::svt_preset_for(duration_ms).to_string()
        } else {
            codec.default_preset().to_string()
        };
        EncodeSettings {
            resolution,
            video_codec: codec,
            // CBR for predictable size, regardless of what the codec
            // would normally default to.
            quality: EncodeQuality::Bitrate(video_kbps),
            video_preset,
            fps_limit: None,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: Self::audio_kbps_for(duration_ms, target_bytes),
        }
    }

    /// Estimated output file size in bytes for the chosen bitrate +
    /// duration. Measured encodes land within ~±2% of the bitrate math
    /// (muxer overhead included), and the budget already sits under the
    /// cap, so the plain bitrate × duration is the honest estimate.
    pub fn estimated_size_bytes(duration_ms: i64, target_bytes: i64) -> i64 {
        let video_kbps = Self::video_kbps_for(duration_ms, target_bytes) as i64;
        let total_kbps = video_kbps + Self::audio_kbps_for(duration_ms, target_bytes) as i64;
        total_kbps * duration_ms / 8
    }
}

/// What the user picked in Discord mode's codec row. `Auto` runs
/// [`DiscordNitro::pick_codec`]; the other two pin the family and let
/// the rest of the plan (bitrate, resolution) follow from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CodecChoice {
    #[default]
    Auto,
    H264,
    Av1,
}

impl CodecChoice {
    pub const ALL: &'static [CodecChoice] = &[Self::Auto, Self::H264, Self::Av1];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::H264 => "H.264",
            Self::Av1 => "AV1",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Auto => "Pick the codec that fits the most picture into the target size",
            Self::H264 => "Force H.264 — fastest encode, plays in every client",
            Self::Av1 => "Force AV1 — smaller for the same picture, slower to encode",
        }
    }
}

/// Everything the Discord export decides, computed once so the stats
/// panel and the export command can't drift apart: which codec, which
/// resolution, the bitrate split, and the size estimate.
#[derive(Clone, Debug)]
pub struct NitroPlan {
    pub codec: VideoCodec,
    pub resolution: ResolutionPreset,
    /// Output dimensions after the resolution preset is applied to the
    /// source. Equal to the source dimensions when no scaling happens.
    pub dims: (u32, u32),
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub est_bytes: i64,
    /// True when [`DiscordNitro::MAX_BITRATE_KBPS`] — not the user's
    /// target — is what's limiting the encode.
    pub cap_dominates: bool,
    /// What H.264 alone would have managed, so the UI can say what the
    /// codec choice actually bought.
    pub h264_resolution: ResolutionPreset,
    /// What the best available AV1 encoder would manage, or `None` when
    /// this machine has no AV1 encoder. Lets the note tell a user who
    /// forced H.264 what they're leaving on the table.
    pub av1_resolution: Option<ResolutionPreset>,
    /// The user's codec row setting, and what Auto would have landed on.
    pub choice: CodecChoice,
    pub auto_codec: VideoCodec,
    pub settings: EncodeSettings,
}

impl NitroPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn compute(
        duration_ms: i64,
        target_bytes: i64,
        src_w: u32,
        src_h: u32,
        src_fps: f64,
        choice: CodecChoice,
        gpu: Option<GpuVendor>,
        available: &[VideoCodec],
    ) -> Self {
        let auto_codec = DiscordNitro::pick_codec(
            duration_ms,
            target_bytes,
            src_w,
            src_h,
            src_fps,
            gpu,
            available,
        );
        let codec = DiscordNitro::resolve_codec(
            choice,
            duration_ms,
            target_bytes,
            src_w,
            src_h,
            src_fps,
            gpu,
            available,
        );
        let video_kbps = DiscordNitro::video_kbps_for(duration_ms, target_bytes);
        let audio_kbps = DiscordNitro::audio_kbps_for(duration_ms, target_bytes);
        let resolution = DiscordNitro::resolution_for(video_kbps, codec, src_h);
        let h264_resolution =
            DiscordNitro::resolution_for(video_kbps, recommend_codec(gpu, available), src_h);
        let av1_resolution = DiscordNitro::best_av1(gpu, available)
            .map(|c| DiscordNitro::resolution_for(video_kbps, c, src_h));
        Self {
            codec,
            resolution,
            dims: resolution
                .dimensions(src_w, src_h)
                .unwrap_or((src_w, src_h)),
            video_kbps,
            audio_kbps,
            est_bytes: DiscordNitro::estimated_size_bytes(duration_ms, target_bytes),
            cap_dominates: DiscordNitro::total_kbps_for(duration_ms, target_bytes)
                >= DiscordNitro::MAX_BITRATE_KBPS,
            h264_resolution,
            av1_resolution,
            choice,
            auto_codec,
            settings: DiscordNitro::settings_for(duration_ms, target_bytes, src_w, src_h, codec),
        }
    }

    /// Estimate in decimal MB — the same unit as the target slider.
    pub fn est_mb(&self) -> f64 {
        self.est_bytes as f64 / 1_000_000.0
    }

    /// One line explaining the codec, for the stats panel. On Auto it
    /// justifies the pick; on a manual override it says what the
    /// override costs or gains against what Auto would have done.
    pub fn codec_note(&self) -> String {
        // "Source" is a scaling instruction, not something to read out.
        let spell = |r: ResolutionPreset| match r {
            ResolutionPreset::Source => "full source resolution".to_string(),
            other => other.label().to_string(),
        };

        if self.choice != CodecChoice::Auto {
            let overridden = self.codec != self.auto_codec;
            return match (self.codec.is_av1(), overridden) {
                // Forced H.264 where Auto wanted AV1 — name the cost.
                (false, true) => match self.av1_resolution {
                    Some(av1_res) if av1_res != self.resolution => format!(
                        "H.264, your pick. Auto would have used AV1 here and held \
                         {} instead of {} — H.264 encodes faster and plays in \
                         every client.",
                        spell(av1_res),
                        spell(self.resolution),
                    ),
                    _ => "H.264, your pick. Auto would have used AV1 at this \
                          bitrate for cleaner detail; H.264 encodes faster and \
                          plays in every client."
                        .to_string(),
                },
                // Forced AV1 where Auto was happy with H.264.
                (true, true) => "AV1, your pick. The budget is roomy enough that \
                                  Auto would have stayed on H.264 — AV1 still \
                                  encodes smaller, it just takes longer and needs \
                                  a client that decodes AV1."
                    .to_string(),
                // Same answer Auto would have given.
                (true, false) => "AV1, your pick — and what Auto would have \
                                   chosen at this target. Discord embeds AV1 in \
                                   MP4."
                    .to_string(),
                (false, false) => "H.264, your pick — and what Auto would have \
                                    chosen at this target."
                    .to_string(),
            };
        }

        if !self.codec.is_av1() {
            return format!(
                "{} — the budget is roomy enough that H.264 holds up, and it \
                 plays everywhere.",
                self.codec.display(),
            );
        }
        if self.resolution != self.h264_resolution {
            format!(
                "AV1 — needs ~{:.0}% less bitrate than H.264, which buys {} \
                 instead of {} at this target. Discord embeds AV1 in MP4.",
                (1.0 - self.codec.bitrate_efficiency()) * 100.0,
                spell(self.resolution),
                spell(self.h264_resolution),
            )
        } else {
            format!(
                "AV1 — the bitrate is thin for {}, so AV1's efficiency goes \
                 into cleaner detail instead of a smaller frame. Discord \
                 embeds AV1 in MP4.",
                spell(self.resolution),
            )
        }
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

/// Render node used for VAAPI encode/decode. Single source of truth so
/// the capability probe and the export command talk to the same device.
pub const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

/// Probe whether an encoder actually *works* here, not just whether
/// ffmpeg lists it.
///
/// `ffmpeg -encoders` reports everything the build was compiled with, so
/// an RDNA 2 card still advertises `av1_vaapi` (no AV1 encode block) and
/// a machine with no NVIDIA driver still advertises `av1_nvenc`. Without
/// this check the failure surfaces as a dead export minutes in. A
/// 2-frame encode to the null muxer settles it in under 100 ms.
pub fn encoder_works(codec: VideoCodec) -> bool {
    let mut cmd = crate::platform::command("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc=d=0.2:s=320x180:r=10",
    ]);
    if codec.needs_vaapi_upload() {
        cmd.args(["-vaapi_device", VAAPI_DEVICE, "-vf", "format=nv12,hwupload"]);
    }
    cmd.args(["-c:v", codec.ffmpeg_name(), "-f", "null", "-"]);
    match cmd.output() {
        Ok(out) => out.status.success(),
        Err(e) => {
            log::warn!(
                "encoder probe for {} failed to run: {e}",
                codec.ffmpeg_name()
            );
            false
        }
    }
}

/// Drop hardware encoders that ffmpeg advertises but the machine can't
/// actually run. `ffmpeg -encoders` lists everything the build was
/// compiled with: an RDNA 2 card advertises `av1_vaapi` with no AV1
/// encode block, a box with no NVIDIA driver advertises `*_nvenc`, and a
/// Mesa built without H.264/HEVC encode (Fedora, some others) still
/// advertises `h264_vaapi`. The Discord planner and the startup
/// recommendation pick from this list unattended, so every hardware
/// entry gets a real test encode. The probes run in parallel (each is
/// ~100 ms, mostly process startup) so startup doesn't pay for them in
/// series. Software encoders are linked in and always run.
pub fn verify_hw_encoders(codecs: Vec<VideoCodec>) -> Vec<VideoCodec> {
    let works: Vec<bool> = std::thread::scope(|scope| {
        let probes: Vec<_> = codecs
            .iter()
            .map(|&c| scope.spawn(move || c.is_software() || encoder_works(c)))
            .collect();
        probes
            .into_iter()
            .map(|h| h.join().unwrap_or(false))
            .collect()
    });
    codecs
        .into_iter()
        .zip(works)
        .filter_map(|(c, ok)| {
            if !ok {
                log::info!(
                    "{} advertised but not usable here — dropping",
                    c.ffmpeg_name()
                );
            }
            ok.then_some(c)
        })
        .collect()
}

/// Probe `ffmpeg -encoders` for the optional encoders this build has —
/// every hardware encoder plus software AV1, which is a separate library
/// (SVT-AV1) and can't be assumed present the way libx264 can.
/// Synchronous — the command is fast (≲ 100ms) and only runs once at
/// startup.
pub fn detect_available_codecs() -> Vec<VideoCodec> {
    let Ok(out) = crate::platform::command("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
    else {
        log::warn!("ffmpeg -encoders probe failed; no optional codecs offered");
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let candidates = [
        ("libsvtav1", VideoCodec::SvtAv1),
        ("h264_nvenc", VideoCodec::NvencH264),
        ("hevc_nvenc", VideoCodec::NvencHevc),
        ("av1_nvenc", VideoCodec::NvencAv1),
        ("h264_vaapi", VideoCodec::VaapiH264),
        ("hevc_vaapi", VideoCodec::VaapiHevc),
        ("av1_vaapi", VideoCodec::VaapiAv1),
        ("h264_qsv", VideoCodec::QsvH264),
        ("hevc_qsv", VideoCodec::QsvHevc),
        ("av1_qsv", VideoCodec::QsvAv1),
        ("h264_amf", VideoCodec::AmfH264),
        ("hevc_amf", VideoCodec::AmfHevc),
        ("av1_amf", VideoCodec::AmfAv1),
    ];
    candidates
        .iter()
        .filter(|(name, codec)| {
            text.contains(name) && !(cfg!(windows) && codec.needs_vaapi_upload())
        })
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
    fn discord_budget_leaves_headroom_under_every_tier_cap() {
        // The plan's own size must sit under the cap with room to spare,
        // or every encoder that runs a little hot uploads-fails. (Long
        // clips that hit the 200 kbps floor are excluded — the
        // post-encode size check owns those.)
        for &(_, mb) in DiscordNitro::TIERS {
            let cap = DiscordNitro::target_bytes_from_mb(mb);
            for &secs in &[5i64, 15, 30, 60, 120, 300] {
                let dur = secs * 1000;
                if DiscordNitro::total_kbps_for(dur, cap) <= 200 {
                    continue;
                }
                let est = DiscordNitro::estimated_size_bytes(dur, cap);
                assert!(
                    est <= cap * 96 / 100,
                    "{mb} MB tier, {secs}s: plan {est} B has <4% headroom under {cap} B"
                );
            }
        }
    }

    #[test]
    fn hardware_encoders_fall_back_to_cpu_for_the_same_format() {
        let with_svt = [VideoCodec::SvtAv1, VideoCodec::VaapiAv1];
        assert_eq!(
            VideoCodec::VaapiAv1.cpu_fallback(&with_svt),
            Some(VideoCodec::SvtAv1)
        );
        // No SVT-AV1 in this ffmpeg → a working H.264 file instead.
        assert_eq!(
            VideoCodec::NvencAv1.cpu_fallback(&[]),
            Some(VideoCodec::X264)
        );
        assert_eq!(
            VideoCodec::VaapiH264.cpu_fallback(&[]),
            Some(VideoCodec::X264)
        );
        assert_eq!(
            VideoCodec::QsvHevc.cpu_fallback(&[]),
            Some(VideoCodec::X265)
        );
        for sw in [VideoCodec::X264, VideoCodec::X265, VideoCodec::SvtAv1] {
            assert_eq!(sw.cpu_fallback(&with_svt), None);
        }
    }

    #[test]
    fn discord_cap_is_decimal_megabytes() {
        // 20 MB means 20,000,000 bytes — the smaller reading, so the file
        // fits whichever unit the uploader counts in.
        assert_eq!(DiscordNitro::target_bytes_from_mb(20), 20_000_000);
    }

    #[test]
    fn refit_scales_video_down_by_the_overshoot() {
        let cap = DiscordNitro::target_bytes_from_mb(20);
        // 60 s, encoder ran 10% over the cap.
        let new = DiscordNitro::refit_video_kbps(2_400, 128, 60_000, cap * 110 / 100, cap)
            .expect("room to shrink");
        assert!(
            new < 2_400 * 100 / 110,
            "refit {new} kbps doesn't undo a 10% overshoot"
        );
        // Re-run the math as if the encoder overshot by the same factor
        // again: the result must land under the cap.
        let audio_bytes = 128 * 60_000 / 8;
        let video_bytes = new as i64 * 60_000 / 8 * 110 / 100;
        assert!(audio_bytes + video_bytes <= cap, "refit still over the cap");
        // Always strictly smaller, even for a 1-byte overshoot.
        let tiny = DiscordNitro::refit_video_kbps(2_400, 128, 60_000, cap + 1, cap).unwrap();
        assert!(tiny < 2_400);
        // Audio alone over the budget → nothing left to shrink.
        assert_eq!(
            DiscordNitro::refit_video_kbps(100, 128, 10 * 60 * 60_000, cap * 2, cap),
            None
        );
    }

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
        // 5-minute clip into the smallest target the slider offers.
        // Audio must drop below 128 kbps or it would eat most of the
        // ~273 kbps total budget.
        let dur = 5 * 60 * 1000;
        let target = DiscordNitro::target_bytes_from_mb(10);
        let audio = DiscordNitro::audio_kbps_for(dur, target);
        assert!(audio < 128, "expected reduced audio bitrate, got {audio}");
        let est = DiscordNitro::estimated_size_bytes(dur, target);
        let est_mb = est as f64 / (1024.0 * 1024.0);
        assert!(est_mb <= 10.5, "10 MB target estimated at {est_mb:.1} MB");
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
        let kbps_small = DiscordNitro::video_kbps_for(dur, DiscordNitro::target_bytes_from_mb(100));
        let kbps_full = DiscordNitro::video_kbps_for(
            dur,
            DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB),
        );
        assert!(
            kbps_small < kbps_full,
            "smaller target should use less bitrate (small={kbps_small}, full={kbps_full})"
        );

        let est_100 =
            DiscordNitro::estimated_size_bytes(dur, DiscordNitro::target_bytes_from_mb(100));
        let mb_100 = est_100 as f64 / (1024.0 * 1024.0);
        // Should be near (but not above) 100 MB.
        assert!(
            mb_100 < 100.0,
            "100 MB target produced {mb_100:.1} MB estimate"
        );
        assert!(
            mb_100 > 70.0,
            "100 MB target should land near 100 MB ({mb_100:.1})"
        );
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
            VideoCodec::SvtAv1,
            VideoCodec::NvencH264,
            VideoCodec::NvencAv1,
            VideoCodec::VaapiH264,
            VideoCodec::VaapiAv1,
            VideoCodec::QsvH264,
            VideoCodec::QsvAv1,
            VideoCodec::AmfH264,
            VideoCodec::AmfAv1,
        ] {
            assert!(
                c.quality_range().contains(&c.default_quality()),
                "{c:?} default quality outside its slider range"
            );
        }
    }

    // ---- AV1 ------------------------------------------------------------

    /// The case that motivated this: a 1080p60 clip squeezed into
    /// Discord's free 20 MB tier. H.264 would drop it to 480/720p;
    /// AV1 is worth the slower encode there.
    #[test]
    fn tight_budget_picks_av1_when_one_is_available() {
        let codec = DiscordNitro::pick_codec(
            5 * 60 * 1000,
            DiscordNitro::target_bytes_from_mb(20),
            1920,
            1080,
            60.0,
            Some(GpuVendor::Amd),
            &[VideoCodec::VaapiH264, VideoCodec::VaapiAv1],
        );
        assert_eq!(codec, VideoCodec::VaapiAv1);
    }

    /// A 30-second clip at the 475 MB Nitro target has bitrate to burn.
    /// H.264 is transparent there and encodes far faster, so no switch.
    #[test]
    fn roomy_budget_stays_on_h264() {
        let codec = DiscordNitro::pick_codec(
            30 * 1000,
            DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB),
            1920,
            1080,
            60.0,
            Some(GpuVendor::Amd),
            &[VideoCodec::VaapiH264, VideoCodec::VaapiAv1],
        );
        assert_eq!(codec, VideoCodec::VaapiH264);
    }

    #[test]
    fn tight_budget_falls_back_to_h264_without_an_av1_encoder() {
        let codec = DiscordNitro::pick_codec(
            5 * 60 * 1000,
            DiscordNitro::target_bytes_from_mb(20),
            1920,
            1080,
            60.0,
            Some(GpuVendor::Amd),
            &[VideoCodec::VaapiH264],
        );
        assert_eq!(codec, VideoCodec::VaapiH264);
    }

    #[test]
    fn software_av1_is_used_when_no_hardware_av1_exists() {
        let codec = DiscordNitro::pick_codec(
            5 * 60 * 1000,
            DiscordNitro::target_bytes_from_mb(20),
            1920,
            1080,
            60.0,
            None,
            &[VideoCodec::SvtAv1],
        );
        assert_eq!(codec, VideoCodec::SvtAv1);
    }

    #[test]
    fn sixty_fps_reaches_the_av1_threshold_before_thirty_does() {
        let args = |fps| {
            DiscordNitro::pick_codec(
                90 * 1000,
                DiscordNitro::target_bytes_from_mb(35),
                1920,
                1080,
                fps,
                None,
                &[VideoCodec::SvtAv1],
            )
        };
        // Same clip length and budget (≈3.1 Mbps of video at 720p): at
        // 30 fps that's 0.11 bpp and H.264 still holds; at 60 fps the
        // same bits are split over twice as many frames — 0.06 bpp —
        // and AV1 takes over.
        assert_eq!(args(30.0), VideoCodec::X264);
        assert_eq!(args(60.0), VideoCodec::SvtAv1);
    }

    #[test]
    fn av1_buys_a_resolution_step_at_the_same_bitrate() {
        let kbps = 3_000;
        let h264 = DiscordNitro::resolution_for(kbps, VideoCodec::X264, 2160);
        let av1 = DiscordNitro::resolution_for(kbps, VideoCodec::SvtAv1, 2160);
        assert_eq!(h264, ResolutionPreset::P720);
        assert_eq!(av1, ResolutionPreset::P1080);
    }

    #[test]
    fn resolution_never_upscales_past_the_source() {
        // 20 Mbps of AV1 would nominally justify 1440p, but a 1080p
        // source has no 1440p to give.
        let res = DiscordNitro::resolution_for(20_000, VideoCodec::SvtAv1, 1080);
        assert_eq!(res, ResolutionPreset::Source);
        assert_eq!(
            DiscordNitro::clamp_to_source(ResolutionPreset::P1440, 1080),
            ResolutionPreset::Source
        );
        assert_eq!(
            DiscordNitro::clamp_to_source(ResolutionPreset::P720, 1080),
            ResolutionPreset::P720
        );
    }

    #[test]
    fn svtav1_bitrate_mode_omits_maxrate() {
        // libsvtav1 hard-errors on -maxrate outside CRF mode, which
        // would kill every Discord export that picked software AV1.
        let s = DiscordNitro::settings_for(
            10 * 60 * 1000,
            DiscordNitro::target_bytes_from_mb(20),
            1920,
            1080,
            VideoCodec::SvtAv1,
        );
        let args = s.video_args();
        assert!(args.iter().any(|a| a == "-b:v"));
        assert!(args.iter().any(|a| a == "-bufsize"));
        assert!(
            !args.iter().any(|a| a == "-maxrate"),
            "libsvtav1 rejects -maxrate in bitrate mode: {args:?}"
        );
        // Every other codec keeps the peak cap.
        let vaapi = DiscordNitro::settings_for(
            10 * 60 * 1000,
            DiscordNitro::target_bytes_from_mb(20),
            1920,
            1080,
            VideoCodec::VaapiAv1,
        );
        assert!(vaapi.video_args().iter().any(|a| a == "-maxrate"));
    }

    #[test]
    fn svtav1_constant_quality_uses_crf() {
        let mut s = settings_for(VideoCodec::SvtAv1);
        s.quality = EncodeQuality::ConstantQuality(30);
        let args = s.video_args();
        assert!(args.windows(2).any(|w| w[0] == "-crf" && w[1] == "30"));
    }

    #[test]
    fn vaapi_av1_constant_quality_uses_global_quality_not_qp() {
        let mut s = settings_for(VideoCodec::VaapiAv1);
        s.quality = EncodeQuality::ConstantQuality(110);
        let args = s.video_args();
        // av1_vaapi has no -qp option of its own — passing one makes
        // ffmpeg reject the command outright.
        assert!(args.windows(2).any(|w| w[0] == "-rc_mode" && w[1] == "CQP"));
        assert!(args.windows(2).any(|w| w[0] == "-q:v" && w[1] == "110"));
        assert!(!args.iter().any(|a| a == "-qp"));
    }

    #[test]
    fn svt_preset_gets_faster_as_clips_get_longer() {
        let p = |ms| DiscordNitro::svt_preset_for(ms).parse::<u32>().unwrap();
        assert!(p(30 * 1000) < p(4 * 60 * 1000));
        assert!(p(4 * 60 * 1000) < p(10 * 60 * 1000));
        assert!(p(10 * 60 * 1000) < p(60 * 60 * 1000));
        assert!(p(60 * 60 * 1000) <= 13, "SVT-AV1 presets stop at 13");
    }

    #[test]
    fn nitro_plan_settings_match_its_reported_codec_and_resolution() {
        let plan = NitroPlan::compute(
            5 * 60 * 1000,
            DiscordNitro::target_bytes_from_mb(20),
            1920,
            1080,
            60.0,
            CodecChoice::Auto,
            Some(GpuVendor::Amd),
            &[VideoCodec::VaapiH264, VideoCodec::VaapiAv1],
        );
        assert_eq!(plan.settings.video_codec, plan.codec);
        assert_eq!(
            plan.settings.resolution,
            plan.resolution.dimensions(1920, 1080)
        );
        assert!(plan.codec.is_av1());
        assert!(plan.codec_note().contains("AV1"));
    }

    #[test]
    fn manual_override_beats_the_automatic_pick_both_ways() {
        let avail = [VideoCodec::VaapiH264, VideoCodec::VaapiAv1];
        let gpu = Some(GpuVendor::Amd);
        // Tight budget: Auto wants AV1, but H.264 was asked for.
        let tight = |choice| {
            DiscordNitro::resolve_codec(
                choice,
                5 * 60 * 1000,
                DiscordNitro::target_bytes_from_mb(20),
                1920,
                1080,
                60.0,
                gpu,
                &avail,
            )
        };
        assert_eq!(tight(CodecChoice::Auto), VideoCodec::VaapiAv1);
        assert_eq!(tight(CodecChoice::H264), VideoCodec::VaapiH264);
        // Roomy budget: Auto wants H.264, but AV1 was asked for.
        let roomy = |choice| {
            DiscordNitro::resolve_codec(
                choice,
                30 * 1000,
                DiscordNitro::target_bytes_from_mb(DiscordNitro::MAX_TARGET_MB),
                1920,
                1080,
                60.0,
                gpu,
                &avail,
            )
        };
        assert_eq!(roomy(CodecChoice::Auto), VideoCodec::VaapiH264);
        assert_eq!(roomy(CodecChoice::Av1), VideoCodec::VaapiAv1);
    }

    #[test]
    fn forcing_av1_without_an_av1_encoder_falls_back_instead_of_failing() {
        let codec = DiscordNitro::resolve_codec(
            CodecChoice::Av1,
            60 * 1000,
            DiscordNitro::target_bytes_from_mb(20),
            1920,
            1080,
            60.0,
            Some(GpuVendor::Amd),
            &[VideoCodec::VaapiH264],
        );
        assert_eq!(codec, VideoCodec::VaapiH264);
    }

    #[test]
    fn override_note_says_what_the_choice_costs() {
        let plan = |choice| {
            NitroPlan::compute(
                5 * 60 * 1000,
                DiscordNitro::target_bytes_from_mb(20),
                1920,
                1080,
                60.0,
                choice,
                Some(GpuVendor::Amd),
                &[VideoCodec::VaapiH264, VideoCodec::VaapiAv1],
            )
        };
        // Auto picks AV1 here, so forcing H.264 is a real trade-off and
        // the panel has to say so rather than silently obeying.
        let forced = plan(CodecChoice::H264);
        assert_eq!(forced.codec, VideoCodec::VaapiH264);
        assert_eq!(forced.auto_codec, VideoCodec::VaapiAv1);
        assert!(forced.codec_note().contains("your pick"));
        assert!(forced.codec_note().contains("Auto would have"));
        // Auto's own note never claims the user picked anything.
        assert!(!plan(CodecChoice::Auto).codec_note().contains("your pick"));
    }

    #[test]
    fn best_av1_prefers_the_vendor_matched_encoder() {
        let all = [
            VideoCodec::SvtAv1,
            VideoCodec::NvencAv1,
            VideoCodec::VaapiAv1,
            VideoCodec::QsvAv1,
        ];
        assert_eq!(
            DiscordNitro::best_av1(Some(GpuVendor::Nvidia), &all),
            Some(VideoCodec::NvencAv1)
        );
        assert_eq!(
            DiscordNitro::best_av1(Some(GpuVendor::Amd), &all),
            Some(VideoCodec::VaapiAv1)
        );
        // AMD box whose ffmpeg lacks av1_vaapi still gets software AV1,
        // never the NVIDIA encoder.
        assert_eq!(
            DiscordNitro::best_av1(Some(GpuVendor::Amd), &[VideoCodec::SvtAv1]),
            Some(VideoCodec::SvtAv1)
        );
        assert_eq!(DiscordNitro::best_av1(Some(GpuVendor::Amd), &[]), None);
    }

    #[test]
    fn av1_encoders_are_flagged_and_more_efficient_than_h264() {
        for c in [
            VideoCodec::SvtAv1,
            VideoCodec::NvencAv1,
            VideoCodec::VaapiAv1,
            VideoCodec::QsvAv1,
            VideoCodec::AmfAv1,
        ] {
            assert!(c.is_av1(), "{c:?} should report as AV1");
            assert!(c.bitrate_efficiency() < VideoCodec::X264.bitrate_efficiency());
        }
        // VAAPI AV1 rides the same hwupload path as the other VAAPI
        // encoders — without this the frames never reach the GPU.
        assert!(VideoCodec::VaapiAv1.needs_vaapi_upload());
        assert_eq!(VideoCodec::VaapiAv1.hw_decode(), HwDecode::Vaapi);
    }

    // ---- Recommendation ------------------------------------------------

    #[test]
    fn recommend_falls_back_to_x264_when_no_hw_available() {
        assert_eq!(recommend_codec(None, &[]), VideoCodec::X264);
        assert_eq!(
            recommend_codec(Some(GpuVendor::Nvidia), &[]),
            VideoCodec::X264
        );
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
