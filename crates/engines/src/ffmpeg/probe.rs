//! Reading ffprobe's JSON and ffmpeg's `-progress` lines.
//!
//! Both are pure parsers: the FFmpeg engine runs the tools through the process
//! runner and hands their output here. Ported from Filee's
//! `src/Filee.Engines/Media/MediaInfo.cs` and `FfmpegRunner.ParsePosition`.

use std::ffi::OsString;
use std::path::Path;

use serde_json::Value;

/// The first real video stream of a file. Cover pictures are not counted.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    /// FFmpeg's codec name (`h264`, `hevc`, ...), if known.
    pub codec: Option<String>,
    /// FFmpeg's pixel format (`yuv420p`, `yuv420p10le`, ...), if known.
    pub pix_fmt: Option<String>,
    /// Coded width in pixels, 0 when unknown.
    pub width: u32,
    /// Coded height in pixels, 0 when unknown.
    pub height: u32,
    /// Display rotation in degrees, from the display matrix or the `rotate` tag.
    pub rotation: i32,
    /// Pixel aspect ratio, 1.0 for square pixels or when unknown.
    pub sample_aspect: f64,
    /// Average frames per second, 0.0 when unknown.
    pub frame_rate: f64,
}

impl VideoStream {
    /// True when the picture is displayed turned by 90 degrees.
    pub fn is_rotated(&self) -> bool {
        (self.rotation % 180).abs() == 90
    }

    /// Height of the picture as displayed (the coded width when rotated).
    pub fn display_height(&self) -> u32 {
        if self.is_rotated() {
            self.width
        } else {
            self.height
        }
    }

    /// Width divided by height as displayed, with pixel aspect and rotation
    /// applied; 16/9 when the size is unknown.
    pub fn display_aspect(&self) -> f64 {
        if self.width == 0 || self.height == 0 {
            return 16.0 / 9.0;
        }
        let aspect = f64::from(self.width) * self.sample_aspect / f64::from(self.height);
        if self.is_rotated() {
            1.0 / aspect
        } else {
            aspect
        }
    }
}

/// The first audio stream of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioStream {
    /// FFmpeg's codec name (`aac`, `opus`, ...), if known.
    pub codec: Option<String>,
    /// Channel count, 0 when unknown.
    pub channels: u32,
    /// Sample rate in Hz, 0 when unknown.
    pub sample_rate: u32,
    /// Bit rate in bits per second, if the stream reports one.
    pub bit_rate: Option<u64>,
}

/// What the FFmpeg engine needs to know about a media file before encoding it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaInfo {
    /// Playing time in seconds; `None` when the container does not say.
    pub duration: Option<f64>,
    /// First video stream that is not a cover picture.
    pub video: Option<VideoStream>,
    /// First audio stream.
    pub audio: Option<AudioStream>,
    /// Stream index of an embedded JPEG or PNG cover picture, such as album art.
    pub cover_stream_index: Option<u32>,
}

/// Arguments for `ffprobe` that print the format and streams of `input` as JSON.
pub fn probe_args(input: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    args
}

/// Parses the output of `ffprobe -print_format json -show_format -show_streams`.
///
/// Fails only when the text is not JSON; missing or malformed fields become
/// "unknown" values.
pub fn parse_media_info(json: &str) -> Result<MediaInfo, serde_json::Error> {
    let root: Value = serde_json::from_str(json)?;

    let mut info = MediaInfo::default();
    let mut longest_stream = 0.0_f64;

    for stream in root
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        longest_stream = longest_stream.max(number(stream, "duration"));
        match text(stream, "codec_type") {
            Some("video") if is_attached_picture(stream) => {
                // Album art rides along in a "video" stream with one frame. It is a cover
                // for audio targets that can hold one, and never, ever a 1-frame movie.
                if info.cover_stream_index.is_none()
                    && matches!(text(stream, "codec_name"), Some("mjpeg" | "png"))
                {
                    info.cover_stream_index = Some(to_u32(number(stream, "index")));
                }
            }
            Some("video") if info.video.is_none() => {
                let sar = parse_ratio(text(stream, "sample_aspect_ratio").unwrap_or_default());
                info.video = Some(VideoStream {
                    codec: owned_text(stream, "codec_name"),
                    pix_fmt: owned_text(stream, "pix_fmt"),
                    width: to_u32(number(stream, "width")),
                    height: to_u32(number(stream, "height")),
                    rotation: rotation_of(stream),
                    // "0:1" is ffprobe's way of shrugging, so anything non-positive means square.
                    sample_aspect: sar.filter(|s| *s > 0.0).unwrap_or(1.0),
                    frame_rate: frame_rate_of(stream),
                });
            }
            Some("audio") if info.audio.is_none() => {
                let bit_rate = number(stream, "bit_rate");
                info.audio = Some(AudioStream {
                    codec: owned_text(stream, "codec_name"),
                    channels: to_u32(number(stream, "channels")),
                    sample_rate: to_u32(number(stream, "sample_rate")),
                    bit_rate: (bit_rate > 0.0).then(|| bit_rate.round() as u64),
                });
            }
            _ => {}
        }
    }

    // Raw streams (an .h264 dump, say) have no container duration, so the longest
    // stream is the next best guess. Zero means nobody knows, which is honest at least.
    let mut seconds = root.get("format").map_or(0.0, |f| number(f, "duration"));
    if seconds <= 0.0 {
        seconds = longest_stream;
    }
    info.duration = (seconds > 0.0).then_some(seconds);
    Ok(info)
}

/// Turns one line of ffmpeg `-progress` output into a fraction of `duration`
/// (seconds), clamped to `0.0..=1.0`.
///
/// Reads `out_time_us=<microseconds>` and the GIF palette pass's
/// `frame_time=<seconds>`. Every other line, an `N/A` value or an unknown
/// duration gives `None`.
pub fn parse_progress_line(line: &str, duration: f64) -> Option<f32> {
    if !(duration.is_finite() && duration > 0.0) {
        return None;
    }
    let (key, value) = line.trim().split_once('=')?;
    let value = value.trim();
    let seconds = match key.trim() {
        // ffmpeg prints "N/A" here until the first frame comes out; the parse just fails.
        "out_time_us" => value.parse::<u64>().ok()? as f64 / 1_000_000.0,
        "frame_time" => value
            .parse::<f64>()
            .ok()
            .filter(|s| s.is_finite() && *s >= 0.0)?,
        _ => return None,
    };
    Some((seconds / duration).clamp(0.0, 1.0) as f32)
}

/// Parses `"a/b"` or `"a:b"` into `a / b`; `None` when malformed or `b <= 0`.
pub fn parse_ratio(text: &str) -> Option<f64> {
    let (num, den) = text.split_once(['/', ':'])?;
    let num = num.trim().parse::<f64>().ok()?;
    let den = den.trim().parse::<f64>().ok()?;
    let ratio = num / den;
    (den > 0.0 && ratio.is_finite()).then_some(ratio)
}

fn is_attached_picture(stream: &Value) -> bool {
    stream
        .get("disposition")
        .map_or(0.0, |d| number(d, "attached_pic"))
        == 1.0
}

/// Average frame rate, falling back to the nominal one when the average is
/// missing or silly ("0/0", or 90000 fps from a variable-rate container).
fn frame_rate_of(stream: &Value) -> f64 {
    let plausible = |key| {
        text(stream, key)
            .and_then(parse_ratio)
            .filter(|r| *r > 0.0 && *r < 1000.0)
    };
    plausible("avg_frame_rate")
        .or_else(|| plausible("r_frame_rate"))
        .unwrap_or(0.0)
}

/// Rotation from the display matrix side data, or the older `rotate` tag.
fn rotation_of(stream: &Value) -> i32 {
    // Phones film portrait by recording landscape and leaving a note saying "turn me".
    // Newer ffprobe puts the note in side_data_list, older ones in tags.rotate.
    let from_side_data = stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|item| item.get("rotation").and_then(as_number));
    if let Some(rotation) = from_side_data {
        return rotation.round() as i32;
    }
    stream
        .get("tags")
        .and_then(|tags| tags.get("rotate"))
        .and_then(as_number)
        .map_or(0, |r| r.round() as i32)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn owned_text(value: &Value, key: &str) -> Option<String> {
    text(value, key).map(str::to_owned)
}

/// A field as a number, 0.0 when missing or unreadable.
fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(as_number).unwrap_or(0.0)
}

/// ffprobe writes most numbers as strings ("2.000000") and a few as real JSON
/// numbers, so take both.
fn as_number(value: &Value) -> Option<f64> {
    let n = match value {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn to_u32(n: f64) -> u32 {
    // `as` saturates and maps NaN to 0, which is exactly the "unknown" we want.
    n as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const MP3_WITH_COVER: &str = r#"{
      "streams": [
        { "index": 0, "codec_name": "mp3", "codec_type": "audio", "sample_rate": "44100",
          "channels": 2, "bit_rate": "320000", "duration": "1.018776",
          "disposition": { "default": 0, "attached_pic": 0 } },
        { "index": 1, "codec_name": "mjpeg", "codec_type": "video", "width": 500, "height": 500,
          "pix_fmt": "yuvj420p", "avg_frame_rate": "0/0", "r_frame_rate": "90000/1",
          "disposition": { "default": 0, "attached_pic": 1 } }
      ],
      "format": { "filename": "song.mp3", "format_name": "mp3", "duration": "1.023000" }
    }"#;

    const PHONE_VIDEO: &str = r#"{
      "streams": [
        { "index": 0, "codec_name": "h264", "codec_type": "video", "width": 1920, "height": 1080,
          "pix_fmt": "yuv420p", "sample_aspect_ratio": "1:1", "avg_frame_rate": "30000/1001",
          "r_frame_rate": "30/1", "duration": "4.004000",
          "disposition": { "attached_pic": 0 },
          "side_data_list": [ { "side_data_type": "Display Matrix", "rotation": -90 } ] },
        { "index": 1, "codec_name": "aac", "codec_type": "audio", "sample_rate": "48000",
          "channels": 1, "duration": "4.010000" }
      ],
      "format": { "duration": 4.01 }
    }"#;

    #[test]
    fn mp3_cover_art_is_not_video() {
        let info = parse_media_info(MP3_WITH_COVER).unwrap();
        assert_eq!(info.video, None);
        assert_eq!(info.cover_stream_index, Some(1));
        let audio = info.audio.unwrap();
        assert_eq!(audio.codec.as_deref(), Some("mp3"));
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.sample_rate, 44100);
        assert_eq!(audio.bit_rate, Some(320_000));
    }

    #[test]
    fn duration_string_parses() {
        let info = parse_media_info(MP3_WITH_COVER).unwrap();
        assert_eq!(info.duration, Some(1.023));
    }

    #[test]
    fn duration_number_parses() {
        let info = parse_media_info(PHONE_VIDEO).unwrap();
        assert_eq!(info.duration, Some(4.01));
    }

    #[test]
    fn rotation_from_side_data_list() {
        let video = parse_media_info(PHONE_VIDEO).unwrap().video.unwrap();
        assert_eq!(video.rotation, -90);
        assert!(video.is_rotated());
        assert_eq!(video.display_height(), 1920);
        assert!((video.display_aspect() - 1080.0 / 1920.0).abs() < 1e-9);
        assert_eq!(video.codec.as_deref(), Some("h264"));
        assert_eq!(video.pix_fmt.as_deref(), Some("yuv420p"));
        assert_eq!((video.width, video.height), (1920, 1080));
        assert!((video.frame_rate - 29.97).abs() < 0.01);
    }

    #[test]
    fn rotation_from_rotate_tag() {
        let json = r#"{ "streams": [ { "codec_type": "video", "width": "640", "height": "480",
            "tags": { "rotate": "90" } } ] }"#;
        let video = parse_media_info(json).unwrap().video.unwrap();
        assert_eq!(video.rotation, 90);
        assert_eq!((video.width, video.height), (640, 480));
    }

    #[test]
    fn side_data_wins_over_tag() {
        let json = r#"{ "streams": [ { "codec_type": "video",
            "side_data_list": [ { "side_data_type": "Other" }, { "rotation": 180 } ],
            "tags": { "rotate": "90" } } ] }"#;
        assert_eq!(parse_media_info(json).unwrap().video.unwrap().rotation, 180);
    }

    #[test]
    fn duration_falls_back_to_longest_stream() {
        let json = r#"{ "streams": [
            { "codec_type": "video", "duration": "2.5" },
            { "codec_type": "audio", "duration": "3.25" } ],
            "format": { "duration": "N/A" } }"#;
        assert_eq!(parse_media_info(json).unwrap().duration, Some(3.25));
    }

    #[test]
    fn unknown_everything_is_none() {
        let info = parse_media_info("{}").unwrap();
        assert_eq!(info, MediaInfo::default());
        assert!(parse_media_info("not json").is_err());
    }

    #[test]
    fn first_streams_win() {
        let json = r#"{ "streams": [
            { "codec_type": "audio", "codec_name": "aac" },
            { "codec_type": "video", "codec_name": "h264" },
            { "codec_type": "audio", "codec_name": "mp3" },
            { "codec_type": "video", "codec_name": "vp9" } ] }"#;
        let info = parse_media_info(json).unwrap();
        assert_eq!(info.video.unwrap().codec.as_deref(), Some("h264"));
        let audio = info.audio.unwrap();
        assert_eq!(audio.codec.as_deref(), Some("aac"));
        assert_eq!(audio.bit_rate, None);
    }

    #[test]
    fn non_square_pixels_and_bad_sar() {
        let json = r#"{ "streams": [ { "codec_type": "video", "width": 720, "height": 480,
            "sample_aspect_ratio": "8:9" } ] }"#;
        let video = parse_media_info(json).unwrap().video.unwrap();
        assert!((video.sample_aspect - 8.0 / 9.0).abs() < 1e-9);

        let json = r#"{ "streams": [ { "codec_type": "video", "sample_aspect_ratio": "0:1" } ] }"#;
        let video = parse_media_info(json).unwrap().video.unwrap();
        assert_eq!(video.sample_aspect, 1.0);
        assert_eq!(video.display_aspect(), 16.0 / 9.0);
    }

    #[test]
    fn png_cover_counts_but_other_attached_pics_do_not() {
        let json = r#"{ "streams": [
            { "index": 2, "codec_type": "video", "codec_name": "bmp", "disposition": { "attached_pic": 1 } },
            { "index": 3, "codec_type": "video", "codec_name": "png", "disposition": { "attached_pic": "1" } } ] }"#;
        let info = parse_media_info(json).unwrap();
        assert_eq!(info.video, None);
        assert_eq!(info.cover_stream_index, Some(3));
    }

    #[test]
    fn ratios() {
        assert_eq!(parse_ratio("30000/1001"), Some(30000.0 / 1001.0));
        assert_eq!(parse_ratio("16:9"), Some(16.0 / 9.0));
        assert_eq!(parse_ratio("0/0"), None);
        assert_eq!(parse_ratio("1/-2"), None);
        assert_eq!(parse_ratio("abc"), None);
        assert_eq!(parse_ratio(""), None);
    }

    #[test]
    fn frame_rate_falls_back_to_nominal() {
        let json = r#"{ "streams": [ { "codec_type": "video", "avg_frame_rate": "0/0",
            "r_frame_rate": "25/1" } ] }"#;
        assert_eq!(
            parse_media_info(json).unwrap().video.unwrap().frame_rate,
            25.0
        );
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=500000", 1.0), Some(0.5));
        assert_eq!(parse_progress_line("out_time_us=5000000", 1.0), Some(1.0));
        assert_eq!(parse_progress_line("out_time_us=0", 2.0), Some(0.0));
        assert_eq!(parse_progress_line("frame_time=1.5", 3.0), Some(0.5));
        assert_eq!(parse_progress_line("out_time_us=N/A", 1.0), None);
        assert_eq!(parse_progress_line("out_time_us=-5", 1.0), None);
        assert_eq!(parse_progress_line("progress=continue", 1.0), None);
        assert_eq!(parse_progress_line("garbage", 1.0), None);
        assert_eq!(parse_progress_line("", 1.0), None);
        assert_eq!(parse_progress_line("out_time_us=500000", 0.0), None);
        assert_eq!(parse_progress_line("out_time_us=500000", f64::NAN), None);
    }

    #[test]
    fn probe_args_end_with_input() {
        let args = probe_args(Path::new("/tmp/my clip 한글.mp4"));
        let expected: Vec<OsString> = [
            "-hide_banner",
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "/tmp/my clip 한글.mp4",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(args, expected);
    }
}
