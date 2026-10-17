//! Group presets: one ring slice that stands for up to six other presets.
//! Ported from Filee 1.11's `PresetKind` (`src/Filee.Core/Presets/Preset.cs`)
//! and the group half of `UserDataStore.EnsureConsistency`.
//!
//! A group converts nothing itself. On the ring it fans its members out in a
//! small ring of their own; everywhere a conversion is picked (the CLI, watch
//! folders, the job runner) it is turned away with
//! [`PresetError::PresetIsGroup`]. Think of it as a folder that refuses to be
//! opened as a file, politely.

use std::collections::HashSet;

use thiserror::Error;

use crate::contract::FormatId;
use crate::planner::{Planner, Route};
use crate::preset::Preset;
use crate::profiles::Library;

/// Most presets one group fans out: more slices would get too narrow to hit.
pub const MAX_SUB_PRESETS: usize = 6;

/// The target of a group: no format at all. A Filee (or MorphRing) from
/// before groups reads a group as a conversion; with this target it finds no
/// route and greys the slice out instead of re-encoding files as themselves.
pub const GROUP_TARGET: &str = "group";

/// What a preset is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PresetKind {
    /// Converts files. Filee writes it as `Convert`.
    #[default]
    Conversion,
    /// Converts nothing; its slice fans out [`Preset::sub_preset_ids`].
    Group,
}

impl PresetKind {
    /// The name Filee writes into `presets.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversion => "Convert",
            Self::Group => "Group",
        }
    }

    /// Filee's lenient read: a whole name (any case) or the enum's number.
    /// Anything else, say a kind from a newer version, reads as a group, so
    /// it is never run as a conversion and the rest of the file still loads.
    pub fn from_json(value: &serde_json::Value) -> Self {
        use serde_json::Value;
        match value {
            Value::String(name) if name.eq_ignore_ascii_case("Convert") => Self::Conversion,
            // Our own spelling, in case someone hand-writes the Rust name.
            Value::String(name) if name.eq_ignore_ascii_case("Conversion") => Self::Conversion,
            Value::Number(n) if n.as_i64() == Some(0) => Self::Conversion,
            _ => Self::Group,
        }
    }
}

/// Serde glue for [`Preset::kind`], flattened into the preset: writes `kind`
/// and a computed `isGroup`, reads `kind` leniently and ignores `isGroup`
/// (it's derived, and a stale copy has no business overruling `kind`). A
/// missing `kind` is a conversion, which is what every pre-1.11 file means.
pub(crate) mod kind_fields {
    use std::fmt;

    use serde::de::{IgnoredAny, MapAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serializer};

    use super::PresetKind;

    pub fn serialize<S: Serializer>(kind: &PresetKind, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("kind", kind.as_str())?;
        map.serialize_entry("isGroup", &(*kind == PresetKind::Group))?;
        map.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PresetKind, D::Error> {
        struct KindVisitor;

        impl<'de> Visitor<'de> for KindVisitor {
            type Value = PresetKind;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a preset object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PresetKind, A::Error> {
                let mut kind = PresetKind::Conversion;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "kind" {
                        kind = PresetKind::from_json(&map.next_value::<serde_json::Value>()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(kind)
            }
        }

        deserializer.deserialize_map(KindVisitor)
    }
}

/// Why a preset can't be run as a conversion.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PresetError {
    /// The preset is a group; `members` are the ids it holds, in ring order.
    #[error("the preset is a group, not a conversion")]
    PresetIsGroup { members: Vec<String> },
    /// The target id names no known format.
    #[error("unknown target format '{0}'")]
    UnknownTarget(String),
    /// No chain of engines turns `from` into `to`.
    #[error("no route from {from} to {to}")]
    NoRoute { from: FormatId, to: FormatId },
}

/// Plans `preset` for a `from` source: groups are turned away before the
/// planner ever sees them, everything else is planned as usual.
pub fn plan(planner: &Planner, from: FormatId, preset: &Preset) -> Result<Route, PresetError> {
    let to = preset.conversion_target()?.for_source(from);
    planner
        .plan(from, to)
        .ok_or(PresetError::NoRoute { from, to })
}

/// The presets `group` fans out, in ring order. Ids with no conversion
/// behind them are skipped (a consistent library has none of those).
pub fn members<'a>(library: &'a Library, group: &Preset) -> Vec<&'a Preset> {
    group
        .sub_preset_ids
        .iter()
        .filter_map(|id| library.preset(id).filter(|p| !p.is_group()))
        .collect()
}

/// The group half of Filee's `EnsureConsistency`:
/// - a conversion with the literal target `group` becomes a group again (an
///   older version saved it and dropped `kind`), an empty one;
/// - each group keeps only conversions that exist, each once, at most
///   [`MAX_SUB_PRESETS`] of them (no groups inside groups, this isn't a
///   matryoshka shop);
/// - a group's target is forced back to [`GROUP_TARGET`].
pub fn ensure_consistency(library: &mut Library) {
    for preset in &mut library.presets {
        if !preset.is_group() && preset.target == GROUP_TARGET {
            preset.kind = PresetKind::Group;
        }
    }

    let conversions: HashSet<String> = library
        .presets
        .iter()
        .filter(|p| !p.is_group())
        .map(|p| p.id.clone())
        .collect();
    for group in library.presets.iter_mut().filter(|p| p.is_group()) {
        let mut seen = HashSet::new();
        group
            .sub_preset_ids
            .retain(|id| conversions.contains(id) && seen.insert(id.clone()));
        group.sub_preset_ids.truncate(MAX_SUB_PRESETS);
        group.target = GROUP_TARGET.to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::Target;
    use crate::settings::Store;
    use crate::testing::FakeConverter;

    /// A Filee 1.11 `presets.json` (Filee writes a bare array), trimmed to
    /// the fields that matter here: two conversions and two groups, one of
    /// them with a kind from the future.
    const FILEE_PRESETS: &str = r#"[
      {"id":"rotate-left","name":"","nameKey":"preset.rotate_left","targetFormat":"*",
       "image":{"quality":95},"kind":"Convert","subPresetIds":[]},
      {"id":"rotate-right","name":"","nameKey":"preset.rotate_right","targetFormat":"*",
       "image":{"quality":95},"kind":"Convert","subPresetIds":[]},
      {"id":"rotate","name":"Rotate","targetFormat":"group","kind":"Group",
       "subPresetIds":["rotate-left","rotate-right"]},
      {"id":"future","name":"Future","targetFormat":"group","kind":"Chain",
       "subPresetIds":["rotate-left"]}
    ]"#;

    fn conversion(id: &str) -> Preset {
        Preset {
            id: id.to_owned(),
            name: id.to_owned(),
            target: "png".to_owned(),
            ..Preset::default()
        }
    }

    fn group_of(id: &str, members: &[&str]) -> Preset {
        Preset {
            id: id.to_owned(),
            name: id.to_owned(),
            target: GROUP_TARGET.to_owned(),
            kind: PresetKind::Group,
            sub_preset_ids: members.iter().map(|&m| m.to_owned()).collect(),
            ..Preset::default()
        }
    }

    fn library(presets: Vec<Preset>) -> Library {
        Library {
            presets,
            profiles: Vec::new(),
        }
    }

    #[test]
    fn group_reads_and_round_trips_a_filee_presets_file() {
        let presets: Vec<Preset> = serde_json::from_str(FILEE_PRESETS).unwrap();
        assert_eq!(presets.len(), 4);
        assert!(!presets[0].is_group());
        assert_eq!(presets[0].target, "*");
        let rotate = &presets[2];
        assert!(rotate.is_group());
        assert_eq!(rotate.sub_preset_ids, ["rotate-left", "rotate-right"]);
        assert_eq!(rotate.target, GROUP_TARGET);
        // An unknown kind is never run as a conversion.
        assert!(presets[3].is_group());

        let json = serde_json::to_value(&presets).unwrap();
        assert_eq!(json[0]["kind"], "Convert");
        assert_eq!(json[0]["isGroup"], false);
        assert_eq!(json[2]["kind"], "Group");
        assert_eq!(json[2]["isGroup"], true);
        assert_eq!(json[2]["subPresetIds"][1], "rotate-right");
        let back: Vec<Preset> = serde_json::from_value(json).unwrap();
        assert_eq!(back, presets);
    }

    #[test]
    fn group_kind_is_read_leniently() {
        let kind = |json: &str| {
            serde_json::from_str::<Preset>(&format!(r#"{{"id":"x","kind":{json}}}"#))
                .unwrap()
                .kind
        };
        assert_eq!(kind(r#""group""#), PresetKind::Group);
        assert_eq!(kind(r#""GROUP""#), PresetKind::Group);
        assert_eq!(kind(r#""convert""#), PresetKind::Conversion);
        assert_eq!(kind(r#""Conversion""#), PresetKind::Conversion);
        assert_eq!(kind("0"), PresetKind::Conversion);
        assert_eq!(kind("1"), PresetKind::Group);
        // Flags-style or numeric strings are not whole names.
        assert_eq!(kind(r#""Convert, Group""#), PresetKind::Group);
        assert_eq!(kind(r#""0""#), PresetKind::Group);
        assert_eq!(kind("7"), PresetKind::Group);
        // No kind at all: every file from before groups.
        let old: Preset = serde_json::from_str(r#"{"id":"x","target":"webp"}"#).unwrap();
        assert_eq!(old.kind, PresetKind::Conversion);
        assert!(old.sub_preset_ids.is_empty());
    }

    #[test]
    fn group_is_group_is_written_but_never_read() {
        let lying: Preset =
            serde_json::from_str(r#"{"id":"x","kind":"Convert","isGroup":true}"#).unwrap();
        assert!(!lying.is_group());
        // Members alone don't make a group either.
        let members: Preset =
            serde_json::from_str(r#"{"id":"x","subPresetIds":["to-png"]}"#).unwrap();
        assert!(!members.is_group());
    }

    #[test]
    fn group_deleting_a_member_leaves_the_group_consistent() {
        let mut lib = library(vec![
            conversion("a"),
            conversion("b"),
            conversion("c"),
            group_of("g", &["a", "b", "c"]),
        ]);
        lib.presets.retain(|p| p.id != "b");
        ensure_consistency(&mut lib);
        let g = lib.preset("g").unwrap();
        assert_eq!(g.sub_preset_ids, ["a", "c"]);
        assert_eq!(
            members(&lib, g).iter().map(|p| &p.id).collect::<Vec<_>>(),
            ["a", "c"]
        );
    }

    #[test]
    fn group_drops_duplicates_groups_and_strangers() {
        let mut lib = library(vec![
            conversion("a"),
            group_of("inner", &["a"]),
            group_of("g", &["a", "ghost", "a", "inner"]),
        ]);
        ensure_consistency(&mut lib);
        assert_eq!(lib.preset("g").unwrap().sub_preset_ids, ["a"]);
    }

    #[test]
    fn group_with_seven_members_loads_as_six() {
        let ids = ["a", "b", "c", "d", "e", "f", "g"];
        let mut presets: Vec<Preset> = ids.iter().map(|id| conversion(id)).collect();
        presets.push(group_of("big", &ids));
        let mut lib = library(presets);
        ensure_consistency(&mut lib);
        assert_eq!(
            lib.preset("big").unwrap().sub_preset_ids,
            ["a", "b", "c", "d", "e", "f"]
        );
    }

    #[test]
    fn group_with_a_real_target_gets_group_back() {
        let mut g = group_of("g", &["a"]);
        g.target = "png".to_owned();
        let mut lib = library(vec![conversion("a"), g]);
        ensure_consistency(&mut lib);
        let g = lib.preset("g").unwrap();
        assert!(g.is_group());
        assert_eq!(g.target, GROUP_TARGET);
    }

    #[test]
    fn group_saved_by_an_older_version_is_still_a_group() {
        // An older version drops `kind` and `subPresetIds`, leaving a
        // "conversion" to the format called group. It comes back empty.
        let old: Preset =
            serde_json::from_str(r#"{"id":"old","name":"Rotate","targetFormat":"group"}"#).unwrap();
        assert!(!old.is_group());
        let mut lib = library(vec![conversion("a"), old]);
        ensure_consistency(&mut lib);
        let old = lib.preset("old").unwrap();
        assert!(old.is_group());
        assert!(old.sub_preset_ids.is_empty());
        assert!(crate::preset::find_conversion(&lib.presets, "old").is_none());
        assert!(crate::preset::find(&lib.presets, "old").is_some());
    }

    #[test]
    fn group_planning_fails_with_preset_is_group() {
        let engines = [FakeConverter::new("fake").edge("jpg", "png", 1)];
        let refs: Vec<&dyn crate::Converter> =
            engines.iter().map(|e| e as &dyn crate::Converter).collect();
        let planner = Planner::new(&refs, true);
        let jpg = FormatId::new("jpg");

        let g = group_of("g", &["a", "b"]);
        assert_eq!(
            plan(&planner, jpg, &g).unwrap_err(),
            PresetError::PresetIsGroup {
                members: vec!["a".into(), "b".into()]
            }
        );
        assert_eq!(
            g.conversion_target().unwrap_err(),
            PresetError::PresetIsGroup {
                members: vec!["a".into(), "b".into()]
            }
        );
        assert_eq!(g.job_target(), None);

        // The ordinary preset next door still plans.
        let png = conversion("a");
        assert!(plan(&planner, jpg, &png).is_ok());
        assert_eq!(
            png.conversion_target(),
            Ok(Target::Format(FormatId::new("png")))
        );
        let bogus = Preset {
            target: "nonsense".into(),
            ..Preset::default()
        };
        assert_eq!(
            plan(&planner, jpg, &bogus).unwrap_err(),
            PresetError::UnknownTarget("nonsense".into())
        );
    }

    #[test]
    fn group_library_load_keeps_groups_consistent() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::new(Some(dir.path().to_path_buf()));
        let mut lib = Library::builtin();
        lib.presets.push(group_of("mine", &["to-png", "to-jpg"]));
        lib.save(&store).unwrap();

        // Someone deletes a member by hand and gives the group a target.
        let path = dir.path().join("presets.json");
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let presets = json["presets"].as_array_mut().unwrap();
        presets.retain(|p| p["id"] != "to-jpg");
        let mine = presets.iter_mut().find(|p| p["id"] == "mine").unwrap();
        mine["target"] = "png".into();
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();

        let loaded = Library::load(&store);
        let mine = loaded.preset("mine").unwrap();
        assert!(mine.is_group());
        assert_eq!(mine.target, GROUP_TARGET);
        assert_eq!(mine.sub_preset_ids, ["to-png"]);
    }
}
