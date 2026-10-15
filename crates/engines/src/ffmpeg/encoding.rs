//! Which container and codecs MorphRing writes for each video and audio format,
//! and the ffmpeg command lines that do it. Ported from Filee's
//! `src/Filee.Engines/Media/MediaEncoding.cs` (the v0.1.0 subset).
//!
//! Everything here is pure: no processes, no file system. The engine (T24)
//! probes the input, hands the facts in as a [`Source`] and runs the passes.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use morphring_core::{Edge, EngineError, QualityLevel};

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
    /// Stored width in pixels (before rotation), 0 when unknown.
    pub width: u32,
    /// Stored height in pixels (before rotation), 0 when unknown.
    pub height: u32,
    /// Average frame rate, `None` when unknown.
    pub fps: Option<f64>,
}

/// Facts about the input's audio stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioSource<'a> {
    /// ffprobe `codec_name`, such as `aac`.
    pub codec: Option<&'a str>,
    /// Channel count, 0 when unknown.
    pub channels: u32,
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
/// when `to` is `gif` and should sit in the job's work directory.
///
/// # Errors
///
/// Fails when ffmpeg cannot write `to`, or when the source lacks the stream the
/// target needs (no video for a video target, no audio for an audio target).
pub fn plan(
    input: &Path,
    output: &Path,
    to: &str,
    quality: QualityLevel,
    source: &Source<'_>,
    palette: &Path,
) -> Result<Vec<FfmpegPass>, EngineError> {
    let single = |args| vec![FfmpegPass { args, weight: 1.0 }];
    match to {
        "gif" => {
            let [first, second] = gif_args(input, output, quality, source, palette)?;
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
                Ok(single(video_args(input, output, to, quality, source)?))
            }
            Some(_) => Ok(single(audio_args(input, output, to, quality, source)?)),
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
    quality: QualityLevel,
    source: &Source<'_>,
) -> Result<Vec<OsString>, EngineError> {
    let video = source.video.as_ref().ok_or_else(no_video)?;
    let spec = target_spec(to)
        .filter(|s| s.video.is_some())
        .ok_or_else(|| EngineError::Failed(format!("'{to}' is not a video format.")))?;
    let video_codec = spec.video.unwrap_or("libx264");

    let mut a = Args::new();
    // 0:V (capital V) skips attached cover pictures, which ffmpeg otherwise
    // happily treats as a one-frame "video" stream.
    a.push("-i").push(input).extend(["-map", "0:V:0"]);
    if source.audio.is_some() {
        a.extend(["-map", "0:a:0"]);
    }

    if can_copy_video(to, video, quality) {
        a.extend(["-c:v", "copy"]);
    } else {
        a.push("-vf").push(scale_filter());
        a.extend(video_codec_args(video_codec, quality, video));
    }

    match &source.audio {
        None => {
            a.push("-an");
        }
        Some(audio) if can_copy_video(to, video, quality) && can_copy_audio(to, audio) => {
            a.extend(["-c:a", "copy"]);
        }
        Some(audio) => {
            a.extend(audio_codec_args(spec.audio, quality, audio.channels, true));
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

/// True when the video stream is rewrapped as is instead of re-encoded.
///
/// v0.1.0 keeps Filee's remux idea but only its most common case: h264 in
/// yuv420p going into mp4, mkv or mov, and never at `Small` quality (Small
/// always re-encodes; High and Normal only say how to encode when we must).
pub fn can_copy_video(to: &str, video: &VideoSource<'_>, quality: QualityLevel) -> bool {
    quality != QualityLevel::Small
        && matches!(to, "mp4" | "mkv" | "mov")
        && video.codec == Some("h264")
        && video.pix_fmt == Some("yuv420p")
}

/// Audio codecs the remux targets take as they are (Filee's table, trimmed to
/// the three containers v0.1.0 remuxes into).
fn can_copy_audio(to: &str, audio: &AudioSource<'_>) -> bool {
    let allowed: &[&str] = match to {
        "mp4" => &["aac", "alac", "ac3", "eac3", "mp3"],
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
            "aac", "alac", "ac3", "eac3", "mp3", "opus", "vorbis", "flac",
        ],
        _ => &[],
    };
    audio.codec.is_some_and(|c| allowed.contains(&c))
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
    quality: QualityLevel,
    source: &Source<'_>,
) -> Result<Vec<OsString>, EngineError> {
    let audio = source
        .audio
        .as_ref()
        .ok_or_else(|| EngineError::Failed("The file has no audio track.".into()))?;
    let spec = target_spec(to)
        .filter(|s| s.video.is_none())
        .ok_or_else(|| EngineError::Failed(format!("'{to}' is not an audio format.")))?;

    let mut a = Args::new();
    a.push("-i").push(input).extend(["-map", "0:a:0"]);
    a.extend(audio_codec_args(spec.audio, quality, audio.channels, false));
    a.extend(metadata_args());
    a.extend(container_args(to));
    a.extend(["-f", spec.muxer, "-progress", "pipe:1"])
        .push(output);
    Ok(a.finish())
}

/// Encoder options for one audio codec. `in_video` switches MP3 to CBR,
/// because VBR MP3 confuses AVI players' audio sync.
fn audio_codec_args(
    codec: &str,
    quality: QualityLevel,
    channels: u32,
    in_video: bool,
) -> Vec<String> {
    let channels = if channels > 0 { channels } else { 2 };
    let mut args = vec!["-c:a".to_owned(), codec.to_owned()];
    match codec {
        // Per stereo pair: 256 k is transparent, 192 excellent, 128 fine for speech.
        "aac" => args.extend(bitrate(
            scaled(choose(quality, 256, 192, 128), channels),
            16,
            256 * channels,
        )),
        // Opus is transparent around 128 k for stereo; 64 is still decent for music.
        "libopus" => args.extend(bitrate(
            scaled(choose(quality, 160, 128, 64), channels),
            6,
            256 * channels,
        )),
        // CBR inside video, snapped to a bitrate the MP3 table actually has.
        "libmp3lame" if in_video => args.extend(bitrate(
            snap(choose(quality, 256, 192, 128), &MP3_BITRATES),
            32,
            320,
        )),
        // LAME's VBR presets: V0 ~ 245, V2 ~ 190, V5 ~ 130 kbit/s.
        "libmp3lame" => args.extend(["-q:a".to_owned(), choose(quality, 0, 2, 5).to_string()]),
        // Vorbis quality 6 ~ 192, 4 ~ 128, 2 ~ 96 kbit/s.
        "libvorbis" => args.extend(["-q:a".to_owned(), choose(quality, 6, 4, 2).to_string()]),
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
                width: 1920,
                height: 1080,
                fps: Some(30.0),
            }),
            audio: Some(AudioSource {
                codec: Some("aac"),
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
                channels: 2,
            }),
            duration: Some(3.0),
        }
    }

    fn video(to: &str, quality: QualityLevel, source: &Source<'_>) -> Vec<OsString> {
        video_args(Path::new("in.mp4"), Path::new("out"), to, quality, source).unwrap()
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
            QualityLevel::Normal,
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

    #[test]
    fn hevc_into_mp4_re_encodes() {
        let mut source = h264_mp4();
        if let Some(v) = source.video.as_mut() {
            v.codec = Some("hevc");
        }
        let args = video("mp4", QualityLevel::High, &source);
        assert!(!strs(&args).contains(&"copy"));
        assert!(has_pair(&args, "-c:v", "libx264"));
        assert!(has_pair(&args, "-crf", "18"));
        assert!(has_pair(&args, "-movflags", "+faststart"));
        assert!(has_pair(&args, "-f", "mp4"));
    }

    #[test]
    fn ten_bit_h264_is_not_copied() {
        let mut source = h264_mp4();
        if let Some(v) = source.video.as_mut() {
            v.pix_fmt = Some("yuv420p10le");
        }
        assert!(!strs(&video("mov", QualityLevel::Normal, &source)).contains(&"copy"));
    }

    #[test]
    fn copied_video_still_re_encodes_audio_the_container_rejects() {
        let mut source = h264_mp4();
        source.audio = Some(AudioSource {
            codec: Some("opus"),
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
            QualityLevel::Small,
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
                q,
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
                    channels,
                }),
                ..Source::default()
            };
            audio_args(Path::new("a"), Path::new("b"), to, q, &source).unwrap()
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
        assert!(plan(p, p, "mp3", QualityLevel::Normal, &silent, p).is_err());
        assert!(plan(p, p, "mp4", QualityLevel::Normal, &wav(), p).is_err());
        assert!(plan(p, p, "gif", QualityLevel::Normal, &wav(), p).is_err());
        assert!(plan(p, p, "png", QualityLevel::Normal, &wav(), p).is_err());
        assert!(plan(p, p, "docx", QualityLevel::Normal, &wav(), p).is_err());
        // A silent video is fine for a video target: it just gets -an.
        let args = &plan(p, p, "webm", QualityLevel::Normal, &silent, p).unwrap()[0].args;
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
}
