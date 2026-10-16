//! Toolbar profiles, the rules that pick one for a set of files, and the
//! preset library (presets plus profiles) as it lives on disk.
//! Ported from Filee's `src/Filee.Core/Profiles/ToolbarProfile.cs`, the
//! profile half of `Presets/BuiltInData.cs` (lines 164-273) and the library
//! parts of `Settings/UserDataStore.cs`.
//!
//! A profile is one ring layout: "for these extensions, show these presets,
//! clockwise from 12 o'clock". The "mixed" profiles (`is_fallback`) catch
//! whatever the normal ones don't, which on a real desktop is a lot.

use std::collections::HashSet;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::builtin;
use crate::formats::{self, Category};
use crate::preset::Preset;
use crate::settings::Store;

/// Most slices one ring can hold. Past twelve the ring stops being a ring and
/// starts being a dartboard.
pub const MAX_SLICES: usize = 12;

/// File name of the preset list inside the store dir.
pub const PRESETS_FILE: &str = "presets.json";

/// File name of the profile list inside the store dir.
pub const PROFILES_FILE: &str = "profiles.json";

/// Current `schemaVersion` of `presets.json` and `profiles.json`.
pub const LIBRARY_SCHEMA_VERSION: u32 = 1;

/// One ring layout for a family of source files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolbarProfile {
    /// Stable identifier. Never shown to users.
    pub id: String,
    /// User-given name. When empty, `name_key` is shown instead.
    pub name: String,
    /// Localization key of a built-in profile's name, e.g. `profile.images`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_key: Option<String>,
    /// Source extensions without the dot. On a normal profile: the files it
    /// is used for. On a mixed profile (`is_fallback`): the extensions a mixed
    /// selection must stay within; empty means any mixed selection.
    pub extensions: Vec<String>,
    /// A "mixed files" profile, used when the files span several profiles or
    /// match none. The name is Filee's, kept so saved files read the same.
    pub is_fallback: bool,
    /// Preset ids in ring order, clockwise from 12 o'clock.
    pub preset_ids: Vec<String>,
}

impl ToolbarProfile {
    /// Whether `extension` (with or without leading dots, any case) is one of
    /// this profile's extensions.
    pub fn matches(&self, extension: &str) -> bool {
        let extension = extension.trim_start_matches('.');
        self.extensions
            .iter()
            .any(|own| own.trim_start_matches('.').eq_ignore_ascii_case(extension))
    }
}

/// A built-in profile: a name key, extensions and presets.
fn profile(id: &str, extensions: Vec<String>, preset_ids: &[&str]) -> ToolbarProfile {
    ToolbarProfile {
        id: id.to_owned(),
        name: String::new(),
        name_key: Some(format!("profile.{id}")),
        extensions,
        is_fallback: false,
        preset_ids: preset_ids.iter().map(|&s| s.to_owned()).collect(),
    }
}

/// Every extension of the formats in `category`, in registry order. The
/// folder pseudo-format's empty extension stays out of it.
pub fn extensions_of(category: Category) -> Vec<String> {
    formats::all()
        .iter()
        .filter(|f| f.category == category)
        .flat_map(|f| f.extensions.iter())
        .filter(|e| !e.is_empty())
        .map(|&e| e.to_owned())
        .collect()
}

/// The 15 built-in profiles in Filee's order, with Filee's preset lists. The
/// mixed one goes last, where it can sulk about catching everything.
pub fn builtin_profiles() -> Vec<ToolbarProfile> {
    use Category::*;
    vec![
        profile(
            "images",
            extensions_of(Image),
            &[
                "to-png",
                "to-jpg",
                "to-webp",
                "to-pdf",
                "to-tiff",
                "to-bmp",
                "to-ico",
                "half-size",
            ],
        ),
        profile(
            "pdf",
            vec!["pdf".to_owned()],
            &[
                "to-png",
                "to-jpg",
                "pdf-long-image",
                "to-docx",
                "to-txt",
                "to-md",
                "to-epub",
                "to-hwpx",
                "to-hwp",
                "to-tiff",
                "pdf-split",
                "merge-pdf",
            ],
        ),
        profile(
            "office",
            extensions_of(Document),
            &[
                "to-pdf",
                "to-hwpx",
                "to-hwp",
                "to-docx",
                "to-odt",
                "to-txt",
                "to-html",
                "to-epub",
                "to-png",
                "merge-pdf",
            ],
        ),
        profile(
            "spreadsheets",
            extensions_of(Spreadsheet),
            &[
                "to-pdf",
                "to-xlsx",
                "to-csv",
                "to-ods",
                "to-hwpx",
                "to-hwp",
                "to-html",
                "merge-pdf",
            ],
        ),
        profile(
            "presentations",
            extensions_of(Presentation),
            &[
                "to-pdf",
                "to-png",
                "to-jpg",
                "to-pptx",
                "to-hwpx",
                "to-hwp",
                "merge-pdf",
            ],
        ),
        profile(
            "hwp",
            vec!["hwp".to_owned(), "hwpx".to_owned()],
            &[
                "to-pdf",
                "to-docx",
                "to-hwpx",
                "to-hwp",
                "to-png",
                "to-jpg",
                "to-txt",
                "to-md",
                "to-epub",
                "merge-pdf",
            ],
        ),
        profile(
            "text",
            extensions_of(Text),
            &[
                "to-pdf", "to-hwpx", "to-hwp", "to-docx", "to-html", "to-txt", "to-epub",
            ],
        ),
        profile(
            "ebooks",
            extensions_of(Ebook),
            &[
                "to-pdf", "to-epub", "to-docx", "to-txt", "to-azw3", "to-mobi", "to-hwpx",
                "to-hwp", "to-html",
            ],
        ),
        profile(
            "video",
            extensions_of(Video),
            &[
                "to-mp4", "mp4-720", "to-webm", "to-mov", "to-gif", "to-mp3", "to-mkv", "to-avi",
            ],
        ),
        profile(
            "audio",
            extensions_of(Audio),
            &[
                "to-mp3", "mp3-128", "to-m4a", "to-wav", "to-flac", "to-ogg", "to-opus", "to-aac",
            ],
        ),
        profile(
            "vector",
            extensions_of(Vector),
            &["to-png", "to-pdf", "to-svg", "to-jpg", "to-webp", "to-eps"],
        ),
        profile(
            "archives",
            extensions_of(Archive),
            &["extract", "to-zip", "to-7z", "to-tgz", "to-tar"],
        ),
        profile(
            "cad",
            extensions_of(Cad),
            &["to-pdf", "to-png", "to-svg", "to-dxf", "to-dwg"],
        ),
        profile(
            "fonts",
            extensions_of(Font),
            &["to-woff2", "to-woff", "to-ttf", "to-otf", "to-eot"],
        ),
        ToolbarProfile {
            is_fallback: true,
            ..profile(
                "mixed",
                Vec::new(),
                &["to-pdf", "merge-pdf", "zip-all", "to-png", "to-txt"],
            )
        },
    ]
}

/// Picks the profile for `files`, in Filee's order:
/// 1. collect the distinct extensions of the files (case-insensitive);
/// 2. the first normal profile that has every one of them;
/// 3. else the first mixed profile with a non-empty extension list that
///    covers every one of them;
/// 4. else [`catch_all`].
///
/// `None` only when `profiles` is empty.
pub fn select<P: AsRef<Path>>(
    profiles: &[ToolbarProfile],
    files: impl IntoIterator<Item = P>,
) -> Option<&ToolbarProfile> {
    let mut extensions: Vec<String> = Vec::new();
    for file in files {
        let ext = formats::extension_of(file);
        if !extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
            extensions.push(ext);
        }
    }

    if !extensions.is_empty() {
        let covers = |p: &ToolbarProfile| extensions.iter().all(|e| p.matches(e));
        let found = profiles
            .iter()
            .find(|p| !p.is_fallback && covers(p))
            .or_else(|| {
                profiles
                    .iter()
                    .find(|p| p.is_fallback && !p.extensions.is_empty() && covers(p))
            });
        if found.is_some() {
            return found;
        }
    }
    catch_all(profiles)
}

/// The profile for selections nobody else takes: the first mixed profile with
/// no extensions, else the first mixed profile at all (better than an
/// unrelated single-type ring), else the first profile, else nothing.
pub fn catch_all(profiles: &[ToolbarProfile]) -> Option<&ToolbarProfile> {
    profiles
        .iter()
        .find(|p| p.is_fallback && p.extensions.is_empty())
        .or_else(|| profiles.iter().find(|p| p.is_fallback))
        .or_else(|| profiles.first())
}

/// What `presets.json` holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PresetsFile {
    schema_version: u32,
    presets: Vec<Preset>,
}

impl Default for PresetsFile {
    // Missing (or broken) file means "factory settings", hence the built-ins.
    fn default() -> Self {
        Self {
            schema_version: LIBRARY_SCHEMA_VERSION,
            presets: builtin::presets(),
        }
    }
}

/// What `profiles.json` holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ProfilesFile {
    schema_version: u32,
    profiles: Vec<ToolbarProfile>,
}

impl Default for ProfilesFile {
    fn default() -> Self {
        Self {
            schema_version: LIBRARY_SCHEMA_VERSION,
            profiles: builtin_profiles(),
        }
    }
}

/// The user's presets and toolbar profiles, loaded and saved together.
#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    pub presets: Vec<Preset>,
    pub profiles: Vec<ToolbarProfile>,
}

impl Default for Library {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Library {
    /// Factory defaults: the built-in presets and profiles.
    pub fn builtin() -> Self {
        Self {
            presets: builtin::presets(),
            profiles: builtin_profiles(),
        }
    }

    /// Reads `presets.json` and `profiles.json` from `store`. A missing file
    /// gives the built-ins; a corrupt one is copied to `<file>.broken` and
    /// also gives the built-ins. The result is made consistent before it is
    /// returned.
    pub fn load(store: &Store) -> Self {
        let presets: PresetsFile = store.read_json(PRESETS_FILE);
        let profiles: ProfilesFile = store.read_json(PROFILES_FILE);
        let mut library = Self {
            presets: presets.presets,
            profiles: profiles.profiles,
        };
        library.ensure_consistency();
        library
    }

    /// Makes the library consistent, then writes both files atomically.
    pub fn save(&mut self, store: &Store) -> io::Result<()> {
        self.ensure_consistency();
        store.write_json(
            PRESETS_FILE,
            &PresetsFile {
                schema_version: LIBRARY_SCHEMA_VERSION,
                presets: self.presets.clone(),
            },
        )?;
        store.write_json(
            PROFILES_FILE,
            &ProfilesFile {
                schema_version: LIBRARY_SCHEMA_VERSION,
                profiles: self.profiles.clone(),
            },
        )
    }

    /// Filee's `EnsureConsistency`: every profile loses preset ids that point
    /// at no preset, loses duplicates (first one stays) and is capped at
    /// [`MAX_SLICES`]; and if no mixed profile is left, the built-in one comes
    /// back (with only the presets that still exist), because a drop with no
    /// ring at all is a sad drop.
    pub fn ensure_consistency(&mut self) {
        let known: HashSet<&str> = self.presets.iter().map(|p| p.id.as_str()).collect();
        for profile in &mut self.profiles {
            let mut seen = HashSet::new();
            profile.preset_ids.retain(|id| {
                known.contains(id.as_str()) && seen.insert(id.clone()) && seen.len() <= MAX_SLICES
            });
        }

        if !self.profiles.iter().any(|p| p.is_fallback) {
            if let Some(mut mixed) = builtin_profiles().into_iter().find(|p| p.is_fallback) {
                mixed.preset_ids.retain(|id| known.contains(id.as_str()));
                self.profiles.push(mixed);
            }
        }
    }

    /// The preset with this exact id.
    pub fn preset(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The profile for `files` (see [`select`]).
    pub fn select<P: AsRef<Path>>(
        &self,
        files: impl IntoIterator<Item = P>,
    ) -> Option<&ToolbarProfile> {
        select(&self.profiles, files)
    }

    /// The presets `profile` puts on the ring, in ring order. Ids with no
    /// preset are skipped.
    pub fn ring_presets(&self, profile: &ToolbarProfile) -> Vec<&Preset> {
        profile
            .preset_ids
            .iter()
            .filter_map(|id| self.preset(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn normal(id: &str, extensions: &[&str]) -> ToolbarProfile {
        ToolbarProfile {
            id: id.to_owned(),
            extensions: extensions.iter().map(|&e| e.to_owned()).collect(),
            ..ToolbarProfile::default()
        }
    }

    fn mixed(id: &str, extensions: &[&str]) -> ToolbarProfile {
        ToolbarProfile {
            is_fallback: true,
            ..normal(id, extensions)
        }
    }

    fn pick<'a>(profiles: &'a [ToolbarProfile], files: &[&str]) -> Option<&'a str> {
        select(profiles, files).map(|p| p.id.as_str())
    }

    fn ids(profiles: &[ToolbarProfile]) -> Vec<&str> {
        profiles.iter().map(|p| p.id.as_str()).collect()
    }

    // ---- ProfileSelectorTests, case by case ----

    #[test]
    fn picks_profile_matching_all_files() {
        assert_eq!(
            pick(&builtin_profiles(), &["a.jpg", "b.PNG"]),
            Some("images")
        );
    }

    #[test]
    fn built_in_profiles_take_their_files() {
        let builtin = builtin_profiles();
        let cases: &[(&str, &[&str])] = &[
            ("pdf", &["a.pdf"]),
            ("archives", &["a.tar.gz", "b.zip"]),
            ("video", &[r"C:\clips\a.MP4", r"C:\clips\b.mkv"]),
            ("text", &["notes.md", "readme.txt"]),
        ];
        for (expected, files) in cases {
            assert_eq!(pick(&builtin, files), Some(*expected), "{files:?}");
        }
    }

    #[test]
    fn mixed_files_use_the_fallback() {
        assert_eq!(
            pick(&builtin_profiles(), &["a.jpg", "b.docx"]),
            Some("mixed")
        );
    }

    #[test]
    fn unknown_files_use_the_fallback() {
        assert_eq!(pick(&builtin_profiles(), &["a.xyz"]), Some("mixed"));
    }

    #[test]
    fn no_files_use_the_fallback() {
        assert_eq!(pick(&builtin_profiles(), &[]), Some("mixed"));
    }

    #[test]
    fn folders_and_files_without_extension_use_the_fallback() {
        let profiles = [
            normal("images", &["jpg"]),
            mixed("mixed", &[]),
            mixed("jpg-and-more", &["jpg", "pdf"]),
        ];
        assert_eq!(pick(&profiles, &[r"C:\photos", "a.jpg"]), Some("mixed"));
    }

    #[test]
    fn a_mixed_profile_takes_a_selection_within_its_checked_extensions() {
        let profiles = [
            normal("images", &["jpg", "png"]),
            normal("pdf", &["pdf"]),
            mixed("mixed", &[]),
            mixed("scans", &["jpg", "png", "pdf"]),
        ];
        assert_eq!(pick(&profiles, &["a.jpg", "b.pdf"]), Some("scans"));
        assert_eq!(pick(&profiles, &["a.jpg", "b.png", "c.pdf"]), Some("scans"));
    }

    #[test]
    fn every_file_must_be_checked_on_the_mixed_profile() {
        let profiles = [
            normal("images", &["jpg"]),
            normal("pdf", &["pdf"]),
            normal("office", &["docx"]),
            mixed("mixed", &[]),
            mixed("scans", &["jpg", "pdf"]),
        ];
        assert_eq!(
            pick(&profiles, &["a.jpg", "b.pdf", "c.docx"]),
            Some("mixed")
        );
    }

    #[test]
    fn a_normal_profile_wins_over_a_mixed_one_listing_the_same_extensions() {
        let mut profiles = vec![
            mixed("scans", &["jpg", "pdf"]),
            normal("images", &["jpg"]),
            mixed("mixed", &[]),
        ];
        assert_eq!(pick(&profiles, &["a.jpg", "b.JPG"]), Some("images"));

        profiles.insert(0, normal("mine", &["jpg", "pdf"]));
        assert_eq!(pick(&profiles, &["a.jpg", "b.pdf"]), Some("mine"));
    }

    #[test]
    fn the_first_matching_mixed_profile_in_list_order_wins() {
        let mut profiles = vec![
            normal("images", &["jpg"]),
            normal("pdf", &["pdf"]),
            mixed("broad", &["jpg", "png", "pdf", "docx"]),
            mixed("narrow", &["jpg", "pdf"]),
        ];
        assert_eq!(pick(&profiles, &["a.jpg", "b.pdf"]), Some("broad"));

        profiles.reverse();
        assert_eq!(pick(&profiles, &["a.jpg", "b.pdf"]), Some("narrow"));
    }

    #[test]
    fn the_catch_all_position_does_not_matter_for_limited_profiles() {
        let profiles = [
            mixed("mixed", &[]),
            normal("images", &["jpg"]),
            normal("pdf", &["pdf"]),
            mixed("scans", &["jpg", "pdf"]),
        ];
        assert_eq!(pick(&profiles, &["a.jpg", "b.pdf"]), Some("scans"));
    }

    #[test]
    fn checked_extensions_are_compared_without_case_and_dots() {
        let profiles = [
            normal("images", &["jpg"]),
            normal("pdf", &["pdf"]),
            mixed("mixed", &[]),
            mixed("scans", &["JPG", "Pdf"]),
        ];
        assert_eq!(pick(&profiles, &["A.Jpg", "b.PDF"]), Some("scans"));
        assert!(profiles[3].matches(".jpg"));
    }

    #[test]
    fn two_part_extensions_are_matched_as_a_whole() {
        let profiles = [
            normal("archives", &["zip", "tar.gz"]),
            normal("images", &["png"]),
            mixed("mixed", &[]),
            mixed("backup", &["tar.gz", "png"]),
        ];
        assert_eq!(
            pick(&profiles, &["site.tar.gz", "logo.png"]),
            Some("backup")
        );
        // Plain .gz is not .tar.gz.
        assert_eq!(pick(&profiles, &["site.gz", "logo.png"]), Some("mixed"));
    }

    #[test]
    fn unknown_extensions_can_be_checked_on_a_mixed_profile() {
        let profiles = [
            normal("images", &["jpg"]),
            mixed("mixed", &[]),
            mixed("raw-and-sidecar", &["jpg", "xmp"]),
        ];
        assert_eq!(
            pick(&profiles, &["a.jpg", "a.xmp"]),
            Some("raw-and-sidecar")
        );
        assert_eq!(pick(&profiles, &["a.xmp"]), Some("raw-and-sidecar"));
    }

    #[test]
    fn a_normal_profile_without_extensions_is_never_picked() {
        let profiles = [
            normal("new", &[]),
            normal("images", &["jpg"]),
            mixed("mixed", &[]),
        ];
        assert_eq!(pick(&profiles, &["a.jpg"]), Some("images"));
        assert_eq!(pick(&profiles, &["a.xyz"]), Some("mixed"));
    }

    #[test]
    fn the_catch_all_is_the_first_mixed_profile_with_nothing_checked() {
        let profiles = [
            normal("images", &["jpg"]),
            mixed("scans", &["jpg", "pdf"]),
            mixed("mixed", &[]),
            mixed("second", &[]),
        ];
        assert_eq!(pick(&profiles, &["a.jpg", "b.docx"]), Some("mixed"));
        assert_eq!(catch_all(&profiles).map(|p| p.id.as_str()), Some("mixed"));
    }

    #[test]
    fn without_a_catch_all_the_first_mixed_profile_takes_the_rest() {
        let profiles = [
            normal("images", &["jpg"]),
            normal("office", &["docx"]),
            mixed("scans", &["jpg", "pdf"]),
            mixed("office-mix", &["docx", "xlsx"]),
        ];
        assert_eq!(pick(&profiles, &["a.jpg", "b.docx"]), Some("scans"));
    }

    #[test]
    fn without_any_mixed_profile_the_first_profile_is_used() {
        let profiles = [normal("images", &["jpg"]), normal("office", &["docx"])];
        assert_eq!(pick(&profiles, &["a.jpg", "b.docx"]), Some("images"));
    }

    #[test]
    fn nothing_to_pick_from_is_none() {
        assert_eq!(pick(&[], &["a.jpg"]), None);
    }

    // ---- Built-ins ----

    #[test]
    fn builtin_profiles_are_filee_s_fifteen_in_order() {
        assert_eq!(
            ids(&builtin_profiles()),
            [
                "images",
                "pdf",
                "office",
                "spreadsheets",
                "presentations",
                "hwp",
                "text",
                "ebooks",
                "video",
                "audio",
                "vector",
                "archives",
                "cad",
                "fonts",
                "mixed",
            ]
        );
        let builtin = builtin_profiles();
        let fallbacks: Vec<_> = builtin.iter().filter(|p| p.is_fallback).collect();
        assert_eq!(fallbacks.len(), 1);
        assert!(fallbacks[0].extensions.is_empty());
        assert_eq!(
            builtin[0].preset_ids,
            [
                "to-png",
                "to-jpg",
                "to-webp",
                "to-pdf",
                "to-tiff",
                "to-bmp",
                "to-ico",
                "half-size"
            ]
        );
        assert_eq!(
            fallbacks[0].preset_ids,
            ["to-pdf", "merge-pdf", "zip-all", "to-png", "to-txt"]
        );
    }

    #[test]
    fn every_builtin_profile_references_existing_presets() {
        let library = Library::builtin();
        for profile in &library.profiles {
            assert!(!profile.preset_ids.is_empty(), "{} is empty", profile.id);
            assert!(profile.preset_ids.len() <= MAX_SLICES, "{}", profile.id);
            for id in &profile.preset_ids {
                assert!(
                    library.preset(id).is_some(),
                    "profile {} names unknown preset {id}",
                    profile.id
                );
            }
            assert_eq!(
                library.ring_presets(profile).len(),
                profile.preset_ids.len()
            );
            assert_eq!(
                profile.name_key.as_deref(),
                Some(&*format!("profile.{}", profile.id))
            );
        }
        // Consistency must be a no-op on factory settings.
        let mut again = library.clone();
        again.ensure_consistency();
        assert_eq!(again, library);
    }

    #[test]
    fn every_normal_builtin_profile_has_extensions() {
        for profile in builtin_profiles().iter().filter(|p| !p.is_fallback) {
            assert!(!profile.extensions.is_empty(), "{}", profile.id);
            assert!(profile.extensions.iter().all(|e| !e.is_empty()));
        }
    }

    // ---- Consistency ----

    fn library_with(preset_ids: &[&str], profiles: Vec<ToolbarProfile>) -> Library {
        Library {
            presets: preset_ids
                .iter()
                .map(|&id| Preset {
                    id: id.to_owned(),
                    ..Preset::default()
                })
                .collect(),
            profiles,
        }
    }

    fn with_presets(mut profile: ToolbarProfile, preset_ids: &[&str]) -> ToolbarProfile {
        profile.preset_ids = preset_ids.iter().map(|&s| s.to_owned()).collect();
        profile
    }

    #[test]
    fn consistency_drops_unknown_ids_and_duplicates() {
        let mut library = library_with(
            &["a", "b", "c"],
            vec![
                with_presets(normal("p", &["jpg"]), &["a", "ghost", "b", "a", "c", "b"]),
                with_presets(mixed("mixed", &[]), &["c"]),
            ],
        );
        library.ensure_consistency();
        assert_eq!(library.profiles[0].preset_ids, ["a", "b", "c"]);
        assert_eq!(library.profiles[1].preset_ids, ["c"]);
        assert_eq!(library.profiles.len(), 2);
    }

    #[test]
    fn consistency_caps_at_twelve_after_dropping_and_deduping() {
        let known: Vec<String> = (0..15).map(|i| format!("p{i}")).collect();
        let known_refs: Vec<&str> = known.iter().map(String::as_str).collect();
        // Duplicates and ghosts up front must not use up slots.
        let mut wanted = vec!["p0", "ghost", "p0"];
        wanted.extend(&known_refs);
        let mut library = library_with(
            &known_refs,
            vec![
                with_presets(normal("big", &["jpg"]), &wanted),
                mixed("mixed", &[]),
            ],
        );
        library.ensure_consistency();
        assert_eq!(library.profiles[0].preset_ids, &known[..MAX_SLICES]);
    }

    #[test]
    fn consistency_brings_back_a_fallback_with_existing_presets_only() {
        let mut library = library_with(
            &["to-pdf", "to-png"],
            vec![with_presets(normal("images", &["jpg"]), &["to-png"])],
        );
        library.ensure_consistency();
        assert_eq!(ids(&library.profiles), ["images", "mixed"]);
        let mixed = &library.profiles[1];
        assert!(mixed.is_fallback && mixed.extensions.is_empty());
        assert_eq!(mixed.preset_ids, ["to-pdf", "to-png"]);
    }

    #[test]
    fn consistency_keeps_a_limited_mixed_profile_as_the_fallback() {
        let mut library = library_with(
            &["a"],
            vec![normal("images", &["jpg"]), mixed("scans", &["jpg", "pdf"])],
        );
        library.ensure_consistency();
        assert_eq!(ids(&library.profiles), ["images", "scans"]);
    }

    // ---- Load and save ----

    fn store(tmp: &TempDir) -> Store {
        Store::new(Some(tmp.path().to_path_buf()))
    }

    #[test]
    fn missing_files_give_the_builtins() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(Library::load(&store(&tmp)), Library::builtin());
        assert_eq!(Library::load(&Store::new(None)), Library::builtin());
    }

    #[test]
    fn load_save_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        let mut library = Library::builtin();
        library.presets.push(Preset {
            id: "mine".to_owned(),
            name: "Mine".to_owned(),
            target: "webp".to_owned(),
            ..Preset::default()
        });
        library.profiles.insert(
            0,
            ToolbarProfile {
                name: "Scans".to_owned(),
                ..with_presets(mixed("scans", &["jpg", "pdf"]), &["mine", "to-pdf"])
            },
        );
        library.save(&store).unwrap();

        let loaded = Library::load(&store);
        assert_eq!(loaded, library);
        assert_eq!(
            loaded.select(["a.jpg", "b.pdf"]).map(|p| p.id.as_str()),
            Some("scans")
        );

        let presets: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(PRESETS_FILE)).unwrap())
                .unwrap();
        assert_eq!(presets["schemaVersion"], 1);
        assert!(presets["presets"].is_array());
        let profiles: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(PROFILES_FILE)).unwrap())
                .unwrap();
        assert_eq!(profiles["schemaVersion"], 1);
        assert_eq!(profiles["profiles"][0]["id"], "scans");
        assert_eq!(profiles["profiles"][0]["isFallback"], true);
        assert_eq!(profiles["profiles"][0]["presetIds"][0], "mine");
        // A user-named profile has no name key, and nulls stay out of the file.
        assert!(profiles["profiles"][0].get("nameKey").is_none());
        assert_eq!(profiles["profiles"][1]["nameKey"], "profile.images");
    }

    #[test]
    fn save_makes_the_library_consistent_first() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        let mut library = library_with(
            &["a"],
            vec![with_presets(normal("p", &["jpg"]), &["a", "a", "ghost"])],
        );
        library.save(&store).unwrap();
        assert_eq!(library.profiles[0].preset_ids, ["a"]);
        assert!(library.profiles.iter().any(|p| p.is_fallback));
        assert_eq!(Library::load(&store), library);
    }

    #[test]
    fn corrupt_files_are_kept_as_broken_and_give_the_builtins() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(PRESETS_FILE), "{ not json").unwrap();
        fs::write(tmp.path().join(PROFILES_FILE), "[1, 2,").unwrap();
        assert_eq!(Library::load(&store(&tmp)), Library::builtin());
        assert_eq!(
            fs::read_to_string(tmp.path().join("presets.json.broken")).unwrap(),
            "{ not json"
        );
        assert_eq!(
            fs::read_to_string(tmp.path().join("profiles.json.broken")).unwrap(),
            "[1, 2,"
        );
    }

    #[test]
    fn user_presets_with_builtin_profiles_are_made_consistent_on_load() {
        let tmp = TempDir::new().unwrap();
        // Only presets.json exists, and it holds a single preset.
        fs::write(
            tmp.path().join(PRESETS_FILE),
            r#"{ "schemaVersion": 1, "presets": [ { "id": "to-pdf", "target": "pdf" } ] }"#,
        )
        .unwrap();
        let library = Library::load(&store(&tmp));
        assert_eq!(library.presets.len(), 1);
        assert_eq!(library.profiles.len(), builtin_profiles().len());
        for profile in &library.profiles {
            assert!(profile.preset_ids.iter().all(|id| id == "to-pdf"));
        }
        assert_eq!(
            library.select(["x.docx"]).map(|p| p.preset_ids.clone()),
            Some(vec!["to-pdf".to_owned()])
        );
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let profile: ToolbarProfile = serde_json::from_str(r#"{ "id": "x" }"#).unwrap();
        assert_eq!(profile, normal("x", &[]));
        let file: ProfilesFile = serde_json::from_str(r#"{ "schemaVersion": 1 }"#).unwrap();
        assert_eq!(ids(&file.profiles), ids(&builtin_profiles()));
    }
}
