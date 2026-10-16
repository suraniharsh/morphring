//! Which container and codecs MorphRing writes for each video and audio format,
//! and the ffmpeg command lines that do it. Ported from Filee's
//! `src/Filee.Engines/Media/MediaEncoding.cs`, remux matrix included.
//!
//! Everything here is pure: no processes, no file system. The engine (T24)
//! probes the input, hands the facts in as a [`Source`] along with the user's
//! [`ConvertOptions`], and runs the passes.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use morphring_core::{ConvertOptions, Edge, EngineError, MediaQuality, QualityLevel, WavFormat};

/// Video formats ffmpeg reads and writes in v0.1.0.
pub const VIDEO_FORMATS: [&str; 5] = ["mp4", "webm", "mkv", "mov", "avi"];

/// Audio formats ffmpeg reads and writes in v0.1.0 (also the targets of audio
/// extraction from video).
pub const AUDIO_FORMATS: [&str; 6] = ["mp3", "m4a", "wav", "flac", "ogg", "opus"];

/// Formats an animated GIF can be turned into.
pub const GIF_TO_VIDEO: [&str; 2] = ["mp4", "webm"];

/// Cost of video -> gif. It is slow and squeezes everything into 256 colours,
/// so video -> jpg should go through the png poster frame instead.
pub const VIDEO_TO_GIF_COST: u32 = 15;

const V: usize = VIDEO_FORMATS.len();
const A: usize = AUDIO_FORMATS.len();
const EDGE_COUNT: usize = V * V + V * A + A * A + V + GIF_TO_VIDEO.len() + V;

/// Every direct conversion the ffmpeg engine performs.
pub static EDGES: [Edge; EDGE_COUNT] = build_edges();

// A const fn with while loops, because I wanted EDGES to be a plain static slice
// and iterators are still not allowed in const land. It's 1970 in here, but it works.
const fn build_edges() -> [Edge; EDGE_COUNT] {
    let mut edges = [Edge::new("", ""); EDGE_COUNT];
    let mut n = 0;
    let mut i = 0;
    while i < V {
        let from = VIDEO_FORMATS[i];
        let mut j = 0;
        // video -> video, self edges included (a self edge re-encodes).
        while j < V {
            edges[n] = Edge::new(from, VIDEO_FORMATS[j]);
            n += 1;
            j += 1;
        }
        // video -> audio: extract the soundtrack.
        j = 0;
        while j < A {
            edges[n] = Edge::new(from, AUDIO_FORMATS[j]);
            n += 1;
            j += 1;
        }
        edges[n] = Edge::with_cost(from, "gif", VIDEO_TO_GIF_COST);
        n += 1;
        // The poster frame is the doorway from video to every image format.
        edges[n] = Edge::new(from, "png");
        n += 1;
        i += 1;
    }
    i = 0;
    while i < A {
        let mut j = 0;
        while j < A {
            edges[n] = Edge::new(AUDIO_FORMATS[i], AUDIO_FORMATS[j]);
            n += 1;
            j += 1;
        }
        i += 1;
    }
    i = 0;
    while i < GIF_TO_VIDEO.len() {
        edges[n] = Edge::new("gif", GIF_TO_VIDEO[i]);
        n += 1;
        i += 1;
    }
    assert!(n == EDGE_COUNT, "EDGE_COUNT does not match the edges built");
    edges
}

/// What the probe found out about the input, as plain values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Source<'a> {
    /// The first real video stream (cover pictures excluded), if any.
    pub video: Option<VideoSource<'a>>,
    /// The first audio stream, if any.
    pub audio: Option<AudioSource<'a>>,
    /// Duration in seconds, `None` when unknown.
    pub duration: Option<f64>,
}

/// Facts about the input's video stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoSource<'a> {
    /// ffprobe `codec_name`, such as `h264`.
    pub codec: Option<&'a str>,
    /// ffprobe `pix_fmt`, such as `yuv420p`.
    pub pix_fmt: Option<&'a str>,
    /// ffprobe `codec_tag_string`, the FourCC such as `hev1` or `XVID`.
    pub codec_tag: Option<&'a str>,
    /// Stored width in pixels (before rotation), 0 when unknown.
    pub width: u32,
    /// Stored height in pixels (before rotation), 0 when unknown.
    pub height: u32,
    /// Height as displayed (the stored width for a portrait phone clip), 0 when unknown.
    pub display_height: u32,
    /// Average frame rate, `None` when unknown.
    pub fps: Option<f64>,
}

/// Facts about the input's audio stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioSource<'a> {
    /// ffprobe `codec_name`, such as `aac`.
    pub codec: Option<&'a str>,
    /// ffprobe `codec_tag_string`, such as `mp4a`.
    pub codec_tag: Option<&'a str>,
    /// Channel count, 0 when unknown.
    pub channels: u32,
}

/// The quality bucket a conversion encodes at.
///
/// The command line's `-q` wins when it is given; otherwise the preset's media
/// quality decides (Balanced is what everyone else calls Normal).
pub fn quality_of(options: &ConvertOptions) -> QualityLevel {
    if options.quality.is_some() {
        return options.quality_level();
    }
    match options.media.quality {
        MediaQuality::High => QualityLevel::High,
        MediaQuality::Balanced => QualityLevel::Normal,
        MediaQuality::Small => QualityLevel::Small,
    }
}

/// One run of ffmpeg.
#[derive(Clone, Debug, PartialEq)]
pub struct FfmpegPass {
    /// Arguments after the executable.
    pub args: Vec<OsString>,
    /// Share of the whole conversion's progress; the passes of one plan add up to 1.
    pub weight: f32,
}

/// Global arguments of every run: no banner, no keyboard input, no stats line,
/// only errors on stderr, overwrite the output.
pub const COMMON: [&str; 6] = ["-hide_banner", "-nostdin", "-nostats", "-v", "error", "-y"];

/// How one target format is written: container, video encoder, audio encoder.
struct Target {
    muxer: &'static str,
    video: Option<&'static str>,
    audio: &'static str,
}

fn target_spec(format: &str) -> Option<Target> {
    let (muxer, video, audio) = match format {
        "mp4" => ("mp4", Some("libx264"), "aac"),
        "mov" => ("mov", Some("libx264"), "aac"),
        "mkv" => ("matroska", Some("libx264"), "aac"),
        // WebM only allows VP8/VP9/AV1 with Vorbis/Opus.
        "webm" => ("webm", Some("libvpx-vp9"), "libopus"),
        // MPEG-4 part 2 tagged XVID with CBR MP3: what old TVs and DVD players read in AVI.
        "avi" => ("avi", Some("mpeg4"), "libmp3lame"),
        "mp3" => ("mp3", None, "libmp3lame"),
        // The iPod muxer writes the M4A brand that phones and iTunes expect.
        "m4a" => ("ipod", None, "aac"),
        "wav" => ("wav", None, "pcm_s16le"),
        "flac" => ("flac", None, "flac"),
        "ogg" => ("ogg", None, "libvorbis"),
        "opus" => ("opus", None, "libopus"),
        _ => return None,
    };
    Some(Target {
        muxer,
        video,
        audio,
    })
}

/// The ffmpeg runs that turn `input` into `output` in format `to`.
///
/// `palette` is a scratch path for the GIF colour palette; it is only used
/// when `to` is `gif` and should sit in the job's work directory. Of the
/// options, GIFs and poster frames only care about the quality.
///
/// # Errors
///
/// Fails when ffmpeg cannot write `to`, or when the source lacks the stream the
/// target needs (no video for a video target, no audio for an audio target).
pub fn plan(
    input: &Path,
    output: &Path,
    to: &str,
    options: &ConvertOptions,
    source: &Source<'_>,
    palette: &Path,
) -> Result<Vec<FfmpegPass>, EngineError> {
    let single = |args| vec![FfmpegPass { args, weight: 1.0 }];
    match to {
        "gif" => {
            let [first, second] = gif_args(input, output, quality_of(options), source, palette)?;
            Ok(vec![
                FfmpegPass {
                    args: first,
                    weight: 0.3,
                },
                FfmpegPass {
                    args: second,
                    weight: 0.7,
                },
            ])
        }
        "png" => Ok(single(poster_args(input, output, source)?)),
        _ => match target_spec(to) {
            Some(spec) if spec.video.is_some() => {
                Ok(single(video_args(input, output, to, options, source)?))
            }
            Some(_) => Ok(single(audio_args(input, output, to, options, source)?)),
            None => Err(EngineError::Failed(format!("FFmpeg cannot write '{to}'."))),
        },
    }
}

/// Small builder so the arg lists below read like the command lines they are.
struct Args(Vec<OsString>);

impl Args {
    fn new() -> Self {
        Self(COMMON.iter().map(OsString::from).collect())
    }

    fn push(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.0.push(arg.as_ref().to_os_string());
        self
    }

    fn extend<I>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator,
        I::Item: AsRef<OsStr>,
    {
        for arg in args {
            self.push(arg);
        }
        self
    }

    fn finish(&mut self) -> Vec<OsString> {
        std::mem::take(&mut self.0)
    }
}

fn no_video() -> EngineError {
    EngineError::Failed("The file has no video stream.".into())
}

// ───────────────────────── Video ─────────────────────────

/// Arguments for a video target (mp4, webm, mkv, mov, avi).
///
/// # Errors
///
/// Fails when the source has no video stream or `to` is not a video format.
pub fn video_args(
    input: &Path,
    output: &Path,
    to: &str,
    options: &ConvertOptions,
    source: &Source<'_>,
) -> Result<Vec<OsString>, EngineError> {
    let video = source.video.as_ref().ok_or_else(no_video)?;
    let quality = quality_of(options);
    let media = &options.media;
    // Asked to drop the soundtrack: as far as the rest of this function is
    // concerned, there never was one.
    let audio = source.audio.as_ref().filter(|_| !media.remove_audio);
    let spec = target_spec(to)
        .filter(|s| s.video.is_some())
        .ok_or_else(|| EngineError::Failed(format!("'{to}' is not a video format.")))?;
    let video_codec = spec.video.unwrap_or("libx264");

    let mut a = Args::new();
    // 0:V (capital V) skips attached cover pictures, which ffmpeg otherwise
    // happily treats as a one-frame "video" stream.
    a.push("-i").push(input).extend(["-map", "0:V:0"]);
    if audio.is_some() {
        a.extend(["-map", "0:a:0"]);
    }

    let shrink = needs_downscale(video, media.max_height);
    if !shrink && can_copy_video(to, video, quality) {
        a.extend(["-c:v", "copy"]);
        a.extend(copy_tag(to, video));
    } else {
        let filter = if media.max_height > 0 {
            downscale_filter(media.max_height)
        } else {
            scale_filter().to_owned()
        };
        a.push("-vf").push(filter);
        a.extend(video_codec_args(video_codec, quality, video));
    }

    let overrides = AudioOverrides::from(media);
    match audio {
        None => {
            a.push("-an");
        }
        Some(audio) if can_copy_audio(to, audio, quality, overrides) => {
            a.extend(["-c:a", "copy"]);
        }
        Some(audio) => {
            a.extend(audio_codec_args(
                spec.audio, quality, audio, overrides, true,
            ));
            a.extend(channel_args(overrides));
        }
    }

    a.extend(metadata_args());
    a.extend(container_args(to));
    // Sparse or late-starting streams otherwise die with "too many packets buffered".
    a.extend(["-max_muxing_queue_size", "4096", "-f", spec.muxer])
        .extend(["-progress", "pipe:1"])
        .push(output);
    Ok(a.finish())
}

/// Video codecs each container takes as they are.
///
/// A video whose codec is listed for the target is only rewrapped: no quality
/// loss, HDR and 10-bit kept, seconds instead of minutes. iPhone clips (HEVC or
/// H.264 in MOV) become MP4 this way. MP4 and MOV keep to what Apple devices and
/// browsers play, WebM to what its specification allows, AVI to what the old
/// DivX/Xvid players read. Matroska takes nearly anything.
fn remuxable_video(to: &str) -> &'static [&'static str] {
    match to {
        "mp4" | "m4v" => &["h264", "hevc", "av1", "mpeg4"],
        // No AV1 here: ffmpeg's mov muxer refuses it ("only supported in MP4 and AVIF").
        "mov" => &["h264", "hevc", "prores", "mpeg4"],
        "mkv" => &[
            "h264",
            "hevc",
            "av1",
            "vp8",
            "vp9",
            "mpeg4",
            "mpeg2video",
            "prores",
            "theora",
        ],
        "webm" => &["vp8", "vp9", "av1"],
        "avi" => &["mpeg4", "msmpeg4v3"],
        "ts" | "m2ts" => &["h264", "hevc"],
        _ => &[],
    }
}

/// Audio codecs each container takes as they are (Filee's table, plus the
/// handful of extras Matroska and AVI are known to swallow).
fn remuxable_audio(to: &str) -> &'static [&'static str] {
    match to {
        "mp4" | "m4v" => &["aac", "alac", "ac3", "eac3", "mp3"],
        "mov" => &[
            "aac",
            "alac",
            "ac3",
            "eac3",
            "mp3",
            "pcm_s16le",
            "pcm_s24le",
        ],
        "mkv" => &[
            "aac",
            "alac",
            "ac3",
            "eac3",
            "mp3",
            "mp2",
            "opus",
            "vorbis",
            "flac",
            "dts",
            "truehd",
            "pcm_s16le",
            "pcm_s24le",
        ],
        "webm" => &["opus", "vorbis"],
        "avi" => &["mp3", "mp2", "ac3", "pcm_s16le"],
        "ts" => &["aac", "ac3", "eac3", "mp3"],
        "m2ts" => &["aac", "ac3", "eac3"],
        _ => &[],
    }
}

/// Pixel formats players decode in hardware: 4:2:0 at 8 or 10 bits (phones
/// record HDR as 10-bit, and we would like to keep it that way).
const PLAYABLE_PIX_FMTS: [&str; 3] = ["yuv420p", "yuvj420p", "yuv420p10le"];

/// True when the video stream is rewrapped as is instead of re-encoded.
///
/// The target container must take the codec, the pixel format must be one
/// players decode (ProRes is exempt, it lives in editing suites, not phones),
/// and the quality must not be `Small`: Small always re-encodes, while High
/// and Normal only say how to encode when we have to.
pub fn can_copy_video(to: &str, video: &VideoSource<'_>, quality: QualityLevel) -> bool {
    let Some(codec) = video.codec else {
        return false;
    };
    quality != QualityLevel::Small
        && remuxable_video(to).contains(&codec)
        && (codec == "prores"
            || video
                .pix_fmt
                .is_some_and(|f| PLAYABLE_PIX_FMTS.contains(&f)))
}

/// Audio settings the user asked for that a stream copy cannot honour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioOverrides {
    /// Requested bitrate in kbit/s; 0 keeps the default.
    pub bitrate_kbps: u32,
    /// Requested channel count: 1 is mono, 2 stereo, anything else keeps the source.
    pub channels: u32,
}

impl From<&morphring_core::MediaOptions> for AudioOverrides {
    fn from(media: &morphring_core::MediaOptions) -> Self {
        Self {
            bitrate_kbps: media.audio_bitrate_kbps,
            channels: media.audio_channels,
        }
    }
}

impl AudioOverrides {
    /// The channel count the user forced, if any (only mono and stereo count).
    fn forced_channels(self) -> Option<u32> {
        matches!(self.channels, 1 | 2).then_some(self.channels)
    }
}

/// True when the audio stream is copied as is: the quality is not `Small`, no
/// bitrate is forced, the channel count stays what it is, and the target
/// container takes the codec.
pub fn can_copy_audio(
    to: &str,
    audio: &AudioSource<'_>,
    quality: QualityLevel,
    overrides: AudioOverrides,
) -> bool {
    quality != QualityLevel::Small
        && overrides.bitrate_kbps == 0
        && (!matches!(overrides.channels, 1 | 2) || overrides.channels == audio.channels)
        && audio
            .codec
            .is_some_and(|c| remuxable_audio(to).contains(&c))
}

/// FourCCs AVI players recognise for MPEG-4 part 2 video.
const AVI_MPEG4_TAGS: [&str; 4] = ["XVID", "DIVX", "DX50", "FMP4"];

/// The `-tag:v` a copied video stream needs to be playable where it lands.
///
/// Apple only plays HEVC in MP4/MOV under the `hvc1` tag, and ffmpeg keeps
/// `hev1` from some sources, so HEVC is always retagged. MPEG-4 part 2 out of
/// an MP4 arrives as `mp4v`, which AVI players have never heard of; it becomes
/// `XVID` unless it already wears a FourCC they know.
fn copy_tag(to: &str, video: &VideoSource<'_>) -> &'static [&'static str] {
    match (video.codec, to) {
        (Some("hevc"), "mp4" | "m4v" | "mov") => &["-tag:v", "hvc1"],
        (Some("mpeg4"), "avi")
            if !video
                .codec_tag
                .is_some_and(|t| AVI_MPEG4_TAGS.iter().any(|k| k.eq_ignore_ascii_case(t))) =>
        {
            &["-tag:v", "XVID"]
        }
        _ => &[],
    }
}

/// Smallest height a downscale goes to: below this even a thumbnail gives up.
const MIN_HEIGHT: u32 = 16;

/// True when `max_height` asks for a smaller picture than the source has, so
/// the video has to be re-encoded even if it could otherwise be copied.
pub fn needs_downscale(video: &VideoSource<'_>, max_height: u32) -> bool {
    let shown = if video.display_height > 0 {
        video.display_height
    } else {
        video.height
    };
    max_height > 0 && shown > max_height.max(MIN_HEIGHT)
}

/// Scales the picture down to at most `max_height` lines (never up), keeping
/// the aspect ratio. `-2` lets ffmpeg pick an even width, and the height is
/// truncated to even too, for the same 4:2:0 reasons as [`scale_filter`].
pub fn downscale_filter(max_height: u32) -> String {
    let h = max_height.max(MIN_HEIGHT);
    format!("scale=w=-2:h='trunc(min(ih,{h})/2)*2'")
}

/// Makes both sides even, which 4:2:0 encoders insist on (an odd-sized phone
/// clip or GIF would otherwise fail).
///
/// The expression works on the decoded, already rotated frame, so it is right
/// for portrait phone videos too. The single quotes are for ffmpeg's own
/// expression parser: we never go through a shell.
pub fn scale_filter() -> &'static str {
    "scale=w='trunc(iw/2)*2':h='trunc(ih/2)*2'"
}

fn video_codec_args(codec: &str, quality: QualityLevel, video: &VideoSource<'_>) -> Vec<String> {
    let s = |v: &str| v.to_owned();
    match codec {
        // CRF 18 is visually lossless, 23 is x264's default, 28 clearly smaller.
        // yuv420p is the only pixel format every H.264 decoder plays.
        "libx264" => vec![
            s("-c:v"),
            s("libx264"),
            s("-preset"),
            s("medium"),
            s("-crf"),
            choose(quality, 18, 23, 28).to_string(),
            s("-pix_fmt"),
            s("yuv420p"),
        ],
        // Constant quality (-b:v 0 + CRF). Row-mt and cpu-used 4 keep VP9 from
        // encoding at geological speed; slower settings barely shrink the file.
        "libvpx-vp9" => vec![
            s("-c:v"),
            s("libvpx-vp9"),
            s("-b:v"),
            s("0"),
            s("-crf"),
            choose(quality, 24, 32, 38).to_string(),
            s("-row-mt"),
            s("1"),
            s("-deadline"),
            s("good"),
            s("-cpu-used"),
            s("4"),
            s("-pix_fmt"),
            s("yuv420p"),
        ],
        // GOP 250 because mpeg4's default of 12 bloats files; a fixed frame rate
        // because MPEG-4 part 2 can't store the odd time bases phones produce.
        _ => {
            let mut args = vec![
                s("-c:v"),
                s("mpeg4"),
                s("-tag:v"),
                s("XVID"),
                s("-q:v"),
                choose(quality, 2, 4, 7).to_string(),
                s("-g"),
                s("250"),
                s("-bf"),
                s("2"),
                s("-pix_fmt"),
                s("yuv420p"),
            ];
            if let Some(rate) = video.fps.and_then(nearest_rate) {
                args.extend([s("-r"), rate]);
            }
            args
        }
    }
}

/// Frame rates MPEG can signal exactly (num, den).
const BROADCAST_RATES: [(u32, u32); 8] = [
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
];

/// The source rate as a simple fraction: exact film/NTSC/PAL rates are kept,
/// anything else (phones report averages like 29.87) rounds to whole frames.
pub fn nearest_rate(fps: f64) -> Option<String> {
    if fps.is_nan() || fps <= 0.0 {
        return None;
    }
    for (num, den) in BROADCAST_RATES {
        if (fps - f64::from(num) / f64::from(den)).abs() < 0.01 {
            return Some(if den == 1 {
                num.to_string()
            } else {
                format!("{num}/{den}")
            });
        }
    }
    // Clamped to 1..=120 first, so the cast can't overflow or go negative.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = fps.round().clamp(1.0, 120.0) as u32;
    Some(whole.to_string())
}

fn container_args(to: &str) -> &'static [&'static str] {
    match to {
        // Index at the front, so playback can start before the file has loaded.
        "mp4" | "mov" | "m4a" => &["-movflags", "+faststart"],
        // ID3v2.3 is what Windows Explorer and most car stereos read; ffmpeg defaults to 2.4.
        "mp3" => &["-id3v2_version", "3"],
        // WAV tops out at 4 GB; long recordings switch to RF64 on their own.
        "wav" => &["-rf64", "auto"],
        _ => &[],
    }
}

/// Keep title, artist, dates and friends, but drop the tags that describe the
/// source container rather than the content (an empty value removes a tag).
fn metadata_args() -> [&'static str; 8] {
    [
        "-map_metadata",
        "0",
        "-metadata",
        "major_brand=",
        "-metadata",
        "minor_version=",
        "-metadata",
        "compatible_brands=",
    ]
}

// ───────────────────────── Audio ─────────────────────────

/// Arguments for an audio target (mp3, m4a, wav, flac, ogg, opus), from an
/// audio file or the soundtrack of a video.
///
/// # Errors
///
/// Fails when the source has no audio stream or `to` is not an audio format.
pub fn audio_args(
    input: &Path,
    output: &Path,
    to: &str,
    options: &ConvertOptions,
    source: &Source<'_>,
) -> Result<Vec<OsString>, EngineError> {
    let quality = quality_of(options);
    let media = &options.media;
    let audio = source
        .audio
        .as_ref()
        .ok_or_else(|| EngineError::Failed("The file has no audio track.".into()))?;
    let spec = target_spec(to)
        .filter(|s| s.video.is_none())
        .ok_or_else(|| EngineError::Failed(format!("'{to}' is not an audio format.")))?;

    let mut a = Args::new();
    a.push("-i").push(input).extend(["-map", "0:a:0"]);
    let codec = if to == "wav" {
        wav_codec(media.wav_format)
    } else {
        spec.audio
    };
    let overrides = AudioOverrides::from(media);
    a.extend(audio_codec_args(codec, quality, audio, overrides, false));
    a.extend(channel_args(overrides));
    // Resampling is a WAV-only knob: the lossy encoders pick their own rates.
    if to == "wav" && (8000..=192_000).contains(&media.audio_sample_rate) {
        a.push("-ar").push(media.audio_sample_rate.to_string());
    }
    a.extend(metadata_args());
    a.extend(container_args(to));
    a.extend(["-f", spec.muxer, "-progress", "pipe:1"])
        .push(output);
    Ok(a.finish())
}

/// The PCM codec for each WAV sample format.
pub fn wav_codec(format: WavFormat) -> &'static str {
    match format {
        WavFormat::Pcm16 => "pcm_s16le",
        // 8-bit WAV is unsigned, because 1991 said so.
        WavFormat::Pcm8 => "pcm_u8",
        WavFormat::Pcm24 => "pcm_s24le",
        WavFormat::Pcm32 => "pcm_s32le",
        WavFormat::Float32 => "pcm_f32le",
    }
}

/// `-ac N` when the user forced mono or stereo, nothing otherwise.
fn channel_args(overrides: AudioOverrides) -> Vec<String> {
    overrides
        .forced_channels()
        .map(|n| vec!["-ac".to_owned(), n.to_string()])
        .unwrap_or_default()
}

/// Encoder options for one audio codec. `in_video` switches MP3 to CBR,
/// because VBR MP3 confuses AVI players' audio sync. A requested bitrate
/// replaces the quality's default; 0 means "you pick".
fn audio_codec_args(
    codec: &str,
    quality: QualityLevel,
    audio: &AudioSource<'_>,
    overrides: AudioOverrides,
    in_video: bool,
) -> Vec<String> {
    // Bitrates scale with the channels that come out, not the ones that went in:
    // a 5.1 film squeezed to stereo does not need 576 k.
    let channels = overrides
        .forced_channels()
        .unwrap_or(if audio.channels > 0 {
            audio.channels
        } else {
            2
        });
    let requested = (overrides.bitrate_kbps > 0).then_some(overrides.bitrate_kbps);
    let mut args = vec!["-c:a".to_owned(), codec.to_owned()];
    match codec {
        // Per stereo pair: 256 k is transparent, 192 excellent, 128 fine for speech.
        "aac" => args.extend(bitrate(
            requested.unwrap_or_else(|| scaled(choose(quality, 256, 192, 128), channels)),
            16,
            256 * channels,
        )),
        // Opus is transparent around 128 k for stereo; 64 is still decent for music.
        "libopus" => args.extend(bitrate(
            requested.unwrap_or_else(|| scaled(choose(quality, 160, 128, 64), channels)),
            6,
            256 * channels,
        )),
        // CBR inside video or when a bitrate was asked for, snapped to a
        // bitrate the MP3 table actually has.
        "libmp3lame" if in_video || requested.is_some() => args.extend(bitrate(
            snap(
                requested.unwrap_or_else(|| choose(quality, 256, 192, 128)),
                &MP3_BITRATES,
            ),
            32,
            320,
        )),
        // LAME's VBR presets: V0 ~ 245, V2 ~ 190, V5 ~ 130 kbit/s.
        "libmp3lame" => args.extend(["-q:a".to_owned(), choose(quality, 0, 2, 5).to_string()]),
        "libvorbis" => match requested {
            Some(kbps) => args.extend(bitrate(kbps, 32, 500)),
            // Vorbis quality 6 ~ 192, 4 ~ 128, 2 ~ 96 kbit/s.
            None => args.extend(["-q:a".to_owned(), choose(quality, 6, 4, 2).to_string()]),
        },
        // flac and PCM are lossless: nothing to tune, nothing to break.
        _ => {}
    }
    args
}

/// A stereo bitrate scaled to the channel count (mono gets half, 5.1 three times).
fn scaled(stereo_kbps: u32, channels: u32) -> u32 {
    stereo_kbps * channels.clamp(1, 8) / 2
}

fn bitrate(kbps: u32, min: u32, max: u32) -> [String; 2] {
    [
        "-b:a".to_owned(),
        format!("{}k", kbps.clamp(min, max.max(min))),
    ]
}

const MP3_BITRATES: [u32; 14] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];

/// The largest allowed bitrate not above `kbps` (the smallest one if all are above).
fn snap(kbps: u32, allowed: &[u32]) -> u32 {
    allowed
        .iter()
        .copied()
        .filter(|&a| a <= kbps)
        .max()
        .or_else(|| allowed.first().copied())
        .unwrap_or(kbps)
}

// ───────────────────────── Animated GIF ─────────────────────────

/// The two passes of an animated GIF: pass 1 picks the 256 colours that suit
/// the whole clip (palettegen), pass 2 maps every frame onto them (paletteuse).
/// One pass with a fixed palette looks like a 1996 banner ad.
///
/// # Errors
///
/// Fails when the source has no video stream.
pub fn gif_args(
    input: &Path,
    output: &Path,
    quality: QualityLevel,
    source: &Source<'_>,
    palette: &Path,
) -> Result<[Vec<OsString>; 2], EngineError> {
    if source.video.is_none() {
        return Err(no_video());
    }
    let fps = choose(quality, 15, 12, 10);
    let max_width = choose(quality, 640, 480, 320);
    let chain = format!(
        "fps={fps},scale=w='min(iw,{max_width})':h=ih:force_original_aspect_ratio=decrease:flags=lanczos"
    );
    let colors = choose(quality, 256, 256, 128);
    // Error diffusion looks best; ordered (Bayer) dithering compresses far better
    // because it doesn't shimmer from frame to frame.
    let dither = choose(
        quality,
        "sierra2_4a",
        "bayer:bayer_scale=4",
        "bayer:bayer_scale=5",
    );

    // Pass 1 only writes its palette at the very end, so a second copy of the
    // frames goes to a null output purely so -progress has a clock to report.
    let mut first = Args::new();
    first.push("-i").push(input);
    first.push("-filter_complex").push(format!(
        "[0:V:0]{chain},split[a][b];[a]palettegen=max_colors={colors}[palette]"
    ));
    first
        .extend(["-map", "[palette]", "-frames:v", "1", "-update", "1"])
        .extend(["-f", "image2"])
        .push(palette);
    first
        .extend(["-map", "[b]", "-progress", "pipe:1"])
        .extend(["-f", "null", "-"]);

    let mut second = Args::new();
    second.push("-i").push(input).push("-i").push(palette);
    second.push("-filter_complex").push(format!(
        "[0:V:0]{chain}[frames];[frames][1:v]paletteuse=dither={dither}:diff_mode=rectangle[gif]"
    ));
    second
        .extend(["-map", "[gif]", "-loop", "0", "-f", "gif"])
        .extend(["-progress", "pipe:1"])
        .push(output);

    Ok([first.finish(), second.finish()])
}

// ───────────────────────── Poster frame ─────────────────────────

/// One full-size PNG frame from a tenth of the way in (the first frames are
/// often black). This is how video reaches every image format.
///
/// # Errors
///
/// Fails when the source has no video stream.
pub fn poster_args(
    input: &Path,
    output: &Path,
    source: &Source<'_>,
) -> Result<Vec<OsString>, EngineError> {
    if source.video.is_none() {
        return Err(no_video());
    }
    let seconds = source.duration.unwrap_or(0.0) / 10.0;
    let mut a = Args::new();
    // -ss before -i seeks by keyframe index instead of decoding the first tenth.
    a.push("-ss").push(format_seconds(seconds));
    a.push("-i")
        .push(input)
        .extend(["-map", "0:V:0", "-vf", "null"]);
    a.extend([
        "-frames:v",
        "1",
        "-update",
        "1",
        "-c:v",
        "png",
        "-f",
        "image2",
    ])
    .extend(["-progress", "pipe:1"])
    .push(output);
    Ok(a.finish())
}

/// Seconds with a '.' and at most 3 decimals, trailing zeros dropped (Filee's
/// `"0.###"` in the invariant culture).
pub fn format_seconds(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "0".to_owned();
    }
    let text = format!("{seconds:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() { "0" } else { text }.to_owned()
}

// ───────────────────────── Helpers ─────────────────────────

fn choose<T>(quality: QualityLevel, high: T, normal: T, small: T) -> T {
    match quality {
        QualityLevel::High => high,
        QualityLevel::Normal => normal,
        QualityLevel::Small => small,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morphring_core::FormatId;

    fn strs(args: &[OsString]) -> Vec<&str> {
        args.iter().map(|a| a.to_str().unwrap()).collect()
    }

    fn has_pair(args: &[OsString], key: &str, value: &str) -> bool {
        strs(args).windows(2).any(|w| w[0] == key && w[1] == value)
    }

    fn h264_mp4() -> Source<'static> {
        Source {
            video: Some(VideoSource {
                codec: Some("h264"),
                pix_fmt: Some("yuv420p"),
                codec_tag: None,
                width: 1920,
                height: 1080,
                display_height: 1080,
                fps: Some(30.0),
            }),
            audio: Some(AudioSource {
                codec: Some("aac"),
                codec_tag: None,
                channels: 2,
            }),
            duration: Some(12.5),
        }
    }

    fn wav() -> Source<'static> {
        Source {
            video: None,
            audio: Some(AudioSource {
                codec: Some("pcm_s16le"),
                codec_tag: None,
                channels: 2,
            }),
            duration: Some(3.0),
        }
    }

    /// Default options at one quality, set the way a preset sets it.
    fn opts(quality: QualityLevel) -> ConvertOptions {
        let mut options = ConvertOptions::default();
        options.media.quality = match quality {
            QualityLevel::High => MediaQuality::High,
            QualityLevel::Normal => MediaQuality::Balanced,
            QualityLevel::Small => MediaQuality::Small,
        };
        options
    }

    fn video(to: &str, quality: QualityLevel, source: &Source<'_>) -> Vec<OsString> {
        video_args(
            Path::new("in.mp4"),
            Path::new("out"),
            to,
            &opts(quality),
            source,
        )
        .unwrap()
    }

    fn edge(from: &str, to: &str) -> Option<u32> {
        EDGES
            .iter()
            .find(|e| e.from.as_str() == from && e.to.as_str() == to)
            .map(|e| e.cost)
    }

    #[test]
    fn wav_to_mp3_normal_pins_the_full_arg_list() {
        // The release smoke test runs exactly this on Ubuntu 22.04's ffmpeg 4.4,
        // so every flag here has to exist there.
        let args = audio_args(
            Path::new("in.wav"),
            Path::new("out.mp3"),
            "mp3",
            &opts(QualityLevel::Normal),
            &wav(),
        )
        .unwrap();
        assert_eq!(
            strs(&args),
            [
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-v",
                "error",
                "-y",
                "-i",
                "in.wav",
                "-map",
                "0:a:0",
                "-c:a",
                "libmp3lame",
                "-q:a",
                "2",
                "-map_metadata",
                "0",
                "-metadata",
                "major_brand=",
                "-metadata",
                "minor_version=",
                "-metadata",
                "compatible_brands=",
                "-id3v2_version",
                "3",
                "-f",
                "mp3",
                "-progress",
                "pipe:1",
                "out.mp3",
            ]
        );
    }

    #[test]
    fn h264_into_mkv_at_high_is_a_stream_copy() {
        let args = video("mkv", QualityLevel::High, &h264_mp4());
        assert!(has_pair(&args, "-c:v", "copy"));
        assert!(has_pair(&args, "-c:a", "copy"));
        assert!(has_pair(&args, "-f", "matroska"));
        assert!(!strs(&args).contains(&"-vf"));
        assert!(!strs(&args).contains(&"libx264"));
    }

    #[test]
    fn h264_into_mkv_at_small_re_encodes() {
        let args = video("mkv", QualityLevel::Small, &h264_mp4());
        assert!(!strs(&args).contains(&"copy"));
        assert!(has_pair(&args, "-c:v", "libx264"));
        assert!(has_pair(&args, "-crf", "28"));
        assert!(has_pair(&args, "-c:a", "aac"));
        assert!(has_pair(&args, "-b:a", "128k"));
    }

    fn clip(
        codec: &'static str,
        pix_fmt: &'static str,
        tag: Option<&'static str>,
    ) -> Source<'static> {
        let mut source = h264_mp4();
        source.video = Some(VideoSource {
            codec: Some(codec),
            pix_fmt: Some(pix_fmt),
            codec_tag: tag,
            width: 1920,
            height: 1080,
            display_height: 1080,
            fps: Some(30.0),
        });
        source
    }

    /// Everything between the `-map`s and the metadata flags: the codec choices.
    fn codec_part(args: &[OsString]) -> Vec<&str> {
        let all = strs(args);
        let start = all.iter().rposition(|a| a.starts_with("0:")).unwrap() + 1;
        let end = all.iter().position(|a| *a == "-map_metadata").unwrap();
        all[start..end].to_vec()
    }

    #[test]
    fn iphone_hevc_mov_to_mp4_is_a_copy_tagged_hvc1() {
        let mut source = clip("hevc", "yuv420p10le", Some("hev1"));
        if let Some(a) = source.audio.as_mut() {
            a.codec_tag = Some("mp4a");
        }
        let args = video_args(
            Path::new("IMG_0042.MOV"),
            Path::new("out.mp4"),
            "mp4",
            &opts(QualityLevel::Normal),
            &source,
        )
        .unwrap();
        assert_eq!(
            strs(&args),
            [
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-v",
                "error",
                "-y",
                "-i",
                "IMG_0042.MOV",
                "-map",
                "0:V:0",
                "-map",
                "0:a:0",
                "-c:v",
                "copy",
                "-tag:v",
                "hvc1",
                "-c:a",
                "copy",
                "-map_metadata",
                "0",
                "-metadata",
                "major_brand=",
                "-metadata",
                "minor_version=",
                "-metadata",
                "compatible_brands=",
                "-movflags",
                "+faststart",
                "-max_muxing_queue_size",
                "4096",
                "-f",
                "mp4",
                "-progress",
                "pipe:1",
                "out.mp4",
            ]
        );
    }

    #[test]
    fn hevc_gets_hvc1_in_apple_containers_only() {
        let source = clip("hevc", "yuv420p", Some("hvc1"));
        for to in ["mp4", "mov"] {
            assert_eq!(
                codec_part(&video(to, QualityLevel::High, &source)),
                ["-c:v", "copy", "-tag:v", "hvc1", "-c:a", "copy"],
                "{to}"
            );
        }
        assert_eq!(
            codec_part(&video("mkv", QualityLevel::High, &source)),
            ["-c:v", "copy", "-c:a", "copy"]
        );
    }

    #[test]
    fn h264_yuv420p_mkv_to_mp4_is_a_copy() {
        let source = clip("h264", "yuv420p", None);
        assert_eq!(
            codec_part(&video("mp4", QualityLevel::Normal, &source)),
            ["-c:v", "copy", "-c:a", "copy"]
        );
    }

    #[test]
    fn vp9_webm_to_mp4_re_encodes() {
        let mut source = clip("vp9", "yuv420p", None);
        source.audio = Some(AudioSource {
            codec: Some("opus"),
            codec_tag: None,
            channels: 2,
        });
        assert_eq!(
            codec_part(&video("mp4", QualityLevel::Normal, &source)),
            [
                "-vf",
                scale_filter(),
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "23",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ]
        );
    }

    #[test]
    fn aac_is_copied_into_mkv_even_when_the_video_is_re_encoded() {
        // Motion JPEG in 4:2:2 is not something to keep, but its AAC soundtrack is.
        let source = clip("mjpeg", "yuvj422p", None);
        let args = video("mkv", QualityLevel::Normal, &source);
        let part = codec_part(&args);
        assert_eq!(part[..4], ["-vf", scale_filter(), "-c:v", "libx264"]);
        assert_eq!(part[part.len() - 2..], ["-c:a", "copy"]);
    }

    #[test]
    fn fit_matrix_per_container() {
        let copies = |codec, fmt, to| {
            can_copy_video(
                to,
                &clip(codec, fmt, None).video.unwrap(),
                QualityLevel::High,
            )
        };
        for to in ["mp4", "m4v"] {
            for codec in ["h264", "hevc", "av1", "mpeg4"] {
                assert!(copies(codec, "yuv420p", to), "{codec} -> {to}");
            }
            assert!(!copies("vp9", "yuv420p", to));
            assert!(!copies("prores", "yuv422p10le", to));
        }
        assert!(copies("prores", "yuv422p10le", "mov"));
        assert!(copies("mpeg4", "yuv420p", "mov"));
        // ffmpeg's mov muxer refuses AV1 outright.
        assert!(!copies("av1", "yuv420p", "mov"));
        for codec in ["h264", "hevc", "av1", "vp8", "vp9", "mpeg2video", "theora"] {
            assert!(copies(codec, "yuv420p", "mkv"), "{codec} -> mkv");
        }
        assert!(copies("prores", "yuv444p12le", "mkv"));
        for codec in ["vp8", "vp9", "av1"] {
            assert!(copies(codec, "yuv420p", "webm"));
        }
        assert!(!copies("h264", "yuv420p", "webm"));
        assert!(copies("mpeg4", "yuv420p", "avi"));
        assert!(!copies("h264", "yuv420p", "avi"));
        assert!(copies("h264", "yuv420p", "ts"));
        assert!(!copies("h264", "yuv420p", "gif"));
    }

    #[test]
    fn pix_fmt_allow_list() {
        let copies = |fmt| {
            can_copy_video(
                "mp4",
                &clip("hevc", fmt, None).video.unwrap(),
                QualityLevel::Normal,
            )
        };
        assert!(copies("yuv420p"));
        assert!(copies("yuvj420p"));
        assert!(copies("yuv420p10le"));
        assert!(!copies("yuv422p"));
        assert!(!copies("yuv444p"));
        let mut unknown = clip("h264", "yuv420p", None);
        if let Some(v) = unknown.video.as_mut() {
            v.pix_fmt = None;
        }
        assert!(!can_copy_video(
            "mkv",
            unknown.video.as_ref().unwrap(),
            QualityLevel::High
        ));
        assert!(!can_copy_video(
            "mp4",
            &clip("hevc", "yuv420p", None).video.unwrap(),
            QualityLevel::Small
        ));
    }

    #[test]
    fn audio_copy_rules() {
        let aac = AudioSource {
            codec: Some("aac"),
            codec_tag: None,
            channels: 2,
        };
        let none = AudioOverrides::default();
        assert!(can_copy_audio("mkv", &aac, QualityLevel::Normal, none));
        assert!(can_copy_audio("mp4", &aac, QualityLevel::High, none));
        assert!(!can_copy_audio("webm", &aac, QualityLevel::High, none));
        assert!(!can_copy_audio("mkv", &aac, QualityLevel::Small, none));
        let bitrate = AudioOverrides {
            bitrate_kbps: 128,
            channels: 0,
        };
        assert!(!can_copy_audio("mkv", &aac, QualityLevel::High, bitrate));
        let mono = AudioOverrides {
            bitrate_kbps: 0,
            channels: 1,
        };
        let stereo = AudioOverrides {
            channels: 2,
            ..mono
        };
        assert!(!can_copy_audio("mkv", &aac, QualityLevel::High, mono));
        assert!(can_copy_audio("mkv", &aac, QualityLevel::High, stereo));
        let opus = AudioSource {
            codec: Some("opus"),
            ..aac
        };
        assert!(can_copy_audio("webm", &opus, QualityLevel::High, none));
        assert!(!can_copy_audio("mp4", &opus, QualityLevel::High, none));
        let unknown = AudioSource { codec: None, ..aac };
        assert!(!can_copy_audio("mkv", &unknown, QualityLevel::High, none));
    }

    #[test]
    fn mpeg4_into_avi_is_retagged_xvid_unless_it_already_is() {
        let from_mp4 = clip("mpeg4", "yuv420p", Some("mp4v"));
        let args = video("avi", QualityLevel::High, &from_mp4);
        assert!(has_pair(&args, "-c:v", "copy"));
        assert!(has_pair(&args, "-tag:v", "XVID"));
        let divx = clip("mpeg4", "yuv420p", Some("DIVX"));
        let args = video("avi", QualityLevel::High, &divx);
        assert!(has_pair(&args, "-c:v", "copy"));
        assert!(!strs(&args).contains(&"-tag:v"));
    }

    #[test]
    fn four_two_two_h264_is_not_copied() {
        let mut source = h264_mp4();
        if let Some(v) = source.video.as_mut() {
            v.pix_fmt = Some("yuv422p");
        }
        let args = video("mov", QualityLevel::Normal, &source);
        assert!(has_pair(&args, "-c:v", "libx264"));
        // The video and the audio decide on their own: the AAC still rides along untouched.
        assert!(has_pair(&args, "-c:a", "copy"));
    }

    #[test]
    fn copied_video_still_re_encodes_audio_the_container_rejects() {
        let mut source = h264_mp4();
        source.audio = Some(AudioSource {
            codec: Some("opus"),
            codec_tag: None,
            channels: 2,
        });
        let args = video("mp4", QualityLevel::Normal, &source);
        assert!(has_pair(&args, "-c:v", "copy"));
        assert!(has_pair(&args, "-c:a", "aac"));
        assert!(has_pair(&args, "-b:a", "192k"));
    }

    #[test]
    fn mp4_to_webm_small_uses_vp9_crf_38() {
        let args = video("webm", QualityLevel::Small, &h264_mp4());
        assert_eq!(
            strs(&args)[strs(&args).iter().position(|a| *a == "-c:v").unwrap()..][..14],
            [
                "-c:v",
                "libvpx-vp9",
                "-b:v",
                "0",
                "-crf",
                "38",
                "-row-mt",
                "1",
                "-deadline",
                "good",
                "-cpu-used",
                "4",
                "-pix_fmt",
                "yuv420p",
            ]
        );
        assert!(has_pair(&args, "-c:a", "libopus"));
        assert!(has_pair(&args, "-b:a", "64k"));
        assert!(has_pair(&args, "-f", "webm"));
    }

    #[test]
    fn full_re_encode_list_for_mp4_to_mov_without_audio() {
        let mut source = h264_mp4();
        source.audio = None;
        let args = video_args(
            Path::new("in.mp4"),
            Path::new("out.mov"),
            "mov",
            &opts(QualityLevel::Small),
            &source,
        )
        .unwrap();
        assert_eq!(
            strs(&args),
            [
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-v",
                "error",
                "-y",
                "-i",
                "in.mp4",
                "-map",
                "0:V:0",
                "-vf",
                "scale=w='trunc(iw/2)*2':h='trunc(ih/2)*2'",
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "28",
                "-pix_fmt",
                "yuv420p",
                "-an",
                "-map_metadata",
                "0",
                "-metadata",
                "major_brand=",
                "-metadata",
                "minor_version=",
                "-metadata",
                "compatible_brands=",
                "-movflags",
                "+faststart",
                "-max_muxing_queue_size",
                "4096",
                "-f",
                "mov",
                "-progress",
                "pipe:1",
                "out.mov",
            ]
        );
    }

    #[test]
    fn avi_uses_xvid_and_cbr_mp3() {
        let mut source = h264_mp4();
        if let Some(v) = source.video.as_mut() {
            v.fps = Some(29.97);
        }
        let args = video("avi", QualityLevel::Normal, &source);
        assert!(has_pair(&args, "-c:v", "mpeg4"));
        assert!(has_pair(&args, "-tag:v", "XVID"));
        assert!(has_pair(&args, "-q:v", "4"));
        assert!(has_pair(&args, "-r", "30000/1001"));
        assert!(has_pair(&args, "-c:a", "libmp3lame"));
        assert!(has_pair(&args, "-b:a", "192k"));
        assert!(!strs(&args).contains(&"-q:a"));
        assert!(has_pair(&args, "-f", "avi"));
    }

    #[test]
    fn scale_filter_rounds_odd_sizes_to_even() {
        // A 641x361 frame must come out 640x360: trunc(n/2)*2 on both sides.
        // Evaluate the same arithmetic the filter asks ffmpeg for.
        let filter = scale_filter();
        assert_eq!(filter, "scale=w='trunc(iw/2)*2':h='trunc(ih/2)*2'");
        let even = |n: u32| (f64::from(n) / 2.0).trunc() as u32 * 2;
        assert_eq!((even(641), even(361)), (640, 360));
        assert_eq!((even(1920), even(1080)), (1920, 1080));
        // And the filter is what every re-encode actually uses.
        let mut source = h264_mp4();
        if let Some(v) = source.video.as_mut() {
            v.width = 641;
            v.height = 361;
            v.codec = Some("vp9");
        }
        assert!(has_pair(
            &video("mp4", QualityLevel::Normal, &source),
            "-vf",
            filter
        ));
    }

    #[test]
    fn gif_passes_use_palettegen_then_paletteuse() {
        let [first, second] = gif_args(
            Path::new("in.mp4"),
            Path::new("out.gif"),
            QualityLevel::Normal,
            &h264_mp4(),
            Path::new("/work/palette.png"),
        )
        .unwrap();
        let first = strs(&first).join(" ");
        let second = strs(&second).join(" ");
        assert!(first.contains("palettegen=max_colors=256"));
        assert!(first.contains("fps=12,"));
        assert!(first.contains("min(iw,480)"));
        assert!(first.contains("/work/palette.png"));
        assert!(first.contains("-progress pipe:1"));
        assert!(second.contains("paletteuse=dither=bayer:bayer_scale=4"));
        assert!(second.contains("fps=12,"));
        assert!(second.contains("-i /work/palette.png"));
        assert!(second.ends_with("-f gif -progress pipe:1 out.gif"));
    }

    #[test]
    fn gif_settings_follow_quality() {
        let gif = |q| {
            let passes = plan(
                Path::new("in.mp4"),
                Path::new("out.gif"),
                "gif",
                &opts(q),
                &h264_mp4(),
                Path::new("p.png"),
            )
            .unwrap();
            assert_eq!(passes.len(), 2);
            assert!((passes[0].weight + passes[1].weight - 1.0).abs() < 1e-6);
            strs(&passes[1].args).join(" ")
        };
        let high = gif(QualityLevel::High);
        assert!(high.contains("fps=15,") && high.contains("min(iw,640)"));
        assert!(high.contains("dither=sierra2_4a"));
        let small = gif(QualityLevel::Small);
        assert!(small.contains("fps=10,") && small.contains("min(iw,320)"));
    }

    #[test]
    fn poster_frame_seeks_a_tenth_in() {
        let args = poster_args(Path::new("in.mp4"), Path::new("out.png"), &h264_mp4()).unwrap();
        assert!(has_pair(&args, "-ss", "1.25"));
        assert!(has_pair(&args, "-frames:v", "1"));
        assert!(has_pair(&args, "-c:v", "png"));
        assert!(has_pair(&args, "-f", "image2"));
        assert_eq!(strs(&args).last(), Some(&"out.png"));
    }

    #[test]
    fn seconds_use_a_dot_and_at_most_three_decimals() {
        assert_eq!(format_seconds(1.25), "1.25");
        assert_eq!(format_seconds(0.123_456), "0.123");
        assert_eq!(format_seconds(2.0), "2");
        assert_eq!(format_seconds(0.0004), "0");
        assert_eq!(format_seconds(f64::NAN), "0");
        assert_eq!(format_seconds(-1.0), "0");
    }

    #[test]
    fn audio_codecs_follow_quality_and_channels() {
        let run = |to: &str, q, channels| {
            let source = Source {
                audio: Some(AudioSource {
                    codec: Some("flac"),
                    codec_tag: None,
                    channels,
                }),
                ..Source::default()
            };
            audio_args(Path::new("a"), Path::new("b"), to, &opts(q), &source).unwrap()
        };
        assert!(has_pair(&run("mp3", QualityLevel::High, 2), "-q:a", "0"));
        assert!(has_pair(&run("mp3", QualityLevel::Small, 2), "-q:a", "5"));
        let m4a = run("m4a", QualityLevel::High, 2);
        assert!(has_pair(&m4a, "-b:a", "256k"));
        assert!(has_pair(&m4a, "-movflags", "+faststart"));
        assert!(has_pair(&m4a, "-f", "ipod"));
        assert!(has_pair(
            &run("m4a", QualityLevel::Normal, 1),
            "-b:a",
            "96k"
        ));
        assert!(has_pair(
            &run("m4a", QualityLevel::Normal, 6),
            "-b:a",
            "576k"
        ));
        assert!(has_pair(
            &run("opus", QualityLevel::High, 2),
            "-b:a",
            "160k"
        ));
        assert!(has_pair(&run("ogg", QualityLevel::Normal, 2), "-q:a", "4"));
        let wav = run("wav", QualityLevel::Normal, 2);
        assert!(has_pair(&wav, "-c:a", "pcm_s16le"));
        assert!(has_pair(&wav, "-rf64", "auto"));
        let flac = run("flac", QualityLevel::Normal, 0);
        assert!(has_pair(&flac, "-c:a", "flac"));
        assert!(!strs(&flac).contains(&"-b:a"));
    }

    #[test]
    fn missing_streams_and_unknown_targets_are_errors() {
        let silent = Source {
            video: h264_mp4().video,
            ..Source::default()
        };
        let p = Path::new("x");
        assert!(plan(p, p, "mp3", &opts(QualityLevel::Normal), &silent, p).is_err());
        assert!(plan(p, p, "mp4", &opts(QualityLevel::Normal), &wav(), p).is_err());
        assert!(plan(p, p, "gif", &opts(QualityLevel::Normal), &wav(), p).is_err());
        assert!(plan(p, p, "png", &opts(QualityLevel::Normal), &wav(), p).is_err());
        assert!(plan(p, p, "docx", &opts(QualityLevel::Normal), &wav(), p).is_err());
        // A silent video is fine for a video target: it just gets -an.
        let args = &plan(p, p, "webm", &opts(QualityLevel::Normal), &silent, p).unwrap()[0].args;
        assert!(strs(args).contains(&"-an"));
    }

    #[test]
    fn nearest_rate_keeps_broadcast_rates_and_rounds_the_rest() {
        assert_eq!(nearest_rate(23.976).as_deref(), Some("24000/1001"));
        assert_eq!(nearest_rate(25.0).as_deref(), Some("25"));
        assert_eq!(nearest_rate(29.87).as_deref(), Some("30"));
        assert_eq!(nearest_rate(240.0).as_deref(), Some("120"));
        assert_eq!(nearest_rate(0.0), None);
    }

    #[test]
    fn edges_cover_the_v010_matrix() {
        assert_eq!(edge("mp4", "gif"), Some(15));
        assert_eq!(edge("mp4", "png"), Some(10));
        assert_eq!(edge("mkv", "mp4"), Some(10));
        assert_eq!(edge("mp4", "mp4"), Some(10));
        assert_eq!(edge("avi", "opus"), Some(10));
        assert_eq!(edge("wav", "mp3"), Some(10));
        assert_eq!(edge("flac", "flac"), Some(10));
        assert_eq!(edge("gif", "mp4"), Some(10));
        assert_eq!(edge("gif", "webm"), Some(10));
        // Not in v0.1.0: audio can't become video, gif -> mov waits for later.
        assert_eq!(edge("mp3", "mp4"), None);
        assert_eq!(edge("gif", "mov"), None);
        assert_eq!(edge("png", "mp4"), None);
        assert_eq!(EDGES.len(), 25 + 30 + 36 + 5 + 2 + 5);
        // No duplicates, and every target is something plan() can write.
        for (i, e) in EDGES.iter().enumerate() {
            assert!(!EDGES[..i].contains(e), "duplicate edge {e:?}");
            assert!(
                matches!(e.to.as_str(), "gif" | "png") || target_spec(e.to.as_str()).is_some(),
                "no writer for {}",
                e.to
            );
            assert_ne!(e.from, FormatId::new(""));
        }
    }

    // ───────────── MediaOptions ─────────────

    fn preset(id: &str) -> ConvertOptions {
        let presets = morphring_core::builtin::presets();
        morphring_core::preset::find(&presets, id)
            .unwrap_or_else(|| panic!("no built-in preset {id}"))
            .options()
    }

    fn audio_only(options: &ConvertOptions, to: &str, source: &Source<'_>) -> Vec<OsString> {
        audio_args(Path::new("in"), Path::new("out"), to, options, source).unwrap()
    }

    #[test]
    fn the_command_line_quality_beats_the_preset() {
        let mut options = opts(QualityLevel::Small);
        assert_eq!(quality_of(&options), QualityLevel::Small);
        options.quality = Some(90);
        assert_eq!(quality_of(&options), QualityLevel::High);
        options.quality = Some(70);
        assert_eq!(quality_of(&options), QualityLevel::Normal);
        assert_eq!(quality_of(&opts(QualityLevel::High)), QualityLevel::High);
        assert_eq!(quality_of(&ConvertOptions::default()), QualityLevel::Normal);
    }

    #[test]
    fn mp4_720_preset_shrinks_a_1080p_clip() {
        let options = preset("mp4-720");
        let args = video_args(
            Path::new("in.mp4"),
            Path::new("out.mp4"),
            "mp4",
            &options,
            &h264_mp4(),
        )
        .unwrap();
        assert_eq!(
            codec_part(&args),
            [
                "-vf",
                "scale=w=-2:h='trunc(min(ih,720)/2)*2'",
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "28",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
            ]
        );
    }

    #[test]
    fn a_max_height_above_the_source_still_copies() {
        let mut options = opts(QualityLevel::High);
        options.media.max_height = 2160;
        let args =
            video_args(Path::new("i"), Path::new("o"), "mkv", &options, &h264_mp4()).unwrap();
        assert_eq!(codec_part(&args), ["-c:v", "copy", "-c:a", "copy"]);
        // Same height: nothing to shrink either.
        options.media.max_height = 1080;
        let args =
            video_args(Path::new("i"), Path::new("o"), "mkv", &options, &h264_mp4()).unwrap();
        assert!(has_pair(&args, "-c:v", "copy"));
    }

    #[test]
    fn a_max_height_below_the_source_forces_a_re_encode() {
        let mut options = opts(QualityLevel::High);
        options.media.max_height = 480;
        let args =
            video_args(Path::new("i"), Path::new("o"), "mkv", &options, &h264_mp4()).unwrap();
        assert!(has_pair(
            &args,
            "-vf",
            "scale=w=-2:h='trunc(min(ih,480)/2)*2'"
        ));
        assert!(has_pair(&args, "-crf", "18"));
        // The soundtrack doesn't care about pixels and is still copied.
        assert!(has_pair(&args, "-c:a", "copy"));
    }

    #[test]
    fn downscale_uses_the_displayed_height_and_a_floor() {
        let mut portrait = h264_mp4().video.unwrap();
        // A phone clip stored 1920x1080 and shown rotated is 1920 lines tall.
        portrait.display_height = 1920;
        assert!(needs_downscale(&portrait, 1080));
        assert!(!needs_downscale(&portrait, 0));
        assert!(!needs_downscale(&portrait, 1920));
        portrait.display_height = 0;
        assert!(!needs_downscale(&portrait, 1080));
        assert_eq!(downscale_filter(4), "scale=w=-2:h='trunc(min(ih,16)/2)*2'");
    }

    #[test]
    fn remove_audio_drops_the_soundtrack() {
        let mut options = ConvertOptions::default();
        options.media.remove_audio = true;
        let args =
            video_args(Path::new("i"), Path::new("o"), "mp4", &options, &h264_mp4()).unwrap();
        assert_eq!(codec_part(&args), ["-c:v", "copy", "-an"]);
        assert!(!strs(&args).contains(&"0:a:0"));
    }

    #[test]
    fn audio_overrides_in_video_re_encode_the_soundtrack() {
        let mut options = ConvertOptions::default();
        options.media.audio_channels = 1;
        let args =
            video_args(Path::new("i"), Path::new("o"), "mkv", &options, &h264_mp4()).unwrap();
        // Mono halves the stereo default.
        assert_eq!(
            codec_part(&args),
            ["-c:v", "copy", "-c:a", "aac", "-b:a", "96k", "-ac", "1"]
        );
        options.media.audio_channels = 0;
        options.media.audio_bitrate_kbps = 160;
        let args =
            video_args(Path::new("i"), Path::new("o"), "avi", &options, &h264_mp4()).unwrap();
        assert!(has_pair(&args, "-c:a", "libmp3lame"));
        assert!(has_pair(&args, "-b:a", "160k"));
        // Sample rates are a WAV thing.
        options.media.audio_sample_rate = 22050;
        let args =
            video_args(Path::new("i"), Path::new("o"), "mkv", &options, &h264_mp4()).unwrap();
        assert!(!strs(&args).contains(&"-ar"));
    }

    #[test]
    fn mp3_128_preset_pins_the_full_arg_list() {
        let args = audio_args(
            Path::new("in.wav"),
            Path::new("out.mp3"),
            "mp3",
            &preset("mp3-128"),
            &wav(),
        )
        .unwrap();
        assert_eq!(
            strs(&args),
            [
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-v",
                "error",
                "-y",
                "-i",
                "in.wav",
                "-map",
                "0:a:0",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "128k",
                "-map_metadata",
                "0",
                "-metadata",
                "major_brand=",
                "-metadata",
                "minor_version=",
                "-metadata",
                "compatible_brands=",
                "-id3v2_version",
                "3",
                "-f",
                "mp3",
                "-progress",
                "pipe:1",
                "out.mp3",
            ]
        );
    }

    #[test]
    fn requested_bitrates_are_snapped_and_clamped() {
        let mut options = ConvertOptions::default();
        let rate = |options: &ConvertOptions, to: &str| {
            let args = audio_only(options, to, &wav());
            let all = strs(&args);
            let at = all.iter().position(|a| *a == "-b:a").unwrap();
            all[at + 1].to_owned()
        };
        options.media.audio_bitrate_kbps = 130;
        assert_eq!(rate(&options, "mp3"), "128k");
        options.media.audio_bitrate_kbps = 999;
        assert_eq!(rate(&options, "mp3"), "320k");
        assert_eq!(rate(&options, "ogg"), "500k");
        // AAC tops out at 256 k per channel: two of them here.
        assert_eq!(rate(&options, "m4a"), "512k");
        options.media.audio_bitrate_kbps = 2;
        assert_eq!(rate(&options, "opus"), "6k");
        assert_eq!(rate(&options, "ogg"), "32k");
        // Lossless ignores the request instead of inventing a flag.
        options.media.audio_bitrate_kbps = 128;
        assert!(!strs(&audio_only(&options, "flac", &wav())).contains(&"-b:a"));
    }

    #[test]
    fn wav_options_pick_the_codec_channels_and_rate() {
        let mut options = ConvertOptions::default();
        options.media.wav_format = WavFormat::Pcm24;
        options.media.audio_channels = 1;
        options.media.audio_sample_rate = 22050;
        assert_eq!(
            codec_part(&audio_only(&options, "wav", &wav())),
            ["-c:a", "pcm_s24le", "-ac", "1", "-ar", "22050"]
        );
        // Out-of-range rates and channel counts are ignored, not passed on.
        options.media.audio_channels = 6;
        options.media.audio_sample_rate = 100;
        options.media.wav_format = WavFormat::Float32;
        assert_eq!(
            codec_part(&audio_only(&options, "wav", &wav())),
            ["-c:a", "pcm_f32le"]
        );
        assert_eq!(wav_codec(WavFormat::Pcm8), "pcm_u8");
        assert_eq!(wav_codec(WavFormat::Pcm16), "pcm_s16le");
        assert_eq!(wav_codec(WavFormat::Pcm32), "pcm_s32le");
        // The rate is WAV only.
        options.media.audio_sample_rate = 22050;
        assert!(!strs(&audio_only(&options, "flac", &wav())).contains(&"-ar"));
    }

    #[test]
    fn forced_mono_scales_the_default_bitrate() {
        let mut options = opts(QualityLevel::High);
        options.media.audio_channels = 1;
        let args = audio_only(&options, "m4a", &wav());
        assert_eq!(
            codec_part(&args),
            ["-c:a", "aac", "-b:a", "128k", "-ac", "1"]
        );
    }
}
