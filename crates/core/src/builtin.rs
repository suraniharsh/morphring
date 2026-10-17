//! The built-in presets Filee ships with.
//! Ported from Filee's `src/Filee.Core/Presets/BuiltInData.cs` (lines 15-235).
//!
//! Ids are stable: toolbar profiles and settings files point at them, so they
//! never change, and neither does the order (it is the order the editor lists
//! them in). Lookup by id or name lives in [`crate::preset::find`].

use crate::options::{AudioCodec, MediaQuality, Resize, TracingMode, VideoCodec};
use crate::output::OutputLocation;
use crate::preset::{Preset, SAME_AS_SOURCE};

/// A preset with a fixed name and nothing else changed.
fn named(id: &str, name: &str, target: &str) -> Preset {
    Preset {
        id: id.to_owned(),
        name: name.to_owned(),
        target: target.to_owned(),
        ..Preset::default()
    }
}

/// A built-in whose name is a localization key instead of fixed text.
fn keyed(id: &str, key: &str, target: &str) -> Preset {
    Preset {
        id: id.to_owned(),
        name_key: Some(key.to_owned()),
        target: target.to_owned(),
        ..Preset::default()
    }
}

/// `preset` with image quality `quality`.
fn quality(mut preset: Preset, quality: u8) -> Preset {
    preset.image.quality = quality;
    preset
}

/// `preset` with file name pattern `pattern`.
fn pattern(mut preset: Preset, pattern: &str) -> Preset {
    preset.output.pattern = pattern.to_owned();
    preset
}

/// Every built-in preset, in Filee's order. There are 94 of them, which is
/// more than anyone will ever put on one ring, and that is fine: the ring
/// only shows what a toolbar profile picks, the rest wait in the editor.
pub fn presets() -> Vec<Preset> {
    let mut list = vec![
        named("to-png", "PNG", "png"),
        quality(named("to-jpg", "JPG", "jpg"), 90),
        pattern(quality(named("jpg-70", "JPG 70%", "jpg"), 70), "{name}_70"),
        quality(named("to-webp", "WEBP", "webp"), 85),
        named("to-tiff", "TIFF", "tiff"),
        named("to-bmp", "BMP", "bmp"),
        named("to-ico", "ICO", "ico"),
        named("to-gif", "GIF", "gif"),
        quality(named("to-avif", "AVIF", "avif"), 60),
        quality(named("to-heic", "HEIC", "heic"), 80),
        {
            let mut p = keyed("half-size", "preset.half_size", SAME_AS_SOURCE);
            p.image.resize = Resize::Percent;
            p.image.resize_percent = 50;
            pattern(p, "{name}_50")
        },
        {
            let mut p = keyed("grayscale", "preset.grayscale", SAME_AS_SOURCE);
            p.image.grayscale = true;
            pattern(p, "{name}_gray")
        },
        {
            // Always PNG: a JPG with a transparent background is just a JPG
            // with a white background and extra disappointment.
            let mut p = keyed("remove-background", "preset.remove_background", "png");
            p.image.remove_background = true;
            pattern(p, "{name}_transparent")
        },
        {
            let mut p = named("tiff-300", "TIFF 300dpi", "tiff");
            p.image.dpi = Some(300);
            p.pdf.render_dpi = 300;
            pattern(p, "{name}_300dpi")
        },
        named("to-pdf", "PDF", "pdf"),
        {
            let mut p = keyed("merge-pdf", "preset.merge_pdf", "pdf");
            p.pdf.merge_into_single = true;
            pattern(p, "{name}_merged")
        },
        {
            let mut p = keyed("pdf-long-image", "preset.long_image", "png");
            p.pdf.combine_pages = true;
            pattern(p, "{name}_long")
        },
        {
            // One folder per PDF, named after it, so a 300-page split does not
            // carpet-bomb the source folder.
            let mut p = keyed("pdf-split", "preset.split_pdf", "pdf");
            p.pdf.split_pages = true;
            p.output.location = OutputLocation::Subfolder("{name}".to_owned());
            p
        },
        named("to-docx", "DOCX", "docx"),
        named("to-hwpx", "HWPX", "hwpx"),
        // Filee's HwpPresets(), added by a settings migration.
        named("to-hwp", "HWP", "hwp"),
        named("to-odt", "ODT", "odt"),
        named("to-rtf", "RTF", "rtf"),
        named("to-txt", "TXT", "txt"),
        named("to-md", "Markdown", "md"),
        named("to-html", "HTML", "html"),
        // Filee's SpreadsheetPresets().
        named("to-xlsx", "XLSX", "xlsx"),
        named("to-csv", "CSV", "csv"),
    ];
    list.extend(catalog());
    list.extend(data());
    list.extend(codecs_and_subtitles());
    list.extend(models());
    list.extend(tracing());
    list
}

/// Filee's `DataPresets()` (its settings schema 14). EML is meant for Outlook
/// MSG files and vCard for contact lists kept in CSV; neither sits on a ring.
fn data() -> Vec<Preset> {
    vec![
        named("to-json", "JSON", "json"),
        named("to-yaml", "YAML", "yaml"),
        named("to-toml", "TOML", "toml"),
        named("to-xml", "XML", "xml"),
        named("to-eml", "EML", "eml"),
        named("to-vcf", "vCard", "vcf"),
    ]
}

/// Filee's `CodecAndSubtitlePresets()` (its settings schema 11): HEVC and AV1
/// video, ALAC, and the subtitle targets.
fn codecs_and_subtitles() -> Vec<Preset> {
    vec![
        // Video codecs and lossless audio
        {
            let mut p = named("mp4-hevc", "MP4 HEVC", "mp4");
            p.media.video_codec = VideoCodec::Hevc;
            pattern(p, "{name}_hevc")
        },
        {
            // Smaller files, longer coffee breaks while the encoder thinks.
            let mut p = named("mp4-av1", "MP4 AV1", "mp4");
            p.media.video_codec = VideoCodec::Av1;
            pattern(p, "{name}_av1")
        },
        {
            let mut p = named("to-alac", "ALAC", "m4a");
            p.media.audio_codec = AudioCodec::Alac;
            p
        },
        // Subtitles. On the video ring, SRT means "pull the first text
        // subtitle track out of this video", hence the separate preset.
        keyed("extract-subtitles", "preset.extract_subtitles", "srt"),
        named("to-srt", "SRT", "srt"),
        named("to-vtt", "WebVTT", "vtt"),
        named("to-ass", "ASS", "ass"),
        named("to-smi", "SAMI", "smi"),
        named("to-lrc", "LRC", "lrc"),
        named("to-ttml", "TTML", "ttml"),
    ]
}

/// Filee's `ModelPresets()` (its settings schema 15). glTF is on no ring:
/// GLB is the same thing in one file, and one file is easier to email.
fn models() -> Vec<Preset> {
    vec![
        named("to-glb", "GLB", "glb"),
        named("to-stl", "STL", "stl"),
        named("to-obj", "OBJ", "obj"),
        named("to-3mf", "3MF", "3mf"),
        named("to-ply", "PLY", "ply"),
        named("to-gltf", "glTF", "gltf"),
    ]
}

/// Filee's `TracePresets()` (its settings schema 16): raster to SVG, in
/// colour and as black line art.
fn tracing() -> Vec<Preset> {
    vec![keyed("trace-svg", "preset.trace_svg", "svg"), {
        let mut p = keyed("trace-line-art", "preset.trace_line_art", "svg");
        p.trace.mode = TracingMode::BlackAndWhite;
        p
    }]
}

/// Filee's `CatalogPresets()`: the formats added in its settings schema 6.
fn catalog() -> Vec<Preset> {
    vec![
        // Images and vector graphics
        quality(named("to-jxl", "JPEG XL", "jxl"), 85),
        named("to-psd", "PSD", "psd"),
        named("to-tga", "TGA", "tga"),
        named("to-icns", "ICNS", "icns"),
        named("to-svg", "SVG", "svg"),
        named("to-eps", "EPS", "eps"),
        // Office and text
        named("to-ods", "ODS", "ods"),
        named("to-pptx", "PPTX", "pptx"),
        named("to-odp", "ODP", "odp"),
        // E-books
        named("to-epub", "EPUB", "epub"),
        named("to-mobi", "MOBI", "mobi"),
        named("to-azw3", "AZW3", "azw3"),
        named("to-kepub", "KEPUB", "kepub"),
        named("to-fb2", "FB2", "fb2"),
        named("to-cbz", "CBZ", "cbz"),
        // Video
        named("to-mp4", "MP4", "mp4"),
        {
            let mut p = named("mp4-720", "MP4 720p", "mp4");
            p.media.max_height = 720;
            p.media.quality = MediaQuality::Small;
            pattern(p, "{name}_720p")
        },
        named("to-webm", "WEBM", "webm"),
        named("to-mov", "MOV", "mov"),
        named("to-mkv", "MKV", "mkv"),
        named("to-avi", "AVI", "avi"),
        // Audio
        named("to-mp3", "MP3", "mp3"),
        {
            let mut p = named("mp3-128", "MP3 128k", "mp3");
            p.media.audio_bitrate_kbps = 128;
            pattern(p, "{name}_128k")
        },
        named("to-m4a", "M4A", "m4a"),
        named("to-wav", "WAV", "wav"),
        named("to-flac", "FLAC", "flac"),
        named("to-ogg", "OGG", "ogg"),
        named("to-opus", "OPUS", "opus"),
        named("to-aac", "AAC", "aac"),
        // Archives
        named("to-zip", "ZIP", "zip"),
        named("to-7z", "7Z", "7z"),
        named("to-tgz", "TAR.GZ", "tgz"),
        named("to-tar", "TAR", "tar"),
        keyed("extract", "preset.extract", "folder"),
        {
            let mut p = keyed("zip-all", "preset.zip_all", "zip");
            p.archive.combine_into_one = true;
            pattern(p, "{name}_files")
        },
        // CAD
        named("to-dxf", "DXF", "dxf"),
        named("to-dwg", "DWG", "dwg"),
        // Fonts
        named("to-ttf", "TTF", "ttf"),
        named("to-otf", "OTF", "otf"),
        named("to-woff", "WOFF", "woff"),
        named("to-woff2", "WOFF2", "woff2"),
        named("to-eot", "EOT", "eot"),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::formats;
    use crate::options::ImageOptions;

    #[test]
    fn preset_builtins_are_94_with_unique_ids() {
        let list = presets();
        assert_eq!(list.len(), 94);
        let ids: HashSet<_> = list.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), 94);
        assert_eq!(list.first().unwrap().id, "to-png");
        assert_eq!(list.last().unwrap().id, "trace-line-art");
    }

    #[test]
    fn preset_builtin_order_matches_filee() {
        let ids: Vec<String> = presets().into_iter().map(|p| p.id).collect();
        let at = |id: &str| {
            ids.iter()
                .position(|i| i == id)
                .unwrap_or_else(|| panic!("no built-in {id}"))
        };
        // Each pair is "a comes right before b" in BuiltInData.cs.
        for (a, b) in [
            ("grayscale", "remove-background"),
            ("remove-background", "tiff-300"),
            ("to-azw3", "to-kepub"),
            ("to-kepub", "to-fb2"),
            ("to-eot", "to-json"),
            ("to-vcf", "mp4-hevc"),
            ("to-ttml", "to-glb"),
            ("to-gltf", "trace-svg"),
            ("trace-svg", "trace-line-art"),
        ] {
            assert_eq!(at(a) + 1, at(b), "{a} then {b}");
        }
    }

    #[test]
    fn preset_builtin_targets_are_writable_formats() {
        for preset in presets() {
            if preset.target == SAME_AS_SOURCE {
                continue;
            }
            // By id, not extension: a target must be a real registry id.
            let format = formats::find_by_id(&preset.target)
                .unwrap_or_else(|| panic!("{}: unknown target {}", preset.id, preset.target));
            assert_eq!(format.id.as_str(), preset.target, "{}", preset.id);
            if preset.id == "to-eml" {
                // Filee writes EML; the registry keeps it read-only until an
                // engine here can (see formats::tests::table_matches_filee).
                // The preset still ships so settings files line up with Filee.
                continue;
            }
            assert!(
                format.writable,
                "{}: {} is read-only",
                preset.id, preset.target
            );
            assert_eq!(format.id.as_str(), preset.target, "{}", preset.id);
            assert!(preset.job_target().is_some(), "{}", preset.id);
        }
    }

    #[test]
    fn preset_builtin_locations_and_patterns() {
        for preset in presets() {
            let expected = if preset.id == "pdf-split" {
                OutputLocation::Subfolder("{name}".into())
            } else {
                OutputLocation::Default
            };
            assert_eq!(preset.output.location, expected, "{}", preset.id);
            // Named presets carry a name, keyed ones a key; never both, never neither.
            assert_ne!(
                preset.name.is_empty(),
                preset.name_key.is_none(),
                "{}",
                preset.id
            );
        }
        let by_id = |id: &str| presets().into_iter().find(|p| p.id == id).unwrap();
        assert_eq!(by_id("jpg-70").output.pattern, "{name}_70");
        assert_eq!(by_id("half-size").output.pattern, "{name}_50");
        assert_eq!(by_id("grayscale").output.pattern, "{name}_gray");
        assert_eq!(by_id("tiff-300").output.pattern, "{name}_300dpi");
        assert_eq!(by_id("merge-pdf").output.pattern, "{name}_merged");
        assert_eq!(by_id("pdf-long-image").output.pattern, "{name}_long");
        assert_eq!(by_id("pdf-split").output.pattern, "{name}");
        assert_eq!(by_id("mp4-720").output.pattern, "{name}_720p");
        assert_eq!(by_id("mp3-128").output.pattern, "{name}_128k");
        assert_eq!(by_id("zip-all").output.pattern, "{name}_files");
        assert_eq!(by_id("to-png").output.pattern, "{name}");
    }

    #[test]
    fn preset_builtin_non_default_options() {
        let by_id = |id: &str| presets().into_iter().find(|p| p.id == id).unwrap();
        assert_eq!(by_id("to-webp").image.quality, 85);
        assert_eq!(by_id("to-heic").image.quality, 80);
        assert_eq!(by_id("to-jxl").image.quality, 85);
        let tiff = by_id("tiff-300");
        assert_eq!((tiff.image.dpi, tiff.pdf.render_dpi), (Some(300), 300));
        assert!(by_id("pdf-split").pdf.split_pages);
        assert!(by_id("pdf-long-image").pdf.combine_pages);
        assert!(by_id("grayscale").image.grayscale);
        assert!(by_id("zip-all").archive.combine_into_one);
        let mp4 = by_id("mp4-720");
        assert_eq!(
            (mp4.media.max_height, mp4.media.quality),
            (720, MediaQuality::Small)
        );
        assert_eq!(by_id("mp3-128").media.audio_bitrate_kbps, 128);
        assert_eq!(by_id("to-mkv").image, ImageOptions::default());
    }

    #[test]
    fn preset_builtin_newer_filee_options() {
        let by_id = |id: &str| presets().into_iter().find(|p| p.id == id).unwrap();

        let bg = by_id("remove-background");
        assert!(bg.image.remove_background);
        assert_eq!(bg.target, "png");
        assert_eq!(bg.output.pattern, "{name}_transparent");
        assert_eq!(bg.name_key.as_deref(), Some("preset.remove_background"));

        let hevc = by_id("mp4-hevc");
        assert_eq!(hevc.media.video_codec, VideoCodec::Hevc);
        assert_eq!(hevc.output.pattern, "{name}_hevc");
        let av1 = by_id("mp4-av1");
        assert_eq!(av1.media.video_codec, VideoCodec::Av1);
        assert_eq!(av1.output.pattern, "{name}_av1");
        let alac = by_id("to-alac");
        assert_eq!(
            (alac.target.as_str(), alac.media.audio_codec),
            ("m4a", AudioCodec::Alac)
        );
        assert_eq!(alac.output.pattern, "{name}");

        let extract = by_id("extract-subtitles");
        assert_eq!(extract.target, "srt");
        assert_eq!(
            extract.name_key.as_deref(),
            Some("preset.extract_subtitles")
        );
        assert_eq!(by_id("to-vtt").name, "WebVTT");
        assert_eq!(by_id("to-smi").name, "SAMI");
        assert_eq!(by_id("to-vcf").name, "vCard");
        assert_eq!(by_id("to-gltf").name, "glTF");
        assert_eq!(by_id("to-kepub").target, "kepub");

        assert_eq!(by_id("trace-svg").trace.mode, TracingMode::default());
        assert_eq!(
            by_id("trace-line-art").trace.mode,
            TracingMode::BlackAndWhite
        );
        assert_eq!(by_id("trace-line-art").target, "svg");

        // Everything else stays on the stock codecs and tracing.
        for preset in presets() {
            if !["mp4-hevc", "mp4-av1"].contains(&preset.id.as_str()) {
                assert_eq!(
                    preset.media.video_codec,
                    VideoCodec::Default,
                    "{}",
                    preset.id
                );
            }
            if preset.id != "to-alac" {
                assert_eq!(
                    preset.media.audio_codec,
                    AudioCodec::Default,
                    "{}",
                    preset.id
                );
            }
            if preset.id != "remove-background" {
                assert!(!preset.image.remove_background, "{}", preset.id);
            }
        }
    }

    #[test]
    fn preset_builtin_name_keys_have_english_text() {
        // A key with no English text quietly falls back to the target in
        // upper case, and "Remove background" ends up cosplaying as "PNG".
        let list = presets();
        for preset in &list {
            if preset.name.is_empty() && preset.name_key.is_some() {
                assert_ne!(
                    preset.display_name(),
                    preset.target.to_uppercase(),
                    "{} has no English text for {:?}",
                    preset.id,
                    preset.name_key
                );
            }
        }
        for (name, id) in [
            ("Remove background", "remove-background"),
            ("Extract subtitles", "extract-subtitles"),
            ("Trace to SVG", "trace-svg"),
            ("Line art to SVG", "trace-line-art"),
        ] {
            assert_eq!(
                crate::preset::find(&list, name).map(|p| p.id.as_str()),
                Some(id),
                "{name}"
            );
        }
    }
}
