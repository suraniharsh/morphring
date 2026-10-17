//! Conversion engines for MorphRing. Most engines wrap an external program
//! (ImageMagick, FFmpeg, LibreOffice, Poppler, Ghostscript, Pandoc) that is
//! found at runtime; the built-in ones (images, SVG, PDF, archives) are plain
//! Rust behind cargo features. Every engine implements
//! `morphring_core::Converter`.

#[cfg(feature = "archive")]
pub mod archive;
pub mod discovery;
pub mod ffmpeg;
pub mod ghostscript;
pub mod magick;
#[cfg(feature = "img")]
pub mod native_image;
pub mod pandoc;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod poppler;
pub mod process;
pub mod progress;
pub mod registry;
pub mod soffice;
#[cfg(feature = "svg")]
pub mod svg;
#[cfg(windows)]
mod win_job;

/// Plumbing for engines made of several parts (PDF, Poppler, archives): each
/// part lists its own edges and converts its own pairs, so the tasks that fill
/// them in never have to touch each other's files.
pub(crate) mod parts {
    use std::path::PathBuf;

    use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, Step};

    /// A part's conversion function.
    pub(crate) type ConvertFn = fn(
        &Step,
        &mut dyn OutputAllocator,
        &dyn Fn(f32),
        &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError>;

    /// One part of an engine: the edges it handles and how it converts them.
    pub(crate) struct Part {
        pub(crate) edges: fn() -> Vec<Edge>,
        pub(crate) convert: ConvertFn,
    }

    /// Every part of one engine, with their edges collected once.
    pub(crate) struct Parts {
        edges: Vec<Edge>,
        routes: Vec<(Vec<Edge>, ConvertFn)>,
    }

    impl Parts {
        /// Collects the edges of `parts`, in order.
        pub(crate) fn new(parts: &[Part]) -> Self {
            let routes: Vec<(Vec<Edge>, ConvertFn)> = parts
                .iter()
                .map(|part| ((part.edges)(), part.convert))
                .collect();
            let edges = routes
                .iter()
                .flat_map(|(edges, _)| edges.iter().copied())
                .collect();
            Self { edges, routes }
        }

        /// Every part's edges, concatenated.
        pub(crate) fn edges(&self) -> &[Edge] {
            &self.edges
        }

        /// Hands `step` to the first part that has its `from -> to` edge.
        pub(crate) fn convert(
            &self,
            engine: &str,
            step: &Step,
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            // The planner only hands us pairs from edges(), so a miss here means
            // somebody called convert by hand. Say so instead of guessing.
            let route = self.routes.iter().find(|(edges, _)| {
                edges
                    .iter()
                    .any(|edge| edge.from == step.from && edge.to == step.to)
            });
            match route {
                Some((_, convert)) => convert(step, out, progress, cancel),
                None => Err(EngineError::Failed(format!(
                    "{engine} cannot convert {} to {}",
                    step.from, step.to
                ))),
            }
        }
    }

    #[cfg(test)]
    pub(crate) mod test_support {
        use std::path::PathBuf;

        use morphring_core::{ConvertOptions, FormatId, OutputAllocator, Step};

        /// An allocator that never hands out a path; stubs never ask anyway.
        pub(crate) struct NoOutputs;

        impl OutputAllocator for NoOutputs {
            fn allocate(&mut self, _ext: &str, _suffix: &str) -> Option<PathBuf> {
                None
            }
        }

        /// A step from `from` to `to` on a file that does not exist.
        pub(crate) fn step(from: &'static str, to: &'static str) -> Step {
            Step {
                input: PathBuf::from("does-not-exist"),
                from: FormatId::new(from),
                to: FormatId::new(to),
                options: ConvertOptions::default(),
                work_dir: std::env::temp_dir(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::test_support::{NoOutputs, step};
        use super::*;

        fn pdf_to_png() -> Vec<Edge> {
            vec![Edge::new("pdf", "png")]
        }

        fn pdf_to_txt() -> Vec<Edge> {
            vec![Edge::new("pdf", "txt"), Edge::new("pdf", "png")]
        }

        fn first(
            _: &Step,
            _: &mut dyn OutputAllocator,
            _: &dyn Fn(f32),
            _: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            Ok(vec![PathBuf::from("first")])
        }

        fn second(
            _: &Step,
            _: &mut dyn OutputAllocator,
            _: &dyn Fn(f32),
            _: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            Ok(vec![PathBuf::from("second")])
        }

        fn parts() -> Parts {
            Parts::new(&[
                Part {
                    edges: pdf_to_png,
                    convert: first,
                },
                Part {
                    edges: pdf_to_txt,
                    convert: second,
                },
            ])
        }

        fn run(
            parts: &Parts,
            from: &'static str,
            to: &'static str,
        ) -> Result<Vec<PathBuf>, EngineError> {
            parts.convert(
                "Test",
                &step(from, to),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
        }

        #[test]
        fn edges_are_concatenated_in_part_order() {
            let parts = parts();
            let pairs: Vec<String> = parts
                .edges()
                .iter()
                .map(|e| format!("{}>{}", e.from, e.to))
                .collect();
            assert_eq!(pairs, ["pdf>png", "pdf>txt", "pdf>png"]);
        }

        #[test]
        fn convert_goes_to_the_first_part_with_the_pair() {
            let parts = parts();
            assert_eq!(run(&parts, "pdf", "png").unwrap(), [PathBuf::from("first")]);
            assert_eq!(
                run(&parts, "pdf", "txt").unwrap(),
                [PathBuf::from("second")]
            );
        }

        #[test]
        fn an_unknown_pair_is_refused() {
            let err = run(&parts(), "png", "pdf").unwrap_err();
            assert_eq!(err.to_string(), "Test cannot convert png to pdf");
        }
    }
}
