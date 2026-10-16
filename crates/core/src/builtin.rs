//! The built-in presets Filee ships with.
//! Ported from Filee's `src/Filee.Core/Presets/BuiltInData.cs` (lines 15-161).
//!
//! Ids are stable: toolbar profiles and settings files point at them, so they
//! never change, and neither does the order (it is the order the editor lists
//! them in). Lookup by id or name lives in [`crate::preset::find`].

use crate::options::{MediaQuality, Resize};
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

/// Every built-in preset, in Filee's order. There are 68 of them, which is
/// more than anyone will ever put on one ring, and that is fine.
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
    list
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
    fn preset_builtins_are_68_with_unique_ids() {
        let list = presets();
        assert_eq!(list.len(), 68);
        let ids: HashSet<_> = list.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), 68);
        assert_eq!(list.first().unwrap().id, "to-png");
        assert_eq!(list.last().unwrap().id, "to-eot");
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
}
