//! Exporting, importing and resetting the preset and toolbar library as one
//! file. Ported from Filee's `UserDataStore.cs` (`Export`, `Import` and
//! `ResetLibrary`).
//!
//! The bundle is plain JSON:
//!
//! ```json
//! { "format": "morphring-library", "version": 2, "presets": [...], "profiles": [...] }
//! ```
//!
//! Import also takes Filee's own bundle (`{ "schemaVersion": 1, "presets":
//! [...], "profiles": [...] }`), since presets and profiles already read
//! Filee's field names. Friends who switch tools shouldn't lose their rings.

use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::preset::Preset;
use crate::profiles::{LIBRARY_SCHEMA_VERSION, Library, ToolbarProfile};
use crate::settings::Store;

/// The `format` tag of a MorphRing library bundle.
pub const BUNDLE_FORMAT: &str = "morphring-library";

/// What went wrong reading a bundle.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The text is not valid JSON, or a preset or profile in it has the
    /// wrong shape. Also used if exporting fails to serialize.
    #[error("the file is not a valid preset library: {0}")]
    Malformed(#[from] serde_json::Error),
    /// Valid JSON, but not a preset library.
    #[error("the file is not a preset library")]
    NotABundle,
    /// A `format` tag this version does not know.
    #[error("unsupported preset library format: {0}")]
    UnknownFormat(String),
}

/// How an import changed the library.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Presets and profiles whose id was new.
    pub added: usize,
    /// Presets and profiles that replaced one with the same id.
    pub replaced: usize,
}

impl ImportReport {
    /// Everything imported, which is the number Filee reports.
    pub fn total(&self) -> usize {
        self.added + self.replaced
    }
}

/// The exported shape. Borrowed, because exporting shouldn't need a clone
/// of every preset just to turn it into text.
#[derive(Serialize)]
struct BundleOut<'a> {
    format: &'static str,
    version: u32,
    presets: &'a [Preset],
    profiles: &'a [ToolbarProfile],
}

/// The imported shape, ours or Filee's. Every field is optional on paper;
/// [`import`] checks there is something library-like in it.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct BundleIn {
    presets: Vec<Preset>,
    profiles: Vec<ToolbarProfile>,
}

/// The library as pretty JSON, ready to save and share.
pub fn export(library: &Library) -> Result<String, BundleError> {
    let bundle = BundleOut {
        format: BUNDLE_FORMAT,
        version: LIBRARY_SCHEMA_VERSION,
        presets: &library.presets,
        profiles: &library.profiles,
    };
    // Presets and profiles are plain data with string keys, so this only
    // fails if serde has a very bad day. Still, a Result beats a panic.
    Ok(serde_json::to_string_pretty(&bundle)?)
}

/// Merges the bundle in `json` into `library`: an item whose id already
/// exists replaces it, a new id is added. Like Filee, a replaced item moves
/// to the end, so an imported mixed profile never takes over as the
/// catch-all from ours. Items without an id are skipped. The library is then
/// made consistent ([`Library::ensure_consistency`]).
///
/// The whole bundle is parsed before anything changes, so on an error the
/// library is exactly as it was. Saving is the caller's job.
pub fn import(library: &mut Library, json: &str) -> Result<ImportReport, BundleError> {
    let bundle = parse(json)?;
    let mut report = ImportReport::default();

    for preset in bundle.presets.into_iter().filter(|p| !p.id.is_empty()) {
        let before = library.presets.len();
        library.presets.retain(|p| p.id != preset.id);
        tally(&mut report, before != library.presets.len());
        library.presets.push(preset);
    }
    for profile in bundle.profiles.into_iter().filter(|p| !p.id.is_empty()) {
        let before = library.profiles.len();
        library.profiles.retain(|p| p.id != profile.id);
        tally(&mut report, before != library.profiles.len());
        library.profiles.push(profile);
    }

    library.ensure_consistency();
    Ok(report)
}

/// Puts the factory presets and profiles back and saves them to `store`.
/// Returns the fresh library so the caller doesn't have to load it again.
pub fn reset(store: &Store) -> io::Result<Library> {
    let mut library = Library::builtin();
    library.save(store)?;
    Ok(library)
}

fn tally(report: &mut ImportReport, replaced: bool) {
    if replaced {
        report.replaced += 1;
    } else {
        report.added += 1;
    }
}

/// Reads and checks a bundle. An object counts when it carries our format
/// tag, or (for Filee's bundles, which have no tag) a `presets` or
/// `profiles` list. Anything else, say a `settings.json` picked by mistake,
/// is refused rather than imported as nothing.
fn parse(json: &str) -> Result<BundleIn, BundleError> {
    let value: Value = serde_json::from_str(json)?;
    let Value::Object(map) = &value else {
        return Err(BundleError::NotABundle);
    };
    match map.get("format") {
        Some(Value::String(format)) if format == BUNDLE_FORMAT => {}
        Some(Value::String(format)) => return Err(BundleError::UnknownFormat(format.clone())),
        Some(_) => return Err(BundleError::NotABundle),
        None if map.contains_key("presets") || map.contains_key("profiles") => {}
        None => return Err(BundleError::NotABundle),
    }
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{PRESETS_FILE, PROFILES_FILE};

    fn custom_preset(id: &str, name: &str) -> Preset {
        Preset {
            id: id.into(),
            name: name.into(),
            ..Preset::default()
        }
    }

    fn custom_profile(id: &str, presets: &[&str]) -> ToolbarProfile {
        ToolbarProfile {
            id: id.into(),
            name: id.into(),
            extensions: vec!["xyz".into()],
            preset_ids: presets.iter().map(|s| s.to_string()).collect(),
            ..ToolbarProfile::default()
        }
    }

    #[test]
    fn export_import_round_trip() {
        let mut library = Library::builtin();
        library.presets.push(custom_preset("mine", "Mine"));
        library.profiles.push(custom_profile("p-mine", &["mine"]));

        let json = export(&library).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["format"], BUNDLE_FORMAT);
        assert_eq!(value["version"], LIBRARY_SCHEMA_VERSION);

        let mut empty = Library {
            presets: Vec::new(),
            profiles: Vec::new(),
        };
        let report = import(&mut empty, &json).unwrap();
        assert_eq!(empty, library);
        assert_eq!(report.replaced, 0);
        assert_eq!(report.added, library.presets.len() + library.profiles.len());

        // Importing your own export again changes nothing but the counters.
        let mut again = library.clone();
        let report = import(&mut again, &json).unwrap();
        assert_eq!(report.added, 0);
        assert_eq!(
            report.total(),
            library.presets.len() + library.profiles.len()
        );
        let ids = |l: &Library| {
            let mut v: Vec<String> = l.presets.iter().map(|p| p.id.clone()).collect();
            v.sort();
            v
        };
        assert_eq!(ids(&again), ids(&library));
    }

    #[test]
    fn import_replaces_same_id_and_adds_new() {
        let mut library = Library::builtin();
        library.presets.push(custom_preset("mine", "Old name"));
        let count = library.presets.len();

        let json = r#"{
            "format": "morphring-library",
            "version": 2,
            "presets": [
                { "id": "mine", "name": "New name", "target": "png" },
                { "id": "theirs", "name": "Theirs", "target": "webp" }
            ],
            "profiles": [{ "id": "p-theirs", "extensions": ["xyz"], "presetIds": ["theirs", "ghost"] }]
        }"#;
        let report = import(&mut library, json).unwrap();
        assert_eq!(
            report,
            ImportReport {
                added: 2,
                replaced: 1
            }
        );
        assert_eq!(library.presets.len(), count + 1);
        assert_eq!(library.preset("mine").unwrap().name, "New name");
        assert_eq!(library.presets.iter().filter(|p| p.id == "mine").count(), 1);
        assert!(library.preset("theirs").is_some());
        // Consistency ran: the dangling "ghost" is gone.
        let profile = library
            .profiles
            .iter()
            .find(|p| p.id == "p-theirs")
            .unwrap();
        assert_eq!(profile.preset_ids, vec!["theirs".to_string()]);
    }

    #[test]
    fn import_takes_filee_bundles() {
        let mut library = Library::builtin();
        let json = r#"{
            "schemaVersion": 1,
            "presets": [{ "id": "f1", "name": "From Filee", "targetFormat": "jpg" }],
            "profiles": []
        }"#;
        let report = import(&mut library, json).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(library.preset("f1").unwrap().target, "jpg");
    }

    #[test]
    fn bad_input_is_an_error_and_leaves_the_library_alone() {
        let mut library = Library::builtin();
        library.presets.push(custom_preset("mine", "Mine"));
        let before = library.clone();

        for bad in [
            "",
            "{ not json",
            "[1, 2, 3]",
            r#"{ "theme": "dark" }"#,
            r#"{ "format": "someone-else", "presets": [] }"#,
            // Valid JSON, but the second preset has the wrong shape: the first
            // one must not sneak in either.
            r#"{ "presets": [{ "id": "a" }, { "id": 5 }] }"#,
        ] {
            assert!(import(&mut library, bad).is_err(), "accepted {bad:?}");
            assert_eq!(library, before, "changed by {bad:?}");
        }
        assert!(matches!(
            import(&mut library, r#"{ "format": "zip" }"#),
            Err(BundleError::UnknownFormat(f)) if f == "zip"
        ));
    }

    #[test]
    fn reset_gives_the_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(Some(tmp.path().to_path_buf()));
        let mut library = Library::builtin();
        library.presets.clear();
        library.presets.push(custom_preset("mine", "Mine"));
        library.save(&store).unwrap();
        assert!(Library::load(&store).preset("mine").is_some());

        let fresh = reset(&store).unwrap();
        assert_eq!(fresh, Library::builtin());
        assert_eq!(Library::load(&store), Library::builtin());
        assert!(tmp.path().join(PRESETS_FILE).exists());
        assert!(tmp.path().join(PROFILES_FILE).exists());
    }
}
