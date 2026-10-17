//! Library migrations: add new built-in presets, profiles and slices to a
//! saved library without overwriting the user's own edits. Ports the intent
//! of Filee's `src/Filee.Core/Settings/UserDataStore.cs` (`ApplyToLibrary`,
//! lines 208-611), minus the history lesson: Filee needed seventeen schema
//! steps, we start fresh at two.
//!
//! The rules, in the order [`upgrade`] applies them:
//! 1. a built-in preset the library lacks is added next to its built-in
//!    neighbour, unless the user deleted it;
//! 2. a built-in profile the library lacks is added after its built-in
//!    neighbour with the extensions no other ring claims yet (none left means
//!    it stays out), unless the user deleted it;
//! 3. a built-in slice a built-in profile lacks is offered once, when the ring
//!    has room, unless the user took it off;
//! 4. existing presets are never edited. The one planned exception (Filee's
//!    schema 17 rotate/flip repair) gets its own step, see
//!    [`repair_resaved_builtins`].
//!
//! "Unless the user deleted it" is answered by the `removedBuiltins` list in
//! `profiles.json`. [`crate::profiles::Library::save`] fills it with every
//! built-in that is missing at save time ([`missing_builtins`]); a library
//! written before the list existed gets it reconstructed from what was
//! built in at its schema version ([`INTRODUCED`]). Either way a deleted
//! item stays deleted, which is the whole point: nobody enjoys a zombie
//! "GIF" slice crawling back after every update.

use std::collections::{BTreeSet, HashSet};

use crate::builtin;
use crate::profiles::{
    LIBRARY_SCHEMA_VERSION, Library, MAX_SLICES, ToolbarProfile, builtin_profiles,
};

/// The built-ins that arrived with one library schema version. Anything not
/// listed here has been around since version 1.
pub struct Introduced {
    /// The library schema version that brought them.
    pub version: u32,
    /// Preset ids.
    pub presets: &'static [&'static str],
    /// Profile ids.
    pub profiles: &'static [&'static str],
    /// `(profile id, preset id)` slices on existing built-in profiles. The
    /// slices of a new profile come with the profile and need no entry.
    pub slices: &'static [(&'static str, &'static str)],
}

/// What each schema version added, oldest first. A new version appends an
/// entry here and bumps [`LIBRARY_SCHEMA_VERSION`].
pub const INTRODUCED: &[Introduced] = &[Introduced {
    // v2: Filee's newer presets (tracing, codecs, subtitles, data, 3D),
    // the subtitles, data and models donuts, and the slices Filee's current
    // donuts have on top of what MorphRing shipped with in schema 1.
    version: 2,
    presets: &[
        "remove-background",
        "trace-svg",
        "trace-line-art",
        "mp4-hevc",
        "mp4-av1",
        "extract-subtitles",
        "to-alac",
        "to-kepub",
        "to-eml",
        "to-srt",
        "to-vtt",
        "to-ass",
        "to-smi",
        "to-lrc",
        "to-ttml",
        "to-json",
        "to-yaml",
        "to-toml",
        "to-xml",
        "to-vcf",
        "to-glb",
        "to-gltf",
        "to-stl",
        "to-obj",
        "to-3mf",
        "to-ply",
    ],
    profiles: &["subtitles", "data", "models"],
    slices: &[
        ("images", "trace-svg"),
        ("images", "trace-line-art"),
        ("spreadsheets", "to-json"),
        ("ebooks", "to-kepub"),
        ("video", "mp4-hevc"),
        ("video", "extract-subtitles"),
    ],
}];

/// One built-in thing the user can delete, as written to `removedBuiltins`:
/// `preset:<id>`, `profile:<id>` or `slice:<profile id>/<preset id>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item<'a> {
    Preset(&'a str),
    Profile(&'a str),
    Slice(&'a str, &'a str),
}

impl Item<'_> {
    fn key(self) -> String {
        match self {
            Item::Preset(id) => format!("preset:{id}"),
            Item::Profile(id) => format!("profile:{id}"),
            Item::Slice(profile, preset) => format!("slice:{profile}/{preset}"),
        }
    }

    /// The schema version that first had this built-in.
    fn introduced(self) -> u32 {
        INTRODUCED
            .iter()
            .find(|step| match self {
                Item::Preset(id) => step.presets.contains(&id),
                Item::Profile(id) => step.profiles.contains(&id),
                // A slice of a profile that is itself new arrived with it.
                Item::Slice(profile, preset) => {
                    step.profiles.contains(&profile) || step.slices.contains(&(profile, preset))
                }
            })
            .map_or(1, |step| step.version)
    }
}

/// Every built-in item this version knows, in built-in order.
fn builtin_items(profiles: &[ToolbarProfile]) -> Vec<String> {
    let presets = builtin::presets();
    let mut keys: Vec<String> = presets.iter().map(|p| Item::Preset(&p.id).key()).collect();
    for profile in profiles {
        keys.push(Item::Profile(&profile.id).key());
        keys.extend(
            profile
                .preset_ids
                .iter()
                .map(|id| Item::Slice(&profile.id, id).key()),
        );
    }
    keys
}

/// Whether `key` (a `removedBuiltins` entry) names a built-in of this version.
/// Entries it does not know come from a newer MorphRing and are kept as they
/// are, so a trip through an older version forgets nothing.
pub fn is_known_builtin(key: &str) -> bool {
    builtin_items(&builtin_profiles()).iter().any(|k| k == key)
}

/// The built-ins missing from `library` that were built in at schema `upto`
/// (`u32::MAX` for all of them), as `removedBuiltins` keys. A slice counts as
/// missing only while its profile is there; a deleted profile is one entry.
fn missing_since(library: &Library, upto: u32) -> BTreeSet<String> {
    let have_presets: HashSet<&str> = library.presets.iter().map(|p| p.id.as_str()).collect();
    let mut missing = BTreeSet::new();
    let mut note = |item: Item<'_>| {
        if item.introduced() <= upto {
            missing.insert(item.key());
        }
    };

    for preset in builtin::presets() {
        if !have_presets.contains(preset.id.as_str()) {
            note(Item::Preset(&preset.id));
        }
    }
    for builtin in builtin_profiles() {
        match library.profiles.iter().find(|p| p.id == builtin.id) {
            None => note(Item::Profile(&builtin.id)),
            Some(own) => {
                for id in builtin
                    .preset_ids
                    .iter()
                    .filter(|id| !own.preset_ids.contains(id))
                {
                    note(Item::Slice(&builtin.id, id));
                }
            }
        }
    }
    missing
}

/// Every built-in preset, profile and slice missing from `library`, as the
/// `removedBuiltins` keys [`crate::profiles::Library::save`] writes. Missing
/// at save time means the user deleted it (or a migration offered it and the
/// ring was full); either way it is not offered again.
pub fn missing_builtins(library: &Library) -> BTreeSet<String> {
    missing_since(library, u32::MAX)
}

/// Upgrades `library`, saved at library schema `from`, to
/// [`LIBRARY_SCHEMA_VERSION`]. `removed` is the `removedBuiltins` list read
/// with it. Returns true when anything was added or repaired. A library at
/// the current version (or a newer one) is left alone.
pub fn upgrade(library: &mut Library, from: u32, removed: &BTreeSet<String>) -> bool {
    if from >= LIBRARY_SCHEMA_VERSION {
        return false;
    }
    // Whatever existed at `from` and is gone now was deleted by the user,
    // list or no list. Schema 1 files never had the list, so this is where
    // their deletions get remembered.
    let mut removed = removed.clone();
    removed.extend(missing_since(library, from));

    let mut changed = add_presets(library, &removed);
    changed |= add_profiles(library, &removed);
    changed |= add_slices(library, &removed);
    changed |= repair_resaved_builtins(library, from);
    changed
}

/// Where to put the built-in at `index` of `order` into `current`: right
/// after its nearest earlier neighbour that `current` has, else right before
/// its nearest later one, else at the end. Built-in order survives as long
/// as the user has not shuffled everything, and when they have, the end is
/// as good a place as any.
fn insert_at<'a>(order: &[&'a str], index: usize, current: &[&'a str]) -> usize {
    let position = |id: &str| current.iter().position(|c| *c == id);
    if let Some(at) = order[..index].iter().rev().find_map(|id| position(id)) {
        return at + 1;
    }
    order[index + 1..]
        .iter()
        .find_map(|id| position(id))
        .unwrap_or(current.len())
}

/// Rule 1: missing built-in presets, each beside its built-in neighbour.
fn add_presets(library: &mut Library, removed: &BTreeSet<String>) -> bool {
    let builtins = builtin::presets();
    let order: Vec<&str> = builtins.iter().map(|p| p.id.as_str()).collect();
    let mut changed = false;
    for (index, preset) in builtins.iter().enumerate() {
        if library.preset(&preset.id).is_some() || removed.contains(&Item::Preset(&preset.id).key())
        {
            continue;
        }
        let current: Vec<&str> = library.presets.iter().map(|p| p.id.as_str()).collect();
        let at = insert_at(&order, index, &current);
        library.presets.insert(at, preset.clone());
        changed = true;
    }
    changed
}

/// Rule 2: missing built-in profiles, with the extensions nobody else claims
/// and the presets that exist. Filee's `AddData` / `AddModels` /
/// `AddCodecsAndSubtitles` all do this dance, so here it is once.
fn add_profiles(library: &mut Library, removed: &BTreeSet<String>) -> bool {
    let builtins = builtin_profiles();
    let order: Vec<&str> = builtins.iter().map(|p| p.id.as_str()).collect();
    // Extensions on a mixed profile only limit that profile; they claim nothing.
    let mut claimed: HashSet<String> = library
        .profiles
        .iter()
        .filter(|p| !p.is_fallback)
        .flat_map(|p| p.extensions.iter().map(|e| e.to_ascii_lowercase()))
        .collect();
    let mut changed = false;

    for (index, builtin) in builtins.iter().enumerate() {
        if library.profiles.iter().any(|p| p.id == builtin.id)
            || removed.contains(&Item::Profile(&builtin.id).key())
        {
            continue;
        }
        let mut profile = builtin.clone();
        if !profile.is_fallback {
            profile
                .extensions
                .retain(|e| claimed.insert(e.to_ascii_lowercase()));
            if profile.extensions.is_empty() {
                // Every file it would take already has a ring the user picked.
                continue;
            }
        }
        profile.preset_ids.retain(|id| {
            library.preset(id).is_some() && !removed.contains(&Item::Slice(&builtin.id, id).key())
        });

        let current: Vec<&str> = library.profiles.iter().map(|p| p.id.as_str()).collect();
        let mut at = insert_at(&order, index, &current);
        // A normal ring never goes after a mixed one: the selector reads them
        // in order and the mixed ones are meant to be the last resort.
        if !profile.is_fallback {
            if let Some(first_mixed) = library.profiles.iter().position(|p| p.is_fallback) {
                at = at.min(first_mixed);
            }
        }
        library.profiles.insert(at, profile);
        changed = true;
    }
    changed
}

/// Rule 3: missing built-in slices on built-in profiles, while there is room.
fn add_slices(library: &mut Library, removed: &BTreeSet<String>) -> bool {
    let known: HashSet<String> = library.presets.iter().map(|p| p.id.clone()).collect();
    let mut changed = false;
    for builtin in builtin_profiles() {
        let Some(own) = library.profiles.iter_mut().find(|p| p.id == builtin.id) else {
            continue;
        };
        let order: Vec<&str> = builtin.preset_ids.iter().map(String::as_str).collect();
        for (index, id) in order.iter().enumerate() {
            if own.preset_ids.len() >= MAX_SLICES {
                break;
            }
            if own.preset_ids.iter().any(|own_id| own_id == id)
                || !known.contains(*id)
                || removed.contains(&Item::Slice(&builtin.id, id).key())
            {
                continue;
            }
            let current: Vec<&str> = own.preset_ids.iter().map(String::as_str).collect();
            let at = insert_at(&order, index, &current);
            own.preset_ids.insert(at, (*id).to_owned());
            changed = true;
        }
    }
    changed
}

/// Rule 4, reserved: the one step allowed to edit an existing preset. Filee's
/// schema 17 gives a rotate/flip preset that an older version re-saved its
/// group kind and members, or its rotation and flip, back (PX-191 fills this
/// in together with those presets). Until then there is nothing to repair,
/// and this function is a very well-documented `false`.
fn repair_resaved_builtins(_library: &mut Library, _from: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::Preset;
    use crate::profiles::{PRESETS_FILE, PROFILES_FILE};
    use crate::settings::Store;
    use serde_json::{Value, json};
    use std::fs;
    use tempfile::TempDir;

    fn new_in_v2() -> &'static Introduced {
        INTRODUCED.iter().find(|s| s.version == 2).unwrap()
    }

    /// What a schema 1 library looked like: today's built-ins minus everything
    /// schema 2 brought.
    fn v1_library() -> Library {
        let step = new_in_v2();
        let mut library = Library::builtin();
        library
            .presets
            .retain(|p| !step.presets.contains(&p.id.as_str()));
        library
            .profiles
            .retain(|p| !step.profiles.contains(&p.id.as_str()));
        for profile in &mut library.profiles {
            let id = profile.id.clone();
            profile
                .preset_ids
                .retain(|p| !step.slices.contains(&(id.as_str(), p.as_str())));
        }
        library
    }

    fn ids<T>(items: &[T], id: impl Fn(&T) -> &str) -> Vec<String> {
        items.iter().map(|i| id(i).to_owned()).collect()
    }

    fn profile<'a>(library: &'a Library, id: &str) -> &'a ToolbarProfile {
        library.profiles.iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn introduced_items_are_real_builtins() {
        let builtin = Library::builtin();
        for step in INTRODUCED {
            assert!(step.version >= 2 && step.version <= LIBRARY_SCHEMA_VERSION);
            for id in step.presets {
                assert!(builtin.preset(id).is_some(), "preset {id}");
            }
            for id in step.profiles {
                assert!(builtin.profiles.iter().any(|p| p.id == *id), "profile {id}");
            }
            for (p, s) in step.slices {
                assert!(
                    profile(&builtin, p).preset_ids.iter().any(|x| x == s),
                    "{p}/{s}"
                );
            }
        }
        assert_eq!(v1_library().presets.len(), 68);
        assert_eq!(v1_library().profiles.len(), 15);
    }

    #[test]
    fn a_full_v1_library_upgrades_to_the_builtins() {
        let mut library = v1_library();
        assert!(upgrade(&mut library, 1, &BTreeSet::new()));
        assert_eq!(library, Library::builtin());
        assert!(missing_builtins(&library).is_empty());
    }

    #[test]
    fn current_and_newer_libraries_are_left_alone() {
        let mut library = v1_library();
        let before = library.clone();
        assert!(!upgrade(
            &mut library,
            LIBRARY_SCHEMA_VERSION,
            &BTreeSet::new()
        ));
        assert!(!upgrade(
            &mut library,
            LIBRARY_SCHEMA_VERSION + 1,
            &BTreeSet::new()
        ));
        assert_eq!(library, before);
    }

    #[test]
    fn deleted_v1_items_stay_deleted_without_a_list() {
        let mut library = v1_library();
        library.presets.retain(|p| p.id != "to-avif");
        library.profiles.retain(|p| p.id != "cad");
        let video = library
            .profiles
            .iter_mut()
            .find(|p| p.id == "video")
            .unwrap();
        video.preset_ids.retain(|id| id != "to-gif");

        upgrade(&mut library, 1, &BTreeSet::new());
        assert!(library.preset("to-avif").is_none());
        assert!(library.profiles.iter().all(|p| p.id != "cad"));
        assert!(
            !profile(&library, "video")
                .preset_ids
                .iter()
                .any(|id| id == "to-gif")
        );
        assert_eq!(
            missing_builtins(&library),
            BTreeSet::from([
                "preset:to-avif".to_owned(),
                "profile:cad".to_owned(),
                "slice:video/to-gif".to_owned(),
            ])
        );
    }

    #[test]
    fn listed_removals_of_new_items_are_respected() {
        let mut library = v1_library();
        let removed = BTreeSet::from([
            "preset:to-glb".to_owned(),
            "profile:subtitles".to_owned(),
            "slice:video/mp4-hevc".to_owned(),
        ]);
        upgrade(&mut library, 1, &removed);
        assert!(library.preset("to-glb").is_none());
        assert!(library.preset("to-stl").is_some());
        assert!(library.profiles.iter().all(|p| p.id != "subtitles"));
        let video = &profile(&library, "video").preset_ids;
        assert!(!video.iter().any(|id| id == "mp4-hevc"));
        assert!(video.iter().any(|id| id == "extract-subtitles"));
        // The models ring comes without the deleted GLB slice.
        assert_eq!(profile(&library, "models").preset_ids[0], "to-stl");
    }

    #[test]
    fn new_slices_go_next_to_their_neighbours_and_only_while_there_is_room() {
        let mut library = v1_library();
        upgrade(&mut library, 1, &BTreeSet::new());
        assert_eq!(
            profile(&library, "images").preset_ids[3..6],
            ["to-pdf", "trace-svg", "trace-line-art"]
        );

        // An images ring the user filled to eleven takes one tracing slice.
        let mut library = v1_library();
        let images = library
            .profiles
            .iter_mut()
            .find(|p| p.id == "images")
            .unwrap();
        images
            .preset_ids
            .extend(["to-gif", "to-avif", "to-heic"].map(String::from));
        assert_eq!(images.preset_ids.len(), 11);
        upgrade(&mut library, 1, &BTreeSet::new());
        let images = &profile(&library, "images").preset_ids;
        assert_eq!(images.len(), MAX_SLICES);
        assert!(images.iter().any(|id| id == "trace-svg"));
        assert!(!images.iter().any(|id| id == "trace-line-art"));
    }

    #[test]
    fn new_presets_keep_builtin_order_around_user_presets() {
        let mut library = v1_library();
        library.presets.insert(
            0,
            Preset {
                id: "mine".to_owned(),
                target: "png".to_owned(),
                ..Preset::default()
            },
        );
        upgrade(&mut library, 1, &BTreeSet::new());
        let mut expected = ids(&builtin::presets(), |p| &p.id);
        expected.insert(0, "mine".to_owned());
        assert_eq!(ids(&library.presets, |p| &p.id), expected);
    }

    #[test]
    fn new_profiles_take_only_unclaimed_extensions_and_stay_before_mixed() {
        let mut library = v1_library();
        // The user already routes subtitles through a ring of their own, and
        // put that ring after mixed for good measure.
        library.profiles.push(ToolbarProfile {
            id: "my-subs".to_owned(),
            extensions: crate::profiles::extensions_of(crate::formats::Category::Subtitle),
            preset_ids: vec!["to-txt".to_owned()],
            ..ToolbarProfile::default()
        });
        upgrade(&mut library, 1, &BTreeSet::new());
        assert!(library.profiles.iter().all(|p| p.id != "subtitles"));
        let order = ids(&library.profiles, |p| &p.id);
        let at = |id: &str| order.iter().position(|o| o == id).unwrap();
        assert_eq!(at("data"), at("fonts") + 1);
        assert_eq!(at("models"), at("data") + 1);
        assert!(at("models") < at("mixed"));
    }

    // ---- Through Library::load, from files on disk ----

    /// The spec's fixture: a schema 1 library whose half-size preset was
    /// edited and whose video ring lost its GIF slice.
    fn write_v1_fixture(dir: &std::path::Path) {
        let mut library = v1_library();
        let half = library
            .presets
            .iter_mut()
            .find(|p| p.id == "half-size")
            .unwrap();
        half.name = "Half, my way".to_owned();
        half.image.resize_percent = 40;
        half.output.pattern = "{name}_small".to_owned();
        let video = library
            .profiles
            .iter_mut()
            .find(|p| p.id == "video")
            .unwrap();
        video.preset_ids.retain(|id| id != "to-gif");

        let presets = json!({ "schemaVersion": 1, "presets": library.presets });
        let profiles = json!({ "schemaVersion": 1, "profiles": library.profiles });
        fs::write(dir.join(PRESETS_FILE), presets.to_string()).unwrap();
        fs::write(dir.join(PROFILES_FILE), profiles.to_string()).unwrap();
    }

    fn read(dir: &std::path::Path, name: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(dir.join(name)).unwrap()).unwrap()
    }

    #[test]
    fn migrate_v1_fixture_on_load() {
        let tmp = TempDir::new().unwrap();
        write_v1_fixture(tmp.path());
        let store = Store::new(Some(tmp.path().to_path_buf()));

        let library = Library::load(&store);

        // The user's edits are untouched.
        let half = library.preset("half-size").unwrap();
        assert_eq!(half.name, "Half, my way");
        assert_eq!(half.image.resize_percent, 40);
        assert_eq!(half.output.pattern, "{name}_small");
        // The deleted GIF slice stays deleted, the new video slices arrive.
        let video = &profile(&library, "video").preset_ids;
        assert!(!video.iter().any(|id| id == "to-gif"));
        assert!(video.iter().any(|id| id == "mp4-hevc"));
        assert!(video.iter().any(|id| id == "extract-subtitles"));
        // The GIF preset itself was never deleted, only its slice.
        assert!(library.preset("to-gif").is_some());
        // Every new preset and the three new donuts are there.
        for id in new_in_v2().presets {
            assert!(library.preset(id).is_some(), "{id}");
        }
        assert_eq!(library.presets.len(), builtin::presets().len());
        for id in ["subtitles", "data", "models"] {
            assert_eq!(profile(&library, id), profile(&Library::builtin(), id));
        }
        assert_eq!(
            library.select(["a.srt"]).map(|p| p.id.as_str()),
            Some("subtitles")
        );

        // Saved once, at the new version, with the deletion remembered.
        let profiles = read(tmp.path(), PROFILES_FILE);
        assert_eq!(
            read(tmp.path(), PRESETS_FILE)["schemaVersion"],
            LIBRARY_SCHEMA_VERSION
        );
        assert_eq!(profiles["schemaVersion"], LIBRARY_SCHEMA_VERSION);
        assert_eq!(profiles["removedBuiltins"], json!(["slice:video/to-gif"]));
    }

    #[test]
    fn migrate_second_load_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        write_v1_fixture(tmp.path());
        let store = Store::new(Some(tmp.path().to_path_buf()));

        let first = Library::load(&store);
        let presets = fs::read(tmp.path().join(PRESETS_FILE)).unwrap();
        let profiles = fs::read(tmp.path().join(PROFILES_FILE)).unwrap();

        let second = Library::load(&store);
        assert_eq!(second, first);
        // Nothing to upgrade, so nothing was written either.
        assert_eq!(fs::read(tmp.path().join(PRESETS_FILE)).unwrap(), presets);
        assert_eq!(fs::read(tmp.path().join(PROFILES_FILE)).unwrap(), profiles);
    }

    #[test]
    fn migrate_save_records_deleted_builtins_and_forgets_restored_ones() {
        let tmp = TempDir::new().unwrap();
        let store = Store::new(Some(tmp.path().to_path_buf()));
        let mut library = Library::builtin();
        library.presets.retain(|p| p.id != "to-heic");
        library.profiles.retain(|p| p.id != "fonts");
        let audio = library
            .profiles
            .iter_mut()
            .find(|p| p.id == "audio")
            .unwrap();
        audio.preset_ids.retain(|id| id != "to-ogg");
        library.save(&store).unwrap();
        assert_eq!(
            read(tmp.path(), PROFILES_FILE)["removedBuiltins"],
            json!(["preset:to-heic", "profile:fonts", "slice:audio/to-ogg"])
        );

        // Put the slice back: it is no longer a removal.
        let audio = library
            .profiles
            .iter_mut()
            .find(|p| p.id == "audio")
            .unwrap();
        audio.preset_ids.push("to-ogg".to_owned());
        library.save(&store).unwrap();
        assert_eq!(
            read(tmp.path(), PROFILES_FILE)["removedBuiltins"],
            json!(["preset:to-heic", "profile:fonts"])
        );

        // Factory settings remove nothing, and the key stays out of the file.
        Library::builtin().save(&store).unwrap();
        assert!(
            read(tmp.path(), PROFILES_FILE)
                .get("removedBuiltins")
                .is_none()
        );
    }

    #[test]
    fn migrate_save_keeps_removals_from_a_newer_version() {
        let tmp = TempDir::new().unwrap();
        let store = Store::new(Some(tmp.path().to_path_buf()));
        let mut profiles = read_or_builtin_profiles();
        profiles["removedBuiltins"] = json!(["preset:from-the-future"]);
        fs::write(tmp.path().join(PROFILES_FILE), profiles.to_string()).unwrap();

        Library::load(&store).save(&store).unwrap();
        assert!(!is_known_builtin("preset:from-the-future"));
        assert!(is_known_builtin("slice:video/to-gif"));
        assert_eq!(
            read(tmp.path(), PROFILES_FILE)["removedBuiltins"],
            json!(["preset:from-the-future"])
        );
    }

    fn read_or_builtin_profiles() -> Value {
        json!({ "schemaVersion": LIBRARY_SCHEMA_VERSION, "profiles": builtin_profiles() })
    }
}
