//! Ring slice suggestions: which presets the ring offers for a set of dropped
//! files, and why a slice is greyed out.
//!
//! Filee's ring shows the presets of the toolbar profile picked for the files
//! (`src/Filee.App/ViewModels/RadialViewModel.cs`, `Load`) and greys out the
//! ones that can't run (`src/Filee.App/Services/PresetAvailability.cs`). This
//! is that, minus the WPF. The v0.1 favourites lists (PLAN.md D2) retired
//! here; they had a good run as a stand-in for real presets.
//!
//! Nothing here plans routes itself: the caller passes, in a [`ReachCtx`], a
//! function that says, for one source and target, whether a route exists with
//! the installed engines, only with a missing one, or not at all, plus whether
//! any installed engine can merge. The app answers the first with two planners
//! (installed engines, then all engines), as D10 describes.

use std::fmt;
use std::path::Path;

use crate::contract::FormatId;
use crate::error::ErrorKey;
use crate::formats::{self, Format};
use crate::preset::{Preset, SAME_AS_SOURCE};
use crate::profiles::Library;

/// Most slices one ring shows (Filee's `ToolbarProfile.MaxSlices`).
pub use crate::profiles::MAX_SLICES;

/// The format a merge preset produces, and the only one it merges into.
const PDF: FormatId = FormatId::new("pdf");

/// What the route planner says about converting one source format to a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// A route exists with the installed engines.
    Ready,
    /// A route exists only through an engine that is not installed.
    NeedsEngine {
        /// The engine's display name, e.g. `LibreOffice`.
        engine: String,
        /// How to install it, e.g. `pacman -S libreoffice-fresh`.
        hint: String,
    },
    /// No route exists even with every engine installed.
    NoRoute,
}

/// What [`suggest`] needs to know about the engines.
pub struct ReachCtx<R: Fn(FormatId, FormatId) -> Reach> {
    /// Whether one source format can be converted to a target.
    pub reach: R,
    /// Whether an installed engine has a merge mode. Callers compute it as
    /// `engines.iter().any(|e| e.status().is_available() && e.merger().is_some())`.
    pub can_merge: bool,
}

/// Why a slice is disabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisabledReason {
    /// One of the stable error keys plus, for a missing engine, a detail
    /// naming the program and how to install it (PLAN.md D10).
    Error {
        key: ErrorKey,
        /// For `error.engine_missing`: `needs LibreOffice: pacman -S libreoffice-fresh`.
        detail: Option<String>,
    },
    /// The preset uses a mode MorphRing does not run yet: one archive for
    /// every file, a multi-page TIFF, or a PDF as one long image.
    NotYetSupported,
    /// A merge preset with fewer than two files.
    NeedsSeveralFiles,
    /// A merge preset, but no installed engine can merge.
    NoMerger,
}

impl DisabledReason {
    /// A reason made of an error key and an optional detail.
    pub fn error(key: ErrorKey, detail: Option<String>) -> Self {
        Self::Error { key, detail }
    }

    /// The stable key, e.g. `error.no_route` or `slice.not_yet_supported`.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Error { key, .. } => key.as_str(),
            Self::NotYetSupported => "slice.not_yet_supported",
            Self::NeedsSeveralFiles => "slice.needs_several_files",
            Self::NoMerger => "slice.no_merger",
        }
    }

    /// The short English message, as the CLI report shows it.
    pub fn message(&self) -> &'static str {
        match self {
            Self::Error { key, .. } => match key {
                ErrorKey::UnsupportedSource => "This file type isn't supported",
                ErrorKey::NoRoute => "No engine can do this conversion",
                ErrorKey::EngineMissing => "A required program is not installed",
                ErrorKey::ConversionFailed => "The engine reported an error",
            },
            Self::NotYetSupported => "This preset is not supported yet",
            Self::NeedsSeveralFiles => "Select two or more files to merge",
            Self::NoMerger => "No installed program can merge files",
        }
    }

    /// The detail, when there is one.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Error { detail, .. } => detail.as_deref(),
            _ => None,
        }
    }
}

impl fmt::Display for DisabledReason {
    /// The message, followed by the detail in parentheses when there is one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.detail() {
            Some(detail) => write!(f, "{} ({detail})", self.message()),
            None => f.write_str(self.message()),
        }
    }
}

/// One slice of the ring: a preset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    /// Id of the preset the slice runs.
    pub preset_id: String,
    /// The label drawn on the slice: the preset's display name, e.g. `PNG`
    /// or `Half size`.
    pub label: String,
    /// The preset's target: a format id, `*` (same as the source) or
    /// `folder`.
    pub target: String,
    /// `None` when the slice can be used; otherwise why it can't.
    pub disabled: Option<DisabledReason>,
}

impl Slice {
    /// Whether the slice can be clicked.
    pub fn is_enabled(&self) -> bool {
        self.disabled.is_none()
    }
}

/// What the ring shows for a set of files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingSuggestion {
    /// Id of the toolbar profile that was used, if the library has any.
    pub profile: Option<String>,
    /// Centre text: `3 × JPG` when all recognised files share a format,
    /// otherwise `3 files`.
    pub title: String,
    /// At most [`MAX_SLICES`] slices, in ring order.
    pub slices: Vec<Slice>,
}

/// Suggests the ring's slices for `files`: the presets of the profile
/// [`Library::select`] picks, in order, at most [`MAX_SLICES`] of them.
///
/// A slice is disabled, checked in this order:
/// - [`DisabledReason::NotYetSupported`] for a mode v0.2 can't run;
/// - for a merge preset (`pdf.merge_into_single` to PDF):
///   [`DisabledReason::NeedsSeveralFiles`] with fewer than two files, else
///   [`DisabledReason::NoMerger`] without `ctx.can_merge`;
/// - `error.unsupported_source` when no file has a known type;
/// - `error.no_route` when the target is not a writable format;
/// - otherwise it is enabled when at least one file's format is `Ready` for
///   the target (`*` meaning that file's own format; a PDF counts as ready
///   for a merge), and else carries `error.engine_missing` (naming the first
///   missing engine found, in file order) or `error.no_route`.
pub fn suggest<P, R>(files: &[P], library: &Library, ctx: &ReachCtx<R>) -> RingSuggestion
where
    P: AsRef<Path>,
    R: Fn(FormatId, FormatId) -> Reach,
{
    let profile = library.select(files);
    let sources = distinct_formats(files);
    let title = match sources.as_slice() {
        [only] => format!("{} × {}", files.len(), only.name),
        _ => format!("{} files", files.len()),
    };

    // `ensure_consistency` already caps a loaded profile, but a library built
    // by hand has signed no such contract, so the cap is enforced here too.
    let slices = profile
        .map(|p| library.ring_presets(p))
        .unwrap_or_default()
        .into_iter()
        .take(MAX_SLICES)
        .map(|preset| Slice {
            preset_id: preset.id.clone(),
            label: preset.display_name(),
            target: target_text(preset),
            disabled: availability(preset, &sources, files.len(), ctx),
        })
        .collect();

    RingSuggestion {
        profile: profile.map(|p| p.id.clone()),
        title,
        slices,
    }
}

/// Recognised formats of the files, first appearance first.
fn distinct_formats<P: AsRef<Path>>(files: &[P]) -> Vec<&'static Format> {
    let mut found: Vec<&'static Format> = Vec::new();
    for format in files.iter().filter_map(formats::detect) {
        if !found.iter().any(|f| f.id == format.id) {
            found.push(format);
        }
    }
    found
}

/// The preset's target as the ring reports it: `*`, a canonical format id,
/// or (for a target nobody recognises) the text as written.
fn target_text(preset: &Preset) -> String {
    let target = preset.target.trim();
    if target == SAME_AS_SOURCE {
        return SAME_AS_SOURCE.to_owned();
    }
    formats::lookup_target(target).map_or_else(|| target.to_owned(), |f| f.id.as_str().to_owned())
}

/// Modes that exist in the preset model but not yet in any v0.2 engine.
/// Multi-page TIFF stays ring-only: the CLI does it (job/tiff.rs), the ring
/// doesn't yet, so it can't live in [`Preset::unsupported_mode`].
fn not_yet_supported(preset: &Preset) -> bool {
    preset.unsupported_mode().is_some() || preset.image.multi_page_tiff
}

/// Filee's merge test: merge mode on, and the target is PDF.
fn is_merge(preset: &Preset) -> bool {
    preset.pdf.merge_into_single
        && formats::lookup_target(&preset.target).is_some_and(|f| f.id == PDF)
}

/// Filee's `PresetAvailability.Check`: `None` if any source works.
fn availability<R>(
    preset: &Preset,
    sources: &[&Format],
    file_count: usize,
    ctx: &ReachCtx<R>,
) -> Option<DisabledReason>
where
    R: Fn(FormatId, FormatId) -> Reach,
{
    if not_yet_supported(preset) {
        return Some(DisabledReason::NotYetSupported);
    }

    let merge = is_merge(preset);
    if merge {
        // Merging one file is a very elaborate way to copy it.
        if file_count < 2 {
            return Some(DisabledReason::NeedsSeveralFiles);
        }
        if !ctx.can_merge {
            return Some(DisabledReason::NoMerger);
        }
    }

    if sources.is_empty() {
        return Some(DisabledReason::error(ErrorKey::UnsupportedSource, None));
    }

    // `None` is "same as the source", resolved per file below.
    let target = if preset.target.trim() == SAME_AS_SOURCE {
        None
    } else {
        match formats::lookup_target(&preset.target).filter(|f| f.writable) {
            Some(format) => Some(format.id),
            None => return Some(DisabledReason::error(ErrorKey::NoRoute, None)),
        }
    };

    let mut missing = None;
    for source in sources {
        // A PDF going into a merge is already where it needs to be.
        if merge && source.id == PDF {
            return None;
        }
        match (ctx.reach)(source.id, target.unwrap_or(source.id)) {
            Reach::Ready => return None,
            Reach::NeedsEngine { engine, hint } => {
                missing.get_or_insert_with(|| format!("needs {engine}: {hint}"));
            }
            Reach::NoRoute => {}
        }
    }

    Some(match missing {
        Some(detail) => DisabledReason::error(ErrorKey::EngineMissing, Some(detail)),
        None => DisabledReason::error(ErrorKey::NoRoute, None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset;
    use crate::profiles::ToolbarProfile;

    /// Every pair converts.
    fn all_ready(_: FormatId, _: FormatId) -> Reach {
        Reach::Ready
    }

    fn ready() -> ReachCtx<fn(FormatId, FormatId) -> Reach> {
        ReachCtx {
            reach: all_ready,
            can_merge: true,
        }
    }

    fn ctx<R: Fn(FormatId, FormatId) -> Reach>(reach: R) -> ReachCtx<R> {
        ReachCtx {
            reach,
            can_merge: true,
        }
    }

    fn ids(ring: &RingSuggestion) -> Vec<&str> {
        ring.slices.iter().map(|s| s.preset_id.as_str()).collect()
    }

    fn by_id<'a>(ring: &'a RingSuggestion, id: &str) -> &'a Slice {
        ring.slices
            .iter()
            .find(|s| s.preset_id == id)
            .unwrap_or_else(|| panic!("no {id} slice in {ring:?}"))
    }

    fn builtin_ring(files: &[&str]) -> RingSuggestion {
        suggest(files, &Library::builtin(), &ready())
    }

    /// A library of `presets` with one catch-all profile showing all of them.
    fn library_of(presets: Vec<Preset>) -> Library {
        let profile = ToolbarProfile {
            id: "custom".into(),
            is_fallback: true,
            preset_ids: presets.iter().map(|p| p.id.clone()).collect(),
            ..ToolbarProfile::default()
        };
        Library {
            presets,
            profiles: vec![profile],
        }
    }

    fn plain(id: &str, target: &str) -> Preset {
        Preset {
            id: id.into(),
            target: target.into(),
            ..Preset::default()
        }
    }

    fn error(key: ErrorKey) -> Option<DisabledReason> {
        Some(DisabledReason::error(key, None))
    }

    #[test]
    fn slices_are_the_profile_presets_in_order() {
        let ring = builtin_ring(&["a.png", "b.png"]);
        assert_eq!(ring.profile.as_deref(), Some("images"));
        assert_eq!(
            ids(&ring),
            [
                "to-png",
                "to-jpg",
                "to-webp",
                "to-pdf",
                "trace-svg",
                "trace-line-art",
                "to-tiff",
                "to-bmp",
                "to-ico",
                "half-size"
            ]
        );
        assert!(ring.slices.iter().all(Slice::is_enabled));
        let jpg = by_id(&ring, "to-jpg");
        assert_eq!((jpg.label.as_str(), jpg.target.as_str()), ("JPG", "jpg"));
    }

    #[test]
    fn profile_is_picked_by_family() {
        let profile = |files: &[&str]| builtin_ring(files).profile;
        assert_eq!(profile(&["a.png", "b.JPG"]).as_deref(), Some("images"));
        assert_eq!(profile(&["report.pdf"]).as_deref(), Some("pdf"));
        assert_eq!(profile(&["letter.docx"]).as_deref(), Some("office"));
        assert_eq!(profile(&["clip.mp4", "b.mkv"]).as_deref(), Some("video"));
        assert_eq!(profile(&["song.flac"]).as_deref(), Some("audio"));
        assert_eq!(profile(&["backup.tar.gz"]).as_deref(), Some("archives"));
        assert_eq!(profile(&["a.png", "b.mp3"]).as_deref(), Some("mixed"));
        assert_eq!(profile(&["README"]).as_deref(), Some("mixed"));
    }

    #[test]
    fn title_counts_files_and_names_a_shared_format() {
        assert_eq!(builtin_ring(&["a.jpg", "b.jpeg", "c.JPG"]).title, "3 × JPG");
        assert_eq!(builtin_ring(&["a.jpg", "b.png"]).title, "2 files");
        // Unknown files still count, like Filee's files.Count.
        assert_eq!(builtin_ring(&["a.jpg", "b.zzz"]).title, "2 × JPG");
        assert_eq!(builtin_ring(&["b.zzz"]).title, "1 files");
    }

    #[test]
    fn keyed_presets_use_their_display_name() {
        let ring = builtin_ring(&["backup.zip"]);
        assert_eq!(
            ids(&ring),
            ["extract", "to-zip", "to-7z", "to-tgz", "to-tar"]
        );
        let extract = by_id(&ring, "extract");
        assert_eq!(
            (extract.label.as_str(), extract.target.as_str()),
            ("Extract", "folder")
        );
        assert_eq!(by_id(&ring, "to-tgz").label, "TAR.GZ");
    }

    #[test]
    fn at_most_twelve_slices() {
        let presets: Vec<Preset> = ["jpg", "webp", "pdf", "tiff", "bmp", "ico", "gif"]
            .iter()
            .flat_map(|t| [plain(&format!("a-{t}"), t), plain(&format!("b-{t}"), t)])
            .collect();
        assert!(presets.len() > MAX_SLICES);
        let ring = suggest(&["a.png"], &library_of(presets.clone()), &ready());
        assert_eq!(ring.slices.len(), MAX_SLICES);
        let expected: Vec<&str> = presets[..MAX_SLICES]
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids(&ring), expected);
    }

    #[test]
    fn missing_presets_are_skipped_and_no_profiles_mean_an_empty_ring() {
        let mut library = library_of(vec![plain("to-jpg", "jpg")]);
        library.profiles[0].preset_ids.insert(0, "ghost".into());
        assert_eq!(ids(&suggest(&["a.png"], &library, &ready())), ["to-jpg"]);

        library.profiles.clear();
        let ring = suggest(&["a.png"], &library, &ready());
        assert_eq!(ring.profile, None);
        assert!(ring.slices.is_empty());
        assert_eq!(ring.title, "1 × PNG");
    }

    #[test]
    fn same_as_source_resolves_to_each_file_format() {
        // Only PNGs can be resized here; the JPG in the pile doesn't spoil it.
        let png_only = |from: FormatId, to: FormatId| {
            if from == to && from.as_str() == "png" {
                Reach::Ready
            } else {
                Reach::NoRoute
            }
        };
        let ring = suggest(&["a.jpg", "b.png"], &Library::builtin(), &ctx(png_only));
        let half = by_id(&ring, "half-size");
        assert_eq!(half.target, "*");
        assert_eq!(half.label, "Half size");
        assert!(half.is_enabled());

        let ring = suggest(&["a.jpg"], &Library::builtin(), &ctx(png_only));
        assert_eq!(by_id(&ring, "half-size").disabled, error(ErrorKey::NoRoute));
    }

    #[test]
    fn modes_not_in_v02_are_not_yet_supported() {
        let mut tiff = plain("multi-tiff", "tiff");
        tiff.image.multi_page_tiff = true;
        let mut library = library_of(vec![tiff]);
        let zip_all = Library::builtin().preset("zip-all").cloned().unwrap();
        let long = Library::builtin()
            .preset("pdf-long-image")
            .cloned()
            .unwrap();
        for preset in [zip_all, long] {
            library.profiles[0].preset_ids.push(preset.id.clone());
            library.presets.push(preset);
        }

        // Even unknown files get this reason: the mode is the problem, not them.
        for files in [&["a.pdf", "b.png"][..], &["x.zzz"]] {
            let ring = suggest(files, &library, &ready());
            assert_eq!(ids(&ring), ["multi-tiff", "zip-all", "pdf-long-image"]);
            for slice in &ring.slices {
                let reason = slice.disabled.as_ref().unwrap();
                assert_eq!(reason, &DisabledReason::NotYetSupported);
                assert_eq!(reason.key(), "slice.not_yet_supported");
                assert_eq!(reason.to_string(), "This preset is not supported yet");
            }
        }
    }

    #[test]
    fn modes_no_engine_implements_are_not_yet_supported() {
        // The ring used to offer these as if they worked. The CLI refuses
        // them with the same verdict, so the two stop disagreeing.
        // mp4-hevc sits on the default video ring, so check it right there.
        let ring = builtin_ring(&["a.mp4"]);
        assert_eq!(
            by_id(&ring, "mp4-hevc").disabled,
            Some(DisabledReason::NotYetSupported)
        );

        let builtin = Library::builtin();
        let mut library = library_of(Vec::new());
        for id in ["remove-background", "to-alac", "mp4-av1"] {
            let preset = builtin.preset(id).cloned().unwrap();
            library.profiles[0].preset_ids.push(preset.id.clone());
            library.presets.push(preset);
        }
        let ring = suggest(&["a.png", "b.wav", "c.mp4"], &library, &ready());
        assert_eq!(ids(&ring), ["remove-background", "to-alac", "mp4-av1"]);
        for slice in &ring.slices {
            assert_eq!(
                slice.disabled,
                Some(DisabledReason::NotYetSupported),
                "{}",
                slice.preset_id
            );
        }
    }

    #[test]
    fn merge_needs_two_files_and_a_merger() {
        let docs = ["a.docx", "b.docx"];
        let ring = suggest(&docs, &Library::builtin(), &ready());
        assert!(by_id(&ring, "merge-pdf").is_enabled());

        let ring = suggest(&["a.docx"], &Library::builtin(), &ready());
        let merge = by_id(&ring, "merge-pdf");
        assert_eq!(merge.disabled, Some(DisabledReason::NeedsSeveralFiles));
        assert_eq!(merge.label, "Merge PDF");
        assert_eq!(merge.target, "pdf");
        // The plain PDF slice next door is not bothered by the file count.
        assert!(by_id(&ring, "to-pdf").is_enabled());

        let no_merger = ReachCtx {
            reach: all_ready,
            can_merge: false,
        };
        let ring = suggest(&docs, &Library::builtin(), &no_merger);
        let reason = by_id(&ring, "merge-pdf").disabled.clone().unwrap();
        assert_eq!(reason, DisabledReason::NoMerger);
        assert_eq!(reason.key(), "slice.no_merger");
        assert!(by_id(&ring, "to-pdf").is_enabled());
    }

    #[test]
    fn merging_pdfs_needs_no_conversion() {
        let nothing = |_: FormatId, _: FormatId| Reach::NoRoute;
        let ring = suggest(&["a.pdf", "b.pdf"], &Library::builtin(), &ctx(nothing));
        assert!(by_id(&ring, "merge-pdf").is_enabled());
        assert_eq!(by_id(&ring, "to-png").disabled, error(ErrorKey::NoRoute));

        // A merge of DOCX files still has to reach PDF first.
        let ring = suggest(&["a.docx", "b.docx"], &Library::builtin(), &ctx(nothing));
        assert_eq!(by_id(&ring, "merge-pdf").disabled, error(ErrorKey::NoRoute));
    }

    #[test]
    fn unknown_or_read_only_target_has_no_route() {
        // CR2 is a camera raw: a fine source, but nothing writes one.
        let library = library_of(vec![plain("odd", "nope"), plain("to-cr2", "cr2")]);
        let ring = suggest(&["a.png"], &library, &ready());
        assert_eq!(by_id(&ring, "odd").target, "nope");
        assert_eq!(by_id(&ring, "to-cr2").target, "raw");
        for slice in &ring.slices {
            assert_eq!(slice.disabled, error(ErrorKey::NoRoute));
        }
    }

    #[test]
    fn target_text_is_canonical() {
        let ring = suggest(
            &["a.png"],
            &library_of(vec![plain("j", ".JPEG"), plain("s", " * ")]),
            &ready(),
        );
        assert_eq!(by_id(&ring, "j").target, "jpg");
        assert_eq!(by_id(&ring, "s").target, "*");
    }

    #[test]
    fn enabled_when_any_source_can_do_it() {
        // Only jpg -> webp works; the png in the pile doesn't spoil it.
        let reach = |from: FormatId, to: FormatId| {
            if from.as_str() == "jpg" && to.as_str() == "webp" {
                Reach::Ready
            } else {
                Reach::NoRoute
            }
        };
        let ring = suggest(&["a.png", "b.jpg"], &Library::builtin(), &ctx(reach));
        assert!(by_id(&ring, "to-webp").is_enabled());
        assert_eq!(by_id(&ring, "to-pdf").disabled, error(ErrorKey::NoRoute));
    }

    #[test]
    fn missing_engine_reason_names_the_program() {
        let reach = |_: FormatId, to: FormatId| match to.as_str() {
            "pdf" => Reach::NeedsEngine {
                engine: "LibreOffice".into(),
                hint: "pacman -S libreoffice-fresh".into(),
            },
            "txt" => Reach::Ready,
            _ => Reach::NoRoute,
        };
        let ring = suggest(&["letter.docx"], &Library::builtin(), &ctx(reach));
        let pdf = &ring.slices[0];
        assert_eq!(pdf.preset_id, "to-pdf");
        let reason = pdf.disabled.clone().unwrap();
        assert_eq!(reason.key(), "error.engine_missing");
        assert_eq!(
            reason.detail(),
            Some("needs LibreOffice: pacman -S libreoffice-fresh")
        );
        assert_eq!(
            reason.to_string(),
            "A required program is not installed (needs LibreOffice: pacman -S libreoffice-fresh)"
        );
        assert!(by_id(&ring, "to-txt").is_enabled());
    }

    #[test]
    fn first_missing_engine_in_file_order_wins_over_no_route() {
        let reach = |from: FormatId, _: FormatId| match from.as_str() {
            "png" => Reach::NoRoute,
            "mp3" => Reach::NeedsEngine {
                engine: "FFmpeg".into(),
                hint: "pacman -S ffmpeg".into(),
            },
            _ => Reach::NeedsEngine {
                engine: "LibreOffice".into(),
                hint: "pacman -S libreoffice-fresh".into(),
            },
        };
        let files = ["a.png", "b.mp3", "c.docx"];
        let ring = suggest(&files, &Library::builtin(), &ctx(reach));
        assert_eq!(ring.profile.as_deref(), Some("mixed"));
        // zip-all is out for another reason; everyone else blames FFmpeg.
        for slice in ring.slices.iter().filter(|s| s.preset_id != "zip-all") {
            let reason = slice.disabled.clone().unwrap();
            assert_eq!(reason.detail(), Some("needs FFmpeg: pacman -S ffmpeg"));
        }
    }

    #[test]
    fn unknown_files_disable_every_slice_without_asking() {
        let reach = |_: FormatId, _: FormatId| -> Reach {
            panic!("no source format, so nothing to ask the planner")
        };
        let library = library_of(vec![
            plain("to-pdf", "pdf"),
            plain("to-png", "png"),
            Preset {
                target: preset::SAME_AS_SOURCE.into(),
                ..plain("same", "")
            },
        ]);
        let ring = suggest(&["notes.zzz"], &library, &ctx(reach));
        assert_eq!(ids(&ring), ["to-pdf", "to-png", "same"]);
        for slice in &ring.slices {
            let reason = slice.disabled.as_ref().unwrap();
            assert_eq!(reason.key(), "error.unsupported_source");
            assert_eq!(reason.to_string(), "This file type isn't supported");
        }
    }

    #[test]
    fn reason_messages_match_the_cli_report() {
        let reason = |key| DisabledReason::error(key, None).to_string();
        assert_eq!(
            reason(ErrorKey::NoRoute),
            "No engine can do this conversion"
        );
        assert_eq!(
            reason(ErrorKey::EngineMissing),
            "A required program is not installed"
        );
        assert_eq!(
            reason(ErrorKey::ConversionFailed),
            "The engine reported an error"
        );
        assert_eq!(
            DisabledReason::NeedsSeveralFiles.to_string(),
            "Select two or more files to merge"
        );
        assert_eq!(
            DisabledReason::NoMerger.to_string(),
            "No installed program can merge files"
        );
    }
}
