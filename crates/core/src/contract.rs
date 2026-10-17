//! The converter contract: the types every engine, the planner and the job
//! runner agree on. Ported from Filee's `src/Filee.Core/Conversion/IConverter.cs`
//! and the job-state rule in `src/Filee.Core/Conversion/JobQueue.cs`.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{EngineError, ErrorKey};
use crate::options::{
    ArchiveOptions, DocumentOptions, ImageOptions, MediaOptions, PdfOptions, TracingOptions,
};

/// Identifier of a format, such as `"png"` or `"docx"`.
///
/// Engines name formats by id string, so they do not depend on the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormatId(&'static str);

impl FormatId {
    /// Wraps a format id. Ids are lower case, without a leading dot.
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// The id as a string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for FormatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Which routes an edge may take part in, judged by the format the job's file
/// started as. Ported from the two flags on Filee's `ConversionEdge`
/// (`src/Filee.Core/Conversion/IConverter.cs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EdgeFlags {
    /// Used only in routes that start from a raster image (category `Image`).
    /// Image tracing may follow `icns -> png`, but must never redraw a PDF or a
    /// video frame that happened to become a PNG along the way.
    pub image_sources_only: bool,
    /// Never used in routes that start from a raster image. `pdf -> svg`
    /// through LibreOffice keeps a PDF's vectors, but a photo that went through
    /// a PDF would only come back as a picture inside an SVG.
    pub not_for_image_sources: bool,
    /// Never used once the route has passed through a raster image (the
    /// source or any intermediate). Reading text out of a PDF, or a soundtrack
    /// out of a video, only works on what the user's file really held; a
    /// picture of a page has no text layer and a GIF turned into a video has
    /// no sound, however politely we ask.
    pub needs_original_content: bool,
}

impl EdgeFlags {
    /// No restriction: the edge is usable in every route.
    pub const NONE: Self = Self {
        image_sources_only: false,
        not_for_image_sources: false,
        needs_original_content: false,
    };

    /// Whether an edge with these flags may be used in a route whose source
    /// file is (`true`) or is not (`false`) a raster image.
    pub const fn allows(self, image_source: bool) -> bool {
        !((self.image_sources_only && !image_source)
            || (self.not_for_image_sources && image_source))
    }
}

/// One direct conversion an engine can perform, for example `docx -> pdf`.
///
/// `from` may equal `to` (a self edge, used to re-encode or resize).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from: FormatId,
    pub to: FormatId,
    /// Relative cost used by the planner; multi-step routes add up their costs.
    pub cost: u32,
    /// Which routes the edge may take part in; none by default.
    pub flags: EdgeFlags,
}

impl Edge {
    /// The cost of a normal direct conversion.
    pub const DEFAULT_COST: u32 = 10;

    /// An edge with the default cost of 10.
    pub const fn new(from: &'static str, to: &'static str) -> Self {
        Self::with_cost(from, to, Self::DEFAULT_COST)
    }

    /// An edge with an explicit cost.
    pub const fn with_cost(from: &'static str, to: &'static str, cost: u32) -> Self {
        Self {
            from: FormatId::new(from),
            to: FormatId::new(to),
            cost,
            flags: EdgeFlags::NONE,
        }
    }

    /// The same edge, usable only in routes that start from a raster image.
    pub const fn image_sources_only(mut self) -> Self {
        self.flags.image_sources_only = true;
        self
    }

    /// The same edge, never used in routes that start from a raster image.
    pub const fn not_for_image_sources(mut self) -> Self {
        self.flags.not_for_image_sources = true;
        self
    }

    /// The same edge, never used once the route has passed through a raster
    /// image: text and audio extraction need the original content.
    pub const fn needs_original_content(mut self) -> Self {
        self.flags.needs_original_content = true;
        self
    }
}

/// Options chosen by the user for a conversion: a preset's option groups plus
/// the command line's quality override.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConvertOptions {
    /// Quality in percent (1 to 100), or `None` for the engine default. This is
    /// the CLI's `-q`; a preset's own quality lives in `image.quality`.
    pub quality: Option<u8>,
    pub image: ImageOptions,
    pub pdf: PdfOptions,
    pub document: DocumentOptions,
    pub media: MediaOptions,
    pub archive: ArchiveOptions,
    pub trace: TracingOptions,
}

impl ConvertOptions {
    /// The quality bucket engines map their own settings from.
    pub fn quality_level(&self) -> QualityLevel {
        QualityLevel::from_percent(self.quality)
    }
}

/// Coarse quality bucket that engines translate into tool-specific settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QualityLevel {
    High,
    #[default]
    Normal,
    Small,
}

impl QualityLevel {
    /// 85 and above is `High`, 60 to 84 is `Normal`, below 60 is `Small`;
    /// `None` is `Normal`.
    pub const fn from_percent(percent: Option<u8>) -> Self {
        match percent {
            None => Self::Normal,
            Some(p) if p >= 85 => Self::High,
            Some(p) if p >= 60 => Self::Normal,
            Some(_) => Self::Small,
        }
    }
}

/// Everything an engine needs to perform one step.
#[derive(Clone, Debug)]
pub struct Step {
    /// The file to read. Engines never modify it.
    pub input: PathBuf,
    pub from: FormatId,
    pub to: FormatId,
    pub options: ConvertOptions,
    /// Private scratch directory for this job; deleted afterwards.
    pub work_dir: PathBuf,
}

/// Hands out output paths to an engine. It is the only way an engine gets one.
pub trait OutputAllocator {
    /// Returns the path to write the next output to, or `None` when this output
    /// must be skipped (an existing file under the skip policy).
    ///
    /// `ext` has no dot; an empty `ext` asks for a folder path. `suffix` is
    /// appended to the name, for example `_p2` when one input produces many files.
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf>;
}

/// Combines many inputs into one output (merge PDFs, zip a selection, stack
/// images into a multi-page TIFF), where a plain step only ever sees one file.
pub trait Merger: Send + Sync {
    /// Merges `inputs`, in order, and returns the files actually written.
    ///
    /// The same rules as [`Converter::convert`] apply: outputs come from `out`,
    /// `progress` only goes up, `cancel` is honoured and inputs are never
    /// modified.
    fn merge(
        &self,
        inputs: &[PathBuf],
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError>;
}

/// Packs many files, unchanged, into one archive ("Compress into one
/// archive"). Ported from Filee's `IFileCombiner`; the job runner uses it when
/// a preset sets `archive.combine_into_one`.
pub trait Combiner: Send + Sync {
    /// True when files can be packed into `format` right now (for example the
    /// tool it needs is installed).
    fn can_combine_into(&self, format: FormatId) -> bool;

    /// Writes every file of `inputs` into one archive of `format`, each under
    /// its own name, and returns the files actually written.
    ///
    /// The same rules as [`Converter::convert`] apply.
    fn combine(
        &self,
        inputs: &[PathBuf],
        format: FormatId,
        options: &ConvertOptions,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError>;
}

/// Writes TIFF pages (and every page of multi-page TIFFs) into one multi-page
/// TIFF, in order. Ported from Filee's `ITiffMerger`; the job runner uses it
/// when a preset sets `image.multi_page_tiff`.
pub trait TiffMerger: Send + Sync {
    /// Merges `inputs`, in order, and returns the files actually written.
    ///
    /// The same rules as [`Converter::convert`] apply.
    fn merge_tiff(
        &self,
        inputs: &[PathBuf],
        options: &ImageOptions,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError>;
}

/// Whether an engine can currently be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineStatus {
    /// The engine's program was found.
    Available { path: PathBuf, version: String },
    /// The engine's program is missing; `hint` says how to install it.
    Missing { hint: String },
}

impl EngineStatus {
    /// True when conversions can run.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Cooperative cancellation flag shared between the caller and running steps.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Every clone of this token sees it.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// True once `cancel` has been called on any clone.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A conversion engine (ImageMagick, FFmpeg, LibreOffice, ...).
///
/// Implementations must be thread-safe up to `max_parallelism` concurrent
/// calls, must check `cancel`, and must never modify the input file.
pub trait Converter: Send + Sync {
    /// Stable lower-case id, such as `"magick"`.
    fn id(&self) -> &'static str;

    /// Human readable name, such as `"ImageMagick"`.
    fn display_name(&self) -> &'static str;

    /// Direct conversions this engine supports.
    fn edges(&self) -> &[Edge];

    /// How many conversions may run at once; 0 means one per CPU core.
    fn max_parallelism(&self) -> usize {
        0
    }

    /// Whether the engine is usable. Cheap; results may be cached.
    fn status(&self) -> EngineStatus;

    /// Whether this engine can honour `options`. An engine that returns
    /// `false` is left out of route planning for that job, so another engine
    /// (or a longer route) is used instead.
    fn accepts(&self, _options: &ConvertOptions) -> bool {
        true
    }

    /// The engine's merge mode, if it has one.
    fn merger(&self) -> Option<&dyn Merger> {
        None
    }

    /// The engine's "many files into one archive" mode, if it has one.
    fn combiner(&self) -> Option<&dyn Combiner> {
        None
    }

    /// The engine's multi-page TIFF mode, if it has one.
    fn tiff_merger(&self) -> Option<&dyn TiffMerger> {
        None
    }

    /// Performs one step and returns the files actually written.
    ///
    /// `progress` receives values in `0.0..=1.0` that only go up.
    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError>;
}

/// How one file of a job ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileState {
    Done,
    /// Every output already existed and the skip policy was chosen.
    Skipped,
    Failed {
        key: ErrorKey,
        detail: String,
    },
    Cancelled,
}

/// The result of converting one source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileResult {
    pub source: PathBuf,
    pub state: FileState,
    pub outputs: Vec<PathBuf>,
}

/// How a whole job ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobState {
    Completed,
    CompletedWithErrors,
    Failed,
    Cancelled,
}

impl JobState {
    /// Filee's rule: `Cancelled` if cancel fired, `Completed` if every file is
    /// done or skipped, `CompletedWithErrors` if at least one file is done,
    /// otherwise `Failed`.
    pub fn from_files(files: &[FileResult], cancelled: bool) -> Self {
        let done = |f: &FileResult| f.state == FileState::Done;
        if cancelled {
            Self::Cancelled
        } else if files
            .iter()
            .all(|f| done(f) || f.state == FileState::Skipped)
        {
            Self::Completed
        } else if files.iter().any(done) {
            Self::CompletedWithErrors
        } else {
            Self::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(state: FileState) -> FileResult {
        FileResult {
            source: PathBuf::from("a.png"),
            state,
            outputs: Vec::new(),
        }
    }

    fn failed() -> FileState {
        FileState::Failed {
            key: ErrorKey::ConversionFailed,
            detail: "boom".into(),
        }
    }

    #[test]
    fn job_state_follows_filee_rules() {
        let done_and_skipped = [file(FileState::Done), file(FileState::Skipped)];
        assert_eq!(
            JobState::from_files(&done_and_skipped, false),
            JobState::Completed
        );

        let mixed = [file(FileState::Done), file(failed())];
        assert_eq!(
            JobState::from_files(&mixed, false),
            JobState::CompletedWithErrors
        );

        let all_bad = [
            file(failed()),
            file(FileState::Skipped),
            file(FileState::Cancelled),
        ];
        assert_eq!(JobState::from_files(&all_bad, false), JobState::Failed);

        // Cancel wins even when every file managed to finish first.
        assert_eq!(
            JobState::from_files(&done_and_skipped, true),
            JobState::Cancelled
        );
    }

    #[test]
    fn quality_level_boundaries() {
        assert_eq!(QualityLevel::from_percent(Some(1)), QualityLevel::Small);
        assert_eq!(QualityLevel::from_percent(Some(59)), QualityLevel::Small);
        assert_eq!(QualityLevel::from_percent(Some(60)), QualityLevel::Normal);
        assert_eq!(QualityLevel::from_percent(Some(84)), QualityLevel::Normal);
        assert_eq!(QualityLevel::from_percent(Some(85)), QualityLevel::High);
        assert_eq!(QualityLevel::from_percent(Some(100)), QualityLevel::High);
        assert_eq!(QualityLevel::from_percent(None), QualityLevel::Normal);
        assert_eq!(
            ConvertOptions::default().quality_level(),
            QualityLevel::Normal
        );
    }

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn edges_default_to_cost_ten() {
        let edge = Edge::new("png", "jpg");
        assert_eq!(edge.cost, 10);
        assert_eq!(edge.from.as_str(), "png");
        assert_eq!(Edge::with_cost("pptx", "pdf", 0).cost, 0);
        assert_eq!(edge.flags, EdgeFlags::NONE);
    }

    #[test]
    fn edge_flags_gate_image_sources() {
        let trace = Edge::with_cost("png", "svg", 40).image_sources_only();
        assert!(trace.flags.image_sources_only);
        assert!(trace.flags.allows(true));
        assert!(!trace.flags.allows(false));

        let office = Edge::with_cost("pdf", "svg", 25).not_for_image_sources();
        assert!(office.flags.not_for_image_sources);
        assert!(!office.flags.allows(true));
        assert!(office.flags.allows(false));

        assert!(EdgeFlags::NONE.allows(true) && EdgeFlags::NONE.allows(false));
    }

    #[test]
    fn needs_original_content_sets_only_its_own_flag() {
        let text = Edge::new("pdf", "txt").needs_original_content();
        assert!(text.flags.needs_original_content);
        assert!(!text.flags.image_sources_only && !text.flags.not_for_image_sources);
        // Judged per route by the planner, not by the source alone.
        assert!(text.flags.allows(true) && text.flags.allows(false));
        assert_eq!(EdgeFlags::default(), EdgeFlags::NONE);
    }
}
