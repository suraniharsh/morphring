//! Per-family conversion options (image, PDF, document, media, archive,
//! tracing). Ported from Filee's `src/Filee.Core/Presets/Preset.cs`
//! (`ImageOptions`, `PdfOptions`, `DocumentOptions`, `MediaOptions`,
//! `MediaCodecs`, `ArchiveOptions`, `TracingOptions`).
//!
//! Core only stores these; the engines decide what they mean (a `MediaQuality`
//! becomes an FFmpeg CRF somewhere far away from here). The JSON shape follows
//! Filee's settings files: camelCase fields, enums as their variant names, and
//! every missing field falls back to its default. Numbers with a fixed range
//! are clamped into it on load, so a hand-edited `"colors": 9000` becomes 64
//! instead of a failed settings file.

use serde::{Deserialize, Deserializer, Serialize};

/// Reads any JSON integer and squeezes it into `min..=max`. Filee stores these
/// as plain `int`, so a negative or enormous value is perfectly legal JSON;
/// we just refuse to believe it.
fn clamped<'de, D: Deserializer<'de>>(d: D, min: u8, max: u8) -> Result<u8, D::Error> {
    let value = i64::deserialize(d)?;
    // The clamp guarantees the cast fits, so no tears are shed over `as`.
    Ok(value.clamp(i64::from(min), i64::from(max)) as u8)
}

/// A percentage, 0 to 100.
fn percent<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    clamped(d, 0, 100)
}

/// A colour count for tracing, [`TracingOptions::MIN_COLORS`] to
/// [`TracingOptions::MAX_COLORS`].
fn trace_colors<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    clamped(d, TracingOptions::MIN_COLORS, TracingOptions::MAX_COLORS)
}

/// How an image is resized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resize {
    /// Keep the source size.
    #[default]
    None,
    /// Scale to `resize_percent` of the source size.
    Percent,
    /// Fit the longer edge to `long_edge` pixels. Never upscales.
    LongEdge,
    /// Fit the width to `width` pixels. Never upscales.
    Width,
}

/// Compression used for TIFF output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TiffCompression {
    None,
    #[default]
    Lzw,
    Zip,
}

/// Page size used when images become a PDF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PageSize {
    /// Each page is exactly the size of its image.
    #[default]
    FitImage,
    A4,
    Letter,
}

/// Quality bucket for audio and video.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaQuality {
    High,
    #[default]
    Balanced,
    Small,
}

/// Sample format of WAV output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WavFormat {
    #[default]
    Pcm16,
    Pcm8,
    Pcm24,
    Pcm32,
    Float32,
}

/// Video codec of a video target. `Default` is the target's usual codec
/// (H.264, VP9 for WebM, ...).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoCodec {
    #[default]
    Default,
    /// H.265 / HEVC: about half the size of H.264 at the same quality.
    Hevc,
    /// AV1: smaller still, slower to encode.
    Av1,
}

/// Audio codec of an audio target. `Default` is the target's usual codec
/// (AAC for M4A, ...).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioCodec {
    #[default]
    Default,
    /// Apple Lossless in M4A / M4B.
    Alac,
}

/// The video codecs `target` (a format id) can be written with. Only
/// [`VideoCodec::Default`] for most targets.
pub fn video_codecs_for(target: &str) -> &'static [VideoCodec] {
    match target {
        // AV1 in MP4 plays in browsers, Android and Windows. QuickTime has no
        // AV1 mapping, so MOV gets to sit this one out.
        "mp4" | "m4v" | "mkv" => &[VideoCodec::Default, VideoCodec::Hevc, VideoCodec::Av1],
        "mov" => &[VideoCodec::Default, VideoCodec::Hevc],
        // The WebM spec allows VP8, VP9 and AV1, and HEVC is not invited.
        "webm" => &[VideoCodec::Default, VideoCodec::Av1],
        _ => &[VideoCodec::Default],
    }
}

/// The audio codecs `target` (a format id) can be written with. Only
/// [`AudioCodec::Default`] for most targets.
pub fn audio_codecs_for(target: &str) -> &'static [AudioCodec] {
    match target {
        "m4a" | "m4b" => &[AudioCodec::Default, AudioCodec::Alac],
        _ => &[AudioCodec::Default],
    }
}

/// Whether `target` can be written with the video codec `codec`.
pub fn video_codec_valid(target: &str, codec: VideoCodec) -> bool {
    video_codecs_for(target).contains(&codec)
}

/// Whether `target` can be written with the audio codec `codec`.
pub fn audio_codec_valid(target: &str, codec: AudioCodec) -> bool {
    audio_codecs_for(target).contains(&codec)
}

/// The video codec `target` is actually written with: `codec` when the target
/// takes it, else [`VideoCodec::Default`].
pub fn effective_video_codec(target: &str, codec: VideoCodec) -> VideoCodec {
    if video_codec_valid(target, codec) {
        codec
    } else {
        VideoCodec::Default
    }
}

/// The audio codec `target` is actually written with: `codec` when the target
/// takes it, else [`AudioCodec::Default`].
pub fn effective_audio_codec(target: &str, codec: AudioCodec) -> AudioCodec {
    if audio_codec_valid(target, codec) {
        codec
    } else {
        AudioCodec::Default
    }
}

/// Compression level of archive output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArchiveLevel {
    Store,
    Fast,
    #[default]
    Normal,
    Maximum,
}

/// Options for image targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageOptions {
    /// 1 to 100, for lossy formats (JPG, WEBP, AVIF, HEIC, JXL).
    pub quality: u8,
    pub resize: Resize,
    /// Used with [`Resize::Percent`].
    pub resize_percent: u32,
    /// Used with [`Resize::LongEdge`].
    pub long_edge: u32,
    /// Used with [`Resize::Width`].
    pub width: u32,
    /// Output DPI; `None` keeps the source value.
    pub dpi: Option<u32>,
    /// Keep EXIF, XMP and ICC data.
    pub keep_metadata: bool,
    /// Keep the ICC profile even when the rest of the metadata is stripped.
    pub keep_color_profile: bool,
    pub grayscale: bool,
    /// Make a plain backdrop (the paper behind a signature, the backdrop of a
    /// logo) transparent. Targets without alpha get `background` instead.
    pub remove_background: bool,
    /// 0 to 100: how far from the backdrop colour a pixel may be and still be
    /// removed.
    #[serde(deserialize_with = "percent")]
    pub background_tolerance: u8,
    /// With `remove_background`: only remove the backdrop connected to the
    /// image edge, keeping enclosed areas such as the white inside a logo.
    pub keep_enclosed_background: bool,
    /// Fill for transparent pixels when the target has no alpha, as `#RRGGBB`.
    pub background: String,
    pub tiff_compression: TiffCompression,
    /// Several images become one multi-page TIFF.
    pub multi_page_tiff: bool,
    pub webp_lossless: bool,
    /// Sizes, in pixels, of the images inside an ICO file.
    pub ico_sizes: Vec<u32>,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            resize: Resize::None,
            resize_percent: 50,
            long_edge: 1920,
            width: 1200,
            dpi: None,
            keep_metadata: true,
            keep_color_profile: true,
            grayscale: false,
            remove_background: false,
            background_tolerance: 25,
            keep_enclosed_background: false,
            background: "#FFFFFF".to_owned(),
            tiff_compression: TiffCompression::Lzw,
            multi_page_tiff: false,
            webp_lossless: false,
            ico_sizes: vec![256, 128, 64, 48, 32, 16],
        }
    }
}

/// Options for PDF sources and targets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PdfOptions {
    /// Every input goes into one PDF.
    pub merge_into_single: bool,
    /// Image to PDF.
    pub page_size: PageSize,
    /// Page margin in millimetres; ignored for [`PageSize::FitImage`].
    pub margin_mm: f32,
    /// PDF to image resolution.
    pub render_dpi: u32,
    /// Pages to use, such as `"1-3,5"`; empty means every page.
    pub page_range: String,
    /// PDF to PDF, one file per page.
    pub split_pages: bool,
    /// PDF to one tall image.
    pub combine_pages: bool,
    /// With `combine_pages`: trim the white margin of each page.
    pub trim_page_margins: bool,
    /// With `combine_pages`: split an image that is too tall instead of
    /// scaling it down.
    pub split_tall_image: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            merge_into_single: false,
            page_size: PageSize::FitImage,
            margin_mm: 0.0,
            render_dpi: 150,
            page_range: String::new(),
            split_pages: false,
            combine_pages: false,
            trim_page_margins: false,
            split_tall_image: false,
        }
    }
}

/// Options for office document targets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DocumentOptions {
    /// Write PDF/A instead of plain PDF.
    pub pdf_a: bool,
}

/// Options for audio and video targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaOptions {
    pub quality: MediaQuality,
    /// Maximum video height in pixels; 0 keeps the source size. Never upscales.
    pub max_height: u32,
    /// Audio bitrate; 0 picks a good default for the format.
    pub audio_bitrate_kbps: u32,
    pub remove_audio: bool,
    /// 0 keeps the source, 1 is mono, 2 is stereo.
    pub audio_channels: u32,
    /// WAV only, 8000 to 48000; 0 keeps the source rate.
    pub audio_sample_rate: u32,
    pub wav_format: WavFormat,
    /// Video codec for targets that hold several (see [`video_codecs_for`]);
    /// other targets use their usual codec.
    pub video_codec: VideoCodec,
    /// Audio codec for targets that hold several (see [`audio_codecs_for`]).
    pub audio_codec: AudioCodec,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self {
            quality: MediaQuality::Balanced,
            max_height: 0,
            audio_bitrate_kbps: 0,
            remove_audio: false,
            audio_channels: 0,
            audio_sample_rate: 0,
            wav_format: WavFormat::Pcm16,
            video_codec: VideoCodec::Default,
            audio_codec: AudioCodec::Default,
        }
    }
}

/// Options for archive targets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArchiveOptions {
    /// Every input goes into one archive.
    pub combine_into_one: bool,
    pub level: ArchiveLevel,
}

/// How a raster image is traced into vector shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TracingMode {
    /// The image is reduced to `colors` colours, each traced as stacked shapes.
    #[default]
    Color,
    /// Dark pixels become black shapes on a transparent background (logos,
    /// signatures, scans).
    BlackAndWhite,
}

/// Options for raster to vector tracing (PNG to SVG, DXF, ...).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TracingOptions {
    pub mode: TracingMode,
    /// Number of colours in [`TracingMode::Color`], [`Self::MIN_COLORS`] to
    /// [`Self::MAX_COLORS`].
    #[serde(deserialize_with = "trace_colors")]
    pub colors: u8,
    /// 0 to 100: low values smooth the outlines and drop specks, high values
    /// follow the pixels and keep small details.
    #[serde(deserialize_with = "percent")]
    pub detail: u8,
}

impl TracingOptions {
    /// Fewest colours a colour trace may use.
    pub const MIN_COLORS: u8 = 2;
    /// Most colours a colour trace may use.
    pub const MAX_COLORS: u8 = 64;
}

impl Default for TracingOptions {
    fn default() -> Self {
        Self {
            mode: TracingMode::Color,
            colors: 16,
            detail: 50,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_filee() {
        let image = ImageOptions::default();
        assert_eq!(image.quality, 90);
        assert_eq!(image.resize, Resize::None);
        assert_eq!(image.resize_percent, 50);
        assert_eq!(image.long_edge, 1920);
        assert_eq!(image.width, 1200);
        assert_eq!(image.dpi, None);
        assert!(image.keep_metadata);
        assert!(image.keep_color_profile);
        assert!(!image.grayscale);
        assert_eq!(image.background, "#FFFFFF");
        assert_eq!(image.tiff_compression, TiffCompression::Lzw);
        assert!(!image.multi_page_tiff);
        assert!(!image.webp_lossless);
        assert_eq!(image.ico_sizes, [256, 128, 64, 48, 32, 16]);

        let pdf = PdfOptions::default();
        assert!(!pdf.merge_into_single);
        assert_eq!(pdf.page_size, PageSize::FitImage);
        assert_eq!(pdf.margin_mm, 0.0);
        assert_eq!(pdf.render_dpi, 150);
        assert_eq!(pdf.page_range, "");
        assert!(!pdf.split_pages);
        assert!(!pdf.combine_pages);
        assert!(!pdf.trim_page_margins);
        assert!(!pdf.split_tall_image);

        assert!(!DocumentOptions::default().pdf_a);

        let media = MediaOptions::default();
        assert_eq!(media.quality, MediaQuality::Balanced);
        assert_eq!(media.max_height, 0);
        assert_eq!(media.audio_bitrate_kbps, 0);
        assert!(!media.remove_audio);
        assert_eq!(media.audio_channels, 0);
        assert_eq!(media.audio_sample_rate, 0);
        assert_eq!(media.wav_format, WavFormat::Pcm16);

        let archive = ArchiveOptions::default();
        assert!(!archive.combine_into_one);
        assert_eq!(archive.level, ArchiveLevel::Normal);
    }

    #[test]
    fn json_is_camel_case_with_string_enums() {
        let json = serde_json::to_value(ImageOptions {
            resize: Resize::LongEdge,
            ..ImageOptions::default()
        })
        .unwrap();
        assert_eq!(json["resize"], "LongEdge");
        assert_eq!(json["resizePercent"], 50);
        assert_eq!(json["tiffCompression"], "Lzw");
        assert_eq!(json["keepColorProfile"], true);

        let pdf = serde_json::to_value(PdfOptions::default()).unwrap();
        assert_eq!(pdf["pageSize"], "FitImage");
        assert_eq!(pdf["marginMm"], 0.0);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // A settings file from an older version knows half the fields; the rest
        // should quietly take their defaults instead of failing the whole load.
        let image: ImageOptions = serde_json::from_str(r#"{"quality":70}"#).unwrap();
        assert_eq!(image.quality, 70);
        assert_eq!(image.long_edge, 1920);

        let media: MediaOptions =
            serde_json::from_str(r#"{"quality":"Small","maxHeight":720}"#).unwrap();
        assert_eq!(media.quality, MediaQuality::Small);
        assert_eq!(media.max_height, 720);
        assert_eq!(media.wav_format, WavFormat::Pcm16);

        let pdf: PdfOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(pdf, PdfOptions::default());
        let archive: ArchiveOptions = serde_json::from_str(r#"{"level":"Store"}"#).unwrap();
        assert_eq!(archive.level, ArchiveLevel::Store);
    }

    #[test]
    fn new_option_defaults_match_filee() {
        let image = ImageOptions::default();
        assert!(!image.remove_background);
        // Filee's Preset.cs says 25, and Filee is the one we answer to.
        assert_eq!(image.background_tolerance, 25);
        assert!(!image.keep_enclosed_background);

        let media = MediaOptions::default();
        assert_eq!(media.video_codec, VideoCodec::Default);
        assert_eq!(media.audio_codec, AudioCodec::Default);

        let trace = TracingOptions::default();
        assert_eq!(trace.mode, TracingMode::Color);
        assert_eq!(trace.colors, 16);
        assert_eq!(trace.detail, 50);
    }

    #[test]
    fn options_out_of_range_values_are_clamped_on_load() {
        let image: ImageOptions = serde_json::from_str(r#"{"backgroundTolerance":250}"#).unwrap();
        assert_eq!(image.background_tolerance, 100);
        let image: ImageOptions = serde_json::from_str(r#"{"backgroundTolerance":-3}"#).unwrap();
        assert_eq!(image.background_tolerance, 0);

        let trace: TracingOptions = serde_json::from_str(r#"{"colors":9000,"detail":-1}"#).unwrap();
        assert_eq!(trace.colors, TracingOptions::MAX_COLORS);
        assert_eq!(trace.detail, 0);
        let trace: TracingOptions = serde_json::from_str(r#"{"colors":1,"detail":101}"#).unwrap();
        assert_eq!(trace.colors, TracingOptions::MIN_COLORS);
        assert_eq!(trace.detail, 100);

        // In-range values come through untouched.
        let trace: TracingOptions = serde_json::from_str(r#"{"colors":2,"detail":100}"#).unwrap();
        assert_eq!((trace.colors, trace.detail), (2, 100));
    }

    #[test]
    fn options_new_fields_use_filee_key_names() {
        // Straight out of a Filee presets file.
        let image: ImageOptions = serde_json::from_str(
            r#"{"removeBackground":true,"backgroundTolerance":40,"keepEnclosedBackground":true}"#,
        )
        .unwrap();
        assert!(image.remove_background);
        assert_eq!(image.background_tolerance, 40);
        assert!(image.keep_enclosed_background);

        let media: MediaOptions =
            serde_json::from_str(r#"{"videoCodec":"Av1","audioCodec":"Alac"}"#).unwrap();
        assert_eq!(media.video_codec, VideoCodec::Av1);
        assert_eq!(media.audio_codec, AudioCodec::Alac);

        let trace: TracingOptions =
            serde_json::from_str(r#"{"mode":"BlackAndWhite","colors":8,"detail":70}"#).unwrap();
        assert_eq!(trace.mode, TracingMode::BlackAndWhite);
        assert_eq!((trace.colors, trace.detail), (8, 70));

        let json = serde_json::to_value(&image).unwrap();
        assert_eq!(json["removeBackground"], true);
        assert_eq!(json["backgroundTolerance"], 40);
        assert_eq!(json["keepEnclosedBackground"], true);
        let json = serde_json::to_value(&media).unwrap();
        assert_eq!(json["videoCodec"], "Av1");
        assert_eq!(json["audioCodec"], "Alac");
        let json = serde_json::to_value(&trace).unwrap();
        assert_eq!(json["mode"], "BlackAndWhite");

        assert_eq!(
            serde_json::from_value::<ImageOptions>(serde_json::to_value(&image).unwrap()).unwrap(),
            image
        );
        assert_eq!(
            serde_json::from_value::<MediaOptions>(serde_json::to_value(&media).unwrap()).unwrap(),
            media
        );
        assert_eq!(
            serde_json::from_value::<TracingOptions>(serde_json::to_value(&trace).unwrap())
                .unwrap(),
            trace
        );
    }

    #[test]
    fn options_video_codec_table() {
        use VideoCodec::{Av1, Default, Hevc};
        let table: &[(&str, [bool; 3])] = &[
            ("mp4", [true, true, true]),
            ("m4v", [true, true, true]),
            ("mkv", [true, true, true]),
            ("mov", [true, true, false]),
            ("webm", [true, false, true]),
            ("avi", [true, false, false]),
            ("gif", [true, false, false]),
            ("mp3", [true, false, false]),
        ];
        for (target, expected) in table {
            let got = [Default, Hevc, Av1].map(|c| video_codec_valid(target, c));
            assert_eq!(&got, expected, "{target}");
        }
        assert_eq!(effective_video_codec("webm", Hevc), Default);
        assert_eq!(effective_video_codec("webm", Av1), Av1);
        assert_eq!(effective_video_codec("mov", Av1), Default);
        assert_eq!(effective_video_codec("mkv", Hevc), Hevc);
    }

    #[test]
    fn options_audio_codec_table() {
        for target in ["m4a", "m4b"] {
            assert!(audio_codec_valid(target, AudioCodec::Default));
            assert!(audio_codec_valid(target, AudioCodec::Alac), "{target}");
        }
        for target in ["mp3", "flac", "wav", "mp4", "aac"] {
            assert!(audio_codec_valid(target, AudioCodec::Default));
            assert!(!audio_codec_valid(target, AudioCodec::Alac), "{target}");
        }
        assert_eq!(
            effective_audio_codec("mp3", AudioCodec::Alac),
            AudioCodec::Default
        );
        assert_eq!(
            effective_audio_codec("m4a", AudioCodec::Alac),
            AudioCodec::Alac
        );
    }
}
