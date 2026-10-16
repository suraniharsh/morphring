//! The preset model: a named target format plus its options and output rule.
//! Ported from Filee's `src/Filee.Core/Presets/Preset.cs` and `OutputRule.cs`.
//!
//! A preset is one slice of the ring: "convert to `target` with these
//! options, and put the result there". The built-in ones live in
//! [`crate::builtin`].

use serde::{Deserialize, Serialize};

use crate::contract::ConvertOptions;
use crate::formats::{self, Format};
use crate::job::Target;
use crate::options::{ArchiveOptions, DocumentOptions, ImageOptions, MediaOptions, PdfOptions};
use crate::output::{OutputLocation, OutputRule};

/// Target id meaning "same format as the source file" (resize-only presets).
pub const SAME_AS_SOURCE: &str = "*";

/// A named conversion recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preset {
    /// Stable identifier referenced by toolbar profiles. Never shown to users.
    pub id: String,
    /// User-given name. When empty, `name_key` is shown instead.
    pub name: String,
    /// Localization key of a built-in preset's name, e.g. `preset.merge_pdf`.
    /// Ignored when `name` is set.
    pub name_key: Option<String>,
    /// Id of the output format, or [`SAME_AS_SOURCE`].
    #[serde(alias = "targetFormat")]
    pub target: String,
    pub image: ImageOptions,
    pub pdf: PdfOptions,
    pub document: DocumentOptions,
    pub media: MediaOptions,
    pub archive: ArchiveOptions,
    pub output: OutputRule,
}

impl Default for Preset {
    fn default() -> Self {
        // Filee hands out a random id here; we have no use for a dice roll in
        // core, so whoever stores a new preset gives it an id.
        Self {
            id: String::new(),
            name: String::new(),
            name_key: None,
            target: "png".to_owned(),
            image: ImageOptions::default(),
            pdf: PdfOptions::default(),
            document: DocumentOptions::default(),
            media: MediaOptions::default(),
            archive: ArchiveOptions::default(),
            output: default_output(),
        }
    }
}

/// The output rule a fresh preset starts with: Filee's default location (the
/// one from the settings) rather than [`OutputRule`]'s own same-folder default.
pub(crate) fn default_output() -> OutputRule {
    OutputRule {
        location: OutputLocation::Default,
        ..OutputRule::default()
    }
}

/// English text of the built-in name keys (Filee's `en.json`, lines 103-109).
fn english_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "preset.half_size" => "Half size",
        "preset.grayscale" => "Grayscale",
        "preset.merge_pdf" => "Merge PDF",
        "preset.split_pdf" => "Split PDF",
        "preset.long_image" => "Long image",
        "preset.extract" => "Extract",
        "preset.zip_all" => "One ZIP",
        _ => return None,
    })
}

impl Preset {
    /// The name shown to users: `name`, else the English text of `name_key`,
    /// else the target in upper case.
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        self.name_key
            .as_deref()
            .and_then(english_name)
            .map_or_else(|| self.target.to_uppercase(), str::to_owned)
    }

    /// The job target this preset converts to: [`Target::SameAsSource`] for
    /// `*`, otherwise the format the target id names. `None` when the target
    /// is not a known format.
    pub fn job_target(&self) -> Option<Target> {
        if self.target.trim() == SAME_AS_SOURCE {
            return Some(Target::SameAsSource);
        }
        formats::lookup_target(&self.target).map(|format| Target::Format(format.id))
    }

    /// The options a job runs this preset with.
    ///
    /// `quality` is only set when the preset's image quality differs from the
    /// default, so a plain `--to` preset leaves every engine on its own
    /// default exactly as v0.1 did.
    pub fn options(&self) -> ConvertOptions {
        let quality =
            (self.image.quality != ImageOptions::default().quality).then_some(self.image.quality);
        ConvertOptions {
            quality,
            image: self.image.clone(),
            pdf: self.pdf.clone(),
            document: self.document.clone(),
            media: self.media.clone(),
            archive: self.archive.clone(),
        }
    }
}

/// The preset behind `--to <format>`: the format with every option at its
/// default.
pub fn for_target(format: &Format) -> Preset {
    Preset {
        id: format!("to-{}", format.id.as_str()),
        name: format.name.to_owned(),
        target: format.id.as_str().to_owned(),
        ..Preset::default()
    }
}

/// Finds a preset by id (case-insensitive), else by display name
/// (case-insensitive). Surrounding whitespace in `key` is ignored.
pub fn find<'a>(presets: &'a [Preset], key: &str) -> Option<&'a Preset> {
    let key = key.trim();
    presets
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(key))
        .or_else(|| {
            // Display names may hold non-ASCII text, so the full Unicode fold.
            let key = key.to_lowercase();
            presets
                .iter()
                .find(|p| p.display_name().to_lowercase() == key)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;
    use crate::contract::FormatId;
    use crate::options::Resize;

    #[test]
    fn preset_find_by_id_name_and_key_text() {
        let presets = builtin::presets();
        assert_eq!(find(&presets, "TO-PNG").unwrap().id, "to-png");
        assert_eq!(find(&presets, "jpg 70%").unwrap().id, "jpg-70");
        assert_eq!(find(&presets, "merge pdf").unwrap().id, "merge-pdf");
        assert_eq!(find(&presets, "  One zip ").unwrap().id, "zip-all");
        assert!(find(&presets, "nope").is_none());
    }

    #[test]
    fn preset_id_wins_over_a_name() {
        // A user preset named "to-png" must not shadow the real to-png.
        let presets = vec![
            Preset {
                id: "mine".into(),
                name: "to-png".into(),
                ..Preset::default()
            },
            builtin::presets().remove(0),
        ];
        assert_eq!(find(&presets, "to-png").unwrap().id, "to-png");
    }

    #[test]
    fn preset_display_names() {
        let presets = builtin::presets();
        let name = |id: &str| find(&presets, id).unwrap().display_name();
        assert_eq!(name("to-png"), "PNG");
        assert_eq!(name("half-size"), "Half size");
        assert_eq!(name("grayscale"), "Grayscale");
        assert_eq!(name("merge-pdf"), "Merge PDF");
        assert_eq!(name("pdf-split"), "Split PDF");
        assert_eq!(name("pdf-long-image"), "Long image");
        assert_eq!(name("extract"), "Extract");
        assert_eq!(name("zip-all"), "One ZIP");

        let unnamed = Preset {
            target: "webp".into(),
            name_key: Some("preset.who_knows".into()),
            ..Preset::default()
        };
        assert_eq!(unnamed.display_name(), "WEBP");
    }

    #[test]
    fn preset_job_target() {
        let presets = builtin::presets();
        let target = |id: &str| find(&presets, id).unwrap().job_target();
        assert_eq!(target("half-size"), Some(Target::SameAsSource));
        assert_eq!(target("to-jpg"), Some(Target::Format(FormatId::new("jpg"))));
        assert_eq!(target("extract"), Some(Target::Format(formats::FOLDER)));
        let bogus = Preset {
            target: "nonsense".into(),
            ..Preset::default()
        };
        assert_eq!(bogus.job_target(), None);
    }

    #[test]
    fn preset_options_only_set_quality_when_it_differs() {
        let presets = builtin::presets();
        let opts = |id: &str| find(&presets, id).unwrap().options();
        assert_eq!(opts("to-png").quality, None);
        assert_eq!(opts("to-jpg").quality, None);
        assert_eq!(opts("jpg-70").quality, Some(70));
        assert_eq!(opts("to-avif").quality, Some(60));
        assert_eq!(opts("half-size").image.resize, Resize::Percent);
        assert!(opts("merge-pdf").pdf.merge_into_single);
    }

    #[test]
    fn preset_for_target_is_all_defaults() {
        let jpg = formats::find_by_id("jpg").unwrap();
        let preset = for_target(jpg);
        assert_eq!(preset.id, "to-jpg");
        assert_eq!(preset.display_name(), "JPG");
        assert_eq!(preset.job_target(), Some(Target::Format(jpg.id)));
        assert_eq!(preset.options(), ConvertOptions::default());
        assert_eq!(preset.output.location, OutputLocation::Default);
    }

    #[test]
    fn preset_json_round_trip() {
        for preset in builtin::presets() {
            let json = serde_json::to_string(&preset).unwrap();
            let back: Preset = serde_json::from_str(&json).unwrap();
            assert_eq!(back, preset);
        }
        let split = find(&builtin::presets(), "pdf-split").unwrap().clone();
        let json = serde_json::to_value(&split).unwrap();
        assert_eq!(json["nameKey"], "preset.split_pdf");
        assert_eq!(json["target"], "pdf");
        assert_eq!(json["pdf"]["splitPages"], true);
    }

    #[test]
    fn preset_missing_fields_take_defaults() {
        let preset: Preset =
            serde_json::from_str(r#"{"id":"x","targetFormat":"webp","image":{"quality":50}}"#)
                .unwrap();
        assert_eq!(preset.target, "webp");
        assert_eq!(preset.image.quality, 50);
        assert_eq!(preset.image.long_edge, 1920);
        assert_eq!(preset.output.location, OutputLocation::Default);
        assert_eq!(preset.name_key, None);
    }
}
