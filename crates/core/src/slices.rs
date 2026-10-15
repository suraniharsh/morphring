//! Ring slice suggestions: which target formats the ring offers for a set of
//! dropped files, and why a slice is greyed out.
//!
//! Filee's ring shows presets picked by a toolbar profile
//! (`src/Filee.Core/Profiles/ToolbarProfile.cs`, `ProfileSelector`) and greys
//! out the ones that can't run (`src/Filee.App/Services/PresetAvailability.cs`).
//! Until presets land in v0.2, a slice is simply a target format taken from a
//! favourites list (PLAN.md D2). The list selection, the centre title and the
//! "at least one source can do it" rule are Filee's.
//!
//! Nothing here plans routes itself: the caller passes a function that says,
//! for one source and target, whether a route exists with the installed
//! engines, only with a missing one, or not at all. The app answers that with
//! two planners (installed engines, then all engines), as D10 describes.

use std::fmt;
use std::path::Path;

use crate::contract::FormatId;
use crate::error::ErrorKey;
use crate::formats::{self, Category, FOLDER, Format};

/// Most slices one ring shows (Filee's `ToolbarProfile.MaxSlices`).
pub const MAX_SLICES: usize = 12;

/// The favourite target formats for one family of source files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavouriteList {
    /// Stable identifier, e.g. `images`.
    pub id: String,
    /// Name shown to the user, e.g. `Images`.
    pub name: String,
    /// Source categories this list is for. For a mixed list, the categories a
    /// mixed selection must stay within; empty means any selection.
    pub sources: Vec<Category>,
    /// A list for selections that no single-family list takes.
    pub mixed: bool,
    /// Target format ids in ring order, clockwise from 12 o'clock. Unknown and
    /// read-only ids are ignored.
    pub targets: Vec<String>,
}

impl FavouriteList {
    fn new(id: &str, name: &str, sources: &[Category], mixed: bool, targets: &[&str]) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            sources: sources.to_vec(),
            mixed,
            targets: targets.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    fn takes(&self, category: Category) -> bool {
        self.sources.contains(&category)
    }
}

/// The built-in favourites: Filee's default toolbar profiles
/// (`src/Filee.Core/Presets/BuiltInData.cs`, `CreateProfiles`) with each
/// "to X" preset turned into its target format, in the same order.
pub fn default_favourites() -> Vec<FavouriteList> {
    use Category::*;
    // Filee's profiles also hold presets that aren't a plain target (half
    // size, merge PDF, zip all, 720p, 128 kbps). Those wait for v0.2 presets;
    // what's left is the honest "to X" skeleton. "extract" became the folder
    // pseudo-format, which is what extracting an archive produces anyway.
    vec![
        FavouriteList::new(
            "images",
            "Images",
            &[Image],
            false,
            &["png", "jpg", "webp", "pdf", "tiff", "bmp", "ico"],
        ),
        FavouriteList::new(
            "pdf",
            "PDF",
            &[Pdf],
            false,
            &[
                "png", "jpg", "docx", "txt", "md", "epub", "hwpx", "hwp", "tiff",
            ],
        ),
        FavouriteList::new(
            "office",
            "Documents",
            &[Document],
            false,
            &[
                "pdf", "hwpx", "hwp", "docx", "odt", "txt", "html", "epub", "png",
            ],
        ),
        FavouriteList::new(
            "spreadsheets",
            "Spreadsheets",
            &[Spreadsheet],
            false,
            &["pdf", "xlsx", "csv", "ods", "hwpx", "hwp", "html"],
        ),
        FavouriteList::new(
            "presentations",
            "Presentations",
            &[Presentation],
            false,
            &["pdf", "png", "jpg", "pptx", "hwpx", "hwp"],
        ),
        FavouriteList::new(
            "hwp",
            "HWP / HWPX",
            &[Hwp],
            false,
            &[
                "pdf", "docx", "hwpx", "hwp", "png", "jpg", "txt", "md", "epub",
            ],
        ),
        FavouriteList::new(
            "text",
            "Text & Markdown",
            &[Text],
            false,
            &["pdf", "hwpx", "hwp", "docx", "html", "txt", "epub"],
        ),
        FavouriteList::new(
            "ebooks",
            "E-books",
            &[Ebook],
            false,
            &[
                "pdf", "epub", "docx", "txt", "azw3", "mobi", "hwpx", "hwp", "html",
            ],
        ),
        FavouriteList::new(
            "video",
            "Video",
            &[Video],
            false,
            &["mp4", "webm", "mov", "gif", "mp3", "mkv", "avi"],
        ),
        FavouriteList::new(
            "audio",
            "Audio",
            &[Audio],
            false,
            &["mp3", "m4a", "wav", "flac", "ogg", "opus", "aac"],
        ),
        FavouriteList::new(
            "vector",
            "Vector graphics",
            &[Vector],
            false,
            &["png", "pdf", "svg", "jpg", "webp", "eps"],
        ),
        FavouriteList::new(
            "archives",
            "Archives",
            &[Archive],
            false,
            &["folder", "zip", "7z", "tgz", "tar"],
        ),
        FavouriteList::new(
            "cad",
            "CAD drawings",
            &[Cad],
            false,
            &["pdf", "png", "svg", "dxf", "dwg"],
        ),
        FavouriteList::new(
            "fonts",
            "Fonts",
            &[Font],
            false,
            &["woff2", "woff", "ttf", "otf", "eot"],
        ),
        FavouriteList::new("mixed", "Mixed files", &[], true, &["pdf", "png", "txt"]),
    ]
}

/// Picks the favourite list for the given files, in Filee's order:
///
/// 1. the first single-family list that takes every file;
/// 2. else the first mixed list with categories that take every file;
/// 3. else the catch-all: the first mixed list with no categories, then the
///    first mixed list, then the first list.
///
/// A file of unknown type matches no family, so it always lands in step 3.
pub fn select_list<'a, P: AsRef<Path>>(
    lists: &'a [FavouriteList],
    files: &[P],
) -> Option<&'a FavouriteList> {
    let categories: Option<Vec<Category>> = files
        .iter()
        .map(|f| formats::detect(f).map(|format| format.category))
        .collect();

    // Filee compares extensions, which an unknown file can never match. A
    // `None` here is the same thing: one stranger in the pile sends the whole
    // selection to the catch-all.
    if let Some(categories) = categories.filter(|c| !c.is_empty()) {
        let takes_all = |list: &FavouriteList| categories.iter().all(|c| list.takes(*c));
        let found = lists.iter().find(|l| !l.mixed && takes_all(l)).or_else(|| {
            lists
                .iter()
                .find(|l| l.mixed && !l.sources.is_empty() && takes_all(l))
        });
        if found.is_some() {
            return found;
        }
    }

    lists
        .iter()
        .find(|l| l.mixed && l.sources.is_empty())
        .or_else(|| lists.iter().find(|l| l.mixed))
        .or_else(|| lists.first())
}

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

/// Why a slice is disabled: one of the stable error keys plus, for a missing
/// engine, a detail naming the program and how to install it (PLAN.md D10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisabledReason {
    pub key: ErrorKey,
    /// For `error.engine_missing`: `needs LibreOffice: pacman -S libreoffice-fresh`.
    pub detail: Option<String>,
}

impl DisabledReason {
    /// The short English message for the key, as the CLI report shows it.
    pub fn message(&self) -> &'static str {
        match self.key {
            ErrorKey::UnsupportedSource => "This file type isn't supported",
            ErrorKey::NoRoute => "No engine can do this conversion",
            ErrorKey::EngineMissing => "A required program is not installed",
            ErrorKey::ConversionFailed => "The engine reported an error",
        }
    }
}

impl fmt::Display for DisabledReason {
    /// The message, followed by the detail in parentheses when there is one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{} ({detail})", self.message()),
            None => f.write_str(self.message()),
        }
    }
}

/// One slice of the ring: a target format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    /// The format the files are converted to.
    pub target: FormatId,
    /// The label drawn on the slice, e.g. `PNG`.
    pub label: &'static str,
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
    /// Id of the favourite list that was used, if any list exists.
    pub list: Option<String>,
    /// Centre text: `3 × JPG` when all recognised files share a format,
    /// otherwise `3 files`.
    pub title: String,
    /// At most [`MAX_SLICES`] slices, in ring order.
    pub slices: Vec<Slice>,
}

/// Suggests the ring's slices for `files` from the favourite `lists`.
///
/// `reach(from, to)` reports whether `from` can be converted to `to`. A slice
/// is enabled when at least one of the files' formats is `Ready`. Otherwise it
/// carries `error.engine_missing` (naming the first missing engine found, in
/// file order), `error.no_route`, or `error.unsupported_source` when no file
/// has a known type.
///
/// A target that every recognised file already has is left out, as are
/// unknown, read-only and repeated targets.
pub fn suggest<P, R>(files: &[P], lists: &[FavouriteList], reach: R) -> RingSuggestion
where
    P: AsRef<Path>,
    R: Fn(FormatId, FormatId) -> Reach,
{
    let list = select_list(lists, files);
    let sources = distinct_formats(files);
    let title = match sources.as_slice() {
        [only] => format!("{} × {}", files.len(), only.name),
        _ => format!("{} files", files.len()),
    };

    let mut targets: Vec<&'static Format> = Vec::new();
    for id in list.map(|l| l.targets.as_slice()).unwrap_or_default() {
        let Some(target) = formats::find_by_id(id).filter(|f| f.writable) else {
            continue;
        };
        // Converting a pile of MP4s to MP4 is a very slow way to copy files,
        // so the slice for "the format you already have" quietly stays home.
        let already_there = !sources.is_empty() && sources.iter().all(|s| s.id == target.id);
        if already_there || targets.iter().any(|t| t.id == target.id) {
            continue;
        }
        targets.push(target);
        if targets.len() == MAX_SLICES {
            break;
        }
    }

    let slices = targets
        .into_iter()
        .map(|target| Slice {
            target: target.id,
            label: label_of(target),
            disabled: availability(&sources, target.id, &reach),
        })
        .collect();

    RingSuggestion {
        list: list.map(|l| l.id.clone()),
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

fn label_of(target: &'static Format) -> &'static str {
    // "Folder" on a slice reads like a place, not an action. Filee calls this
    // preset "Extract", and so do I.
    if target.id == FOLDER {
        "Extract"
    } else {
        target.name
    }
}

/// Filee's `PresetAvailability.Check`: `None` if any source works.
fn availability<R>(sources: &[&Format], target: FormatId, reach: &R) -> Option<DisabledReason>
where
    R: Fn(FormatId, FormatId) -> Reach,
{
    if sources.is_empty() {
        return Some(DisabledReason {
            key: ErrorKey::UnsupportedSource,
            detail: None,
        });
    }

    let mut missing = None;
    for source in sources {
        match reach(source.id, target) {
            Reach::Ready => return None,
            Reach::NeedsEngine { engine, hint } => {
                missing.get_or_insert_with(|| format!("needs {engine}: {hint}"));
            }
            Reach::NoRoute => {}
        }
    }

    Some(match missing {
        Some(detail) => DisabledReason {
            key: ErrorKey::EngineMissing,
            detail: Some(detail),
        },
        None => DisabledReason {
            key: ErrorKey::NoRoute,
            detail: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pair converts.
    fn all_ready(_: FormatId, _: FormatId) -> Reach {
        Reach::Ready
    }

    fn targets(ring: &RingSuggestion) -> Vec<&'static str> {
        ring.slices.iter().map(|s| s.target.as_str()).collect()
    }

    fn list_id(files: &[&str]) -> Option<String> {
        select_list(&default_favourites(), files).map(|l| l.id.clone())
    }

    #[test]
    fn default_lists_name_real_writable_formats() {
        let lists = default_favourites();
        for list in &lists {
            assert!(!list.targets.is_empty(), "{} is empty", list.id);
            assert!(list.targets.len() <= MAX_SLICES, "{} is too long", list.id);
            for id in &list.targets {
                let format = formats::find_by_id(id);
                assert!(
                    format.is_some_and(|f| f.writable),
                    "{}: {id} is not a writable format",
                    list.id
                );
            }
        }
        // Exactly one catch-all, and it comes last like Filee's.
        assert_eq!(lists.iter().filter(|l| l.mixed).count(), 1);
        assert!(
            lists
                .last()
                .is_some_and(|l| l.mixed && l.sources.is_empty())
        );
    }

    #[test]
    fn every_category_has_a_list() {
        let lists = default_favourites();
        for format in formats::all() {
            assert!(
                lists.iter().any(|l| !l.mixed && l.takes(format.category)),
                "no list for {}",
                format.category.name()
            );
        }
    }

    #[test]
    fn selects_by_family() {
        assert_eq!(list_id(&["a.png", "b.JPG"]).as_deref(), Some("images"));
        assert_eq!(list_id(&["report.pdf"]).as_deref(), Some("pdf"));
        assert_eq!(list_id(&["letter.docx"]).as_deref(), Some("office"));
        assert_eq!(list_id(&["clip.mp4", "b.mkv"]).as_deref(), Some("video"));
        assert_eq!(list_id(&["song.flac"]).as_deref(), Some("audio"));
        assert_eq!(list_id(&["backup.tar.gz"]).as_deref(), Some("archives"));
    }

    #[test]
    fn mixed_or_unknown_files_use_the_catch_all() {
        assert_eq!(list_id(&["a.png", "b.mp3"]).as_deref(), Some("mixed"));
        assert_eq!(list_id(&["a.png", "notes.zzz"]).as_deref(), Some("mixed"));
        assert_eq!(list_id(&["README"]).as_deref(), Some("mixed"));
        let none: [&str; 0] = [];
        assert_eq!(list_id(&none).as_deref(), Some("mixed"));
    }

    #[test]
    fn a_bounded_mixed_list_wins_over_the_catch_all() {
        let mut lists = default_favourites();
        lists.insert(
            0,
            FavouriteList::new(
                "media",
                "Media",
                &[Category::Audio, Category::Video],
                true,
                &["mp3"],
            ),
        );
        let pick = |files: &[&str]| select_list(&lists, files).map(|l| l.id.clone());
        assert_eq!(pick(&["a.mp4", "b.wav"]).as_deref(), Some("media"));
        // A single family still beats a mixed list that comes first.
        assert_eq!(pick(&["a.mp4"]).as_deref(), Some("video"));
        assert_eq!(pick(&["a.mp4", "b.png"]).as_deref(), Some("mixed"));
    }

    #[test]
    fn catch_all_falls_back_to_any_mixed_list_then_the_first() {
        let bounded = FavouriteList::new("media", "Media", &[Category::Audio], true, &["mp3"]);
        let images = FavouriteList::new("images", "Images", &[Category::Image], false, &["png"]);
        let lists = [images.clone(), bounded];
        assert_eq!(
            select_list(&lists, &["x.zzz"]).map(|l| l.id.as_str()),
            Some("media")
        );
        let only_images = [images];
        assert_eq!(
            select_list(&only_images, &["x.zzz"]).map(|l| l.id.as_str()),
            Some("images")
        );
        assert_eq!(select_list(&[], &["x.png"]), None);
    }

    #[test]
    fn slices_follow_the_list_and_skip_the_source_format() {
        let ring = suggest(&["a.png", "b.png"], &default_favourites(), all_ready);
        assert_eq!(ring.list.as_deref(), Some("images"));
        assert_eq!(targets(&ring), ["jpg", "webp", "pdf", "tiff", "bmp", "ico"]);
        assert!(ring.slices.iter().all(Slice::is_enabled));
        assert_eq!(ring.slices[0].label, "JPG");
    }

    #[test]
    fn mixed_image_formats_keep_every_target() {
        let ring = suggest(&["a.png", "b.jpg"], &default_favourites(), all_ready);
        assert_eq!(
            targets(&ring),
            ["png", "jpg", "webp", "pdf", "tiff", "bmp", "ico"]
        );
    }

    #[test]
    fn title_counts_files_and_names_a_shared_format() {
        let lists = default_favourites();
        assert_eq!(
            suggest(&["a.jpg", "b.jpeg", "c.JPG"], &lists, all_ready).title,
            "3 × JPG"
        );
        assert_eq!(
            suggest(&["a.jpg", "b.png"], &lists, all_ready).title,
            "2 files"
        );
        // Unknown files still count, like Filee's files.Count.
        assert_eq!(
            suggest(&["a.jpg", "b.zzz"], &lists, all_ready).title,
            "2 × JPG"
        );
        assert_eq!(suggest(&["b.zzz"], &lists, all_ready).title, "1 files");
    }

    #[test]
    fn folder_target_is_labelled_extract() {
        let ring = suggest(&["backup.zip"], &default_favourites(), all_ready);
        assert_eq!(targets(&ring), ["folder", "7z", "tgz", "tar"]);
        assert_eq!(ring.slices[0].label, "Extract");
    }

    #[test]
    fn unknown_read_only_and_repeated_targets_are_dropped() {
        let list = FavouriteList::new(
            "custom",
            "Custom",
            &[Category::Image],
            false,
            &["jpg", "nope", "JPG", "cr2", "webp", "jpg"],
        );
        let ring = suggest(&["a.png"], &[list], all_ready);
        assert_eq!(targets(&ring), ["jpg", "webp"]);
    }

    #[test]
    fn at_most_twelve_slices() {
        let many = [
            "jpg", "webp", "pdf", "tiff", "bmp", "ico", "gif", "avif", "heic", "jxl", "psd", "tga",
            "icns", "svg",
        ];
        let list = FavouriteList::new("big", "Big", &[Category::Image], false, &many);
        let ring = suggest(&["a.png"], &[list], all_ready);
        assert_eq!(ring.slices.len(), MAX_SLICES);
        assert_eq!(targets(&ring), many[..MAX_SLICES]);
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
        let ring = suggest(&["a.png", "b.jpg"], &default_favourites(), reach);
        let webp = ring.slices.iter().find(|s| s.target.as_str() == "webp");
        assert!(webp.is_some_and(Slice::is_enabled));
        let pdf = ring.slices.iter().find(|s| s.target.as_str() == "pdf");
        assert_eq!(
            pdf.and_then(|s| s.disabled.clone()),
            Some(DisabledReason {
                key: ErrorKey::NoRoute,
                detail: None
            })
        );
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
        let ring = suggest(&["letter.docx"], &default_favourites(), reach);
        let pdf = &ring.slices[0];
        assert_eq!(pdf.target.as_str(), "pdf");
        let reason = pdf.disabled.clone().unwrap();
        assert_eq!(reason.key, ErrorKey::EngineMissing);
        assert_eq!(
            reason.detail.as_deref(),
            Some("needs LibreOffice: pacman -S libreoffice-fresh")
        );
        assert_eq!(
            reason.to_string(),
            "A required program is not installed (needs LibreOffice: pacman -S libreoffice-fresh)"
        );
        let txt = ring.slices.iter().find(|s| s.target.as_str() == "txt");
        assert!(txt.is_some_and(Slice::is_enabled));
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
        let ring = suggest(&["a.png", "b.mp3", "c.docx"], &default_favourites(), reach);
        assert!(!ring.slices.is_empty());
        for slice in &ring.slices {
            let reason = slice.disabled.clone().unwrap();
            assert_eq!(reason.key, ErrorKey::EngineMissing);
            assert_eq!(
                reason.detail.as_deref(),
                Some("needs FFmpeg: pacman -S ffmpeg")
            );
        }
    }

    #[test]
    fn unknown_files_disable_every_slice_without_asking() {
        let reach = |_: FormatId, _: FormatId| -> Reach {
            panic!("no source format, so nothing to ask the planner")
        };
        let ring = suggest(&["notes.zzz"], &default_favourites(), reach);
        assert_eq!(ring.list.as_deref(), Some("mixed"));
        assert_eq!(targets(&ring), ["pdf", "png", "txt"]);
        for slice in &ring.slices {
            let reason = slice.disabled.as_ref().unwrap();
            assert_eq!(reason.key, ErrorKey::UnsupportedSource);
            assert_eq!(reason.to_string(), "This file type isn't supported");
        }
    }

    #[test]
    fn no_lists_means_an_empty_ring() {
        let ring = suggest(&["a.png"], &[], all_ready);
        assert_eq!(ring.list, None);
        assert!(ring.slices.is_empty());
        assert_eq!(ring.title, "1 × PNG");
    }

    #[test]
    fn reason_messages_match_the_cli_report() {
        let reason = |key| DisabledReason { key, detail: None }.to_string();
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
    }
}
