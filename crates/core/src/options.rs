//! Per-family conversion options (image, PDF, document, media, archive).
//! Ported from Filee's `src/Filee.Core/Presets/Preset.cs` (`ImageOptions`,
//! `PdfOptions`, `DocumentOptions`, `MediaOptions`, `ArchiveOptions`).
//!
//! Core only stores these; the engines decide what they mean (a `MediaQuality`
//! becomes an FFmpeg CRF somewhere far away from here). The JSON shape follows
//! Filee's settings files: camelCase fields, enums as their variant names, and
//! every missing field falls back to its default.

use serde::{Deserialize, Serialize};

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
}
