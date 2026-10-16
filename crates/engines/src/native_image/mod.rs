//! The built-in image engine: raster formats decoded and encoded in plain Rust
//! with the `image` crate, no ImageMagick required.
//!
//! It covers png, jpg, gif, bmp, tiff, ico, qoi, ppm and tga, each to each,
//! at a lower cost than ImageMagick so the planner reaches for it first. A
//! preset it cannot honour (see [`ops::accepts`]) benches it for the job, and
//! ImageMagick takes over.

pub mod ops;

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use image::codecs::bmp::BmpEncoder;
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::pnm::{PnmEncoder, PnmSubtype, SampleEncoding};
use image::codecs::qoi::QoiEncoder;
use image::codecs::tga::TgaEncoder;
use image::codecs::tiff::TiffEncoder;
use image::imageops::FilterType;
use image::{
    AnimationDecoder, DynamicImage, Frame, ImageDecoder, ImageEncoder, ImageError, ImageReader,
    Limits,
};
use morphring_core::{
    CancelToken, ConvertOptions, Converter, Edge, EngineError, EngineStatus, FormatId,
    OutputAllocator, Step,
};

use crate::discovery;
use crate::process;

/// The Built-in images engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct ImageEngine;

impl ImageEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for ImageEngine {
    fn id(&self) -> &'static str {
        "image"
    }

    fn display_name(&self) -> &'static str {
        "Built-in images"
    }

    fn edges(&self) -> &[Edge] {
        ops::EDGES
    }

    fn status(&self) -> EngineStatus {
        discovery::builtin_status()
    }

    fn accepts(&self, options: &ConvertOptions) -> bool {
        ops::accepts(&options.image)
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let (Some(_), Some(_)) = (ops::image_format(step.from), ops::image_format(step.to)) else {
            return Err(EngineError::Failed(format!(
                "Built-in images cannot convert {} to {}",
                step.from, step.to
            )));
        };
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        progress(0.0);

        // Ask for the name first: under the skip policy there is nothing to do,
        // and decoding a 90-megapixel TIFF to throw it away is a poor hobby.
        let ext = step.to.as_str();
        let Some(target) = out.allocate(ext, "") else {
            return Ok(Vec::new());
        };

        let picture = if step.from.as_str() == "gif" && step.to.as_str() == "gif" {
            Picture::Animation(read_animation(step, cancel)?)
        } else {
            let (img, icc) = read_still(step)?;
            Picture::Still(img, icc)
        };
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        progress(0.3);

        let picture = match picture {
            Picture::Still(img, icc) => {
                Picture::Still(ops::apply(img, step.to, &step.options), icc)
            }
            Picture::Animation(frames) => {
                let mut done = Vec::with_capacity(frames.len());
                for frame in frames {
                    if cancel.is_cancelled() {
                        return Err(EngineError::Cancelled);
                    }
                    let delay = frame.delay();
                    let img = ops::apply(
                        DynamicImage::ImageRgba8(frame.into_buffer()),
                        step.to,
                        &step.options,
                    );
                    done.push(Frame::from_parts(img.to_rgba8(), 0, 0, delay));
                }
                Picture::Animation(done)
            }
        };
        progress(0.7);

        // Encode into the job's scratch dir and move into place at the end, so a
        // failed encode never leaves half a picture under the user's name.
        std::fs::create_dir_all(&step.work_dir)?;
        let temp = tempfile::Builder::new()
            .prefix("image-")
            .tempdir_in(&step.work_dir)?;
        let scratch = temp.path().join(format!("out.{ext}"));
        write(&scratch, picture, step.to, &step.options).map_err(|err| {
            EngineError::Failed(format!("could not write {}: {err}", ext.to_uppercase()))
        })?;

        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        process::move_into_place(&scratch, &target)?;
        progress(1.0);
        Ok(vec![target])
    }
}

/// A decoded source: one picture (plus its ICC profile, if any) or the full
/// frames of an animation.
enum Picture {
    Still(DynamicImage, Option<Vec<u8>>),
    Animation(Vec<Frame>),
}

/// The decoder limits every read runs under.
fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_alloc = Some(ops::MAX_ALLOC);
    limits
}

fn read_error(path: &Path, err: &ImageError) -> EngineError {
    EngineError::Failed(format!("could not read {}: {err}", path.display()))
}

fn too_large(path: &Path) -> EngineError {
    EngineError::Failed(format!(
        "{} is larger than {} megapixels",
        path.display(),
        ops::MAX_PIXELS / 1_000_000
    ))
}

/// Decodes the first frame of `step.input`, upright, with its ICC profile.
///
/// For a multi-frame source (an animated GIF, a multi-page TIFF) that is the
/// first frame; for an ICO it is the largest icon, which is the one the
/// `image` crate's ICO decoder picks.
fn read_still(step: &Step) -> Result<(DynamicImage, Option<Vec<u8>>), EngineError> {
    let path = &step.input;
    let mut reader = ImageReader::new(BufReader::new(File::open(path)?)).with_guessed_format()?;
    // The magic bytes win when they name a format we decode (a JPEG that
    // calls itself .png is still a JPEG); otherwise the extension's word is
    // taken, since TGA has no magic bytes at all.
    let declared = ops::image_format(step.from);
    let sniffed = reader.format().filter(|f| {
        ops::FORMATS
            .iter()
            .any(|id| ops::image_format(FormatId::new(id)) == Some(*f))
    });
    if let Some(format) = sniffed.or(declared) {
        reader.set_format(format);
    }
    reader.limits(limits());

    let mut decoder = reader.into_decoder().map_err(|e| read_error(path, &e))?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > ops::MAX_PIXELS {
        return Err(too_large(path));
    }
    let orientation = decoder.orientation().map_err(|e| read_error(path, &e))?;
    let icc = keep_icc(&step.options)
        .then(|| decoder.icc_profile().ok().flatten())
        .flatten();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| read_error(path, &e))?;
    img.apply_orientation(orientation);
    Ok((img, icc))
}

/// Decodes every frame of a GIF, each one composited into a full picture.
fn read_animation(step: &Step, cancel: &CancelToken) -> Result<Vec<Frame>, EngineError> {
    let path = &step.input;
    let mut decoder =
        GifDecoder::new(BufReader::new(File::open(path)?)).map_err(|e| read_error(path, &e))?;
    let (w, h) = decoder.dimensions();
    let frame_bytes = u64::from(w) * u64::from(h) * 4;
    if u64::from(w) * u64::from(h) > ops::MAX_PIXELS {
        return Err(too_large(path));
    }
    decoder
        .set_limits(limits())
        .map_err(|e| read_error(path, &e))?;

    let mut frames = Vec::new();
    let mut held = 0u64;
    for frame in decoder.into_frames() {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        // The decoder's budget covers one frame at a time; the pile we keep is
        // our problem, so it gets the same 512 MB ceiling.
        held += frame_bytes;
        if held > ops::MAX_ALLOC {
            return Err(EngineError::Failed(format!(
                "{} has too many frames to convert in memory",
                path.display()
            )));
        }
        frames.push(frame.map_err(|e| read_error(path, &e))?);
    }
    if frames.is_empty() {
        return Err(EngineError::Failed(format!(
            "{} has no frames",
            path.display()
        )));
    }
    Ok(frames)
}

/// True when the ICC profile should travel to the output.
fn keep_icc(opts: &ConvertOptions) -> bool {
    opts.image.keep_metadata || opts.image.keep_color_profile
}

/// Encodes `picture` as `to` into `path`.
fn write(
    path: &Path,
    picture: Picture,
    to: FormatId,
    opts: &ConvertOptions,
) -> Result<(), ImageError> {
    let mut w = BufWriter::new(File::create(path)?);
    match picture {
        Picture::Animation(frames) => write_gif(&mut w, frames)?,
        Picture::Still(img, icc) => match to.as_str() {
            "gif" => {
                let frame = Frame::new(img.to_rgba8());
                write_gif(&mut w, vec![frame])?;
            }
            "ico" => write_ico(&mut w, &img, opts)?,
            _ => write_still(&mut w, ops::for_target(img, to), to, opts, icc)?,
        },
    }
    w.flush()?;
    Ok(())
}

fn write_still(
    w: &mut BufWriter<File>,
    img: DynamicImage,
    to: FormatId,
    opts: &ConvertOptions,
    icc: Option<Vec<u8>>,
) -> Result<(), ImageError> {
    // Only the formats with somewhere to put a profile get one; the encoders
    // that can't say so politely, and we nod and move on.
    fn with_icc<E: ImageEncoder>(mut encoder: E, icc: Option<Vec<u8>>) -> E {
        if let Some(icc) = icc {
            let _ = encoder.set_icc_profile(icc);
        }
        encoder
    }
    let (bytes, width, height, color) = (img.as_bytes(), img.width(), img.height(), img.color());
    let color = color.into();
    match to.as_str() {
        "png" => with_icc(PngEncoder::new(w), icc).write_image(bytes, width, height, color),
        "jpg" => with_icc(JpegEncoder::new_with_quality(w, ops::quality(opts)), icc)
            .write_image(bytes, width, height, color),
        "tiff" => with_icc(TiffEncoder::new(w), icc).write_image(bytes, width, height, color),
        "bmp" => BmpEncoder::new(w).write_image(bytes, width, height, color),
        "qoi" => QoiEncoder::new(w).write_image(bytes, width, height, color),
        "tga" => TgaEncoder::new(w).write_image(bytes, width, height, color),
        // Always P6: a .ppm that turns out to be a P5 graymap upsets the
        // stricter readers, and a gray RGB pixmap upsets nobody.
        "ppm" => PnmEncoder::new(w)
            .with_subtype(PnmSubtype::Pixmap(SampleEncoding::Binary))
            .write_image(bytes, width, height, color),
        other => Err(ImageError::Unsupported(
            image::error::UnsupportedError::from_format_and_kind(
                image::error::ImageFormatHint::Name(other.to_owned()),
                image::error::UnsupportedErrorKind::GenericFeature("encoding".to_owned()),
            ),
        )),
    }
}

fn write_gif(w: &mut BufWriter<File>, frames: Vec<Frame>) -> Result<(), ImageError> {
    let animated = frames.len() > 1;
    let mut encoder = GifEncoder::new(w);
    if animated {
        encoder.set_repeat(Repeat::Infinite)?;
    }
    encoder.encode_frames(frames)
}

/// Writes one ICO with an icon per preset size, each a PNG inside, from the
/// picture padded to a centred square.
fn write_ico(
    w: &mut BufWriter<File>,
    img: &DynamicImage,
    opts: &ConvertOptions,
) -> Result<(), ImageError> {
    let square = DynamicImage::ImageRgba8(ops::square(img));
    let mut icons = Vec::new();
    for size in ops::ico_sizes(&opts.image.ico_sizes) {
        let icon = if square.width() == size {
            square.to_rgba8()
        } else {
            square
                .resize_exact(size, size, FilterType::Lanczos3)
                .to_rgba8()
        };
        icons.push(IcoFrame::as_png(
            icon.as_raw(),
            size,
            size,
            image::ExtendedColorType::Rgba8,
        )?);
    }
    IcoEncoder::new(w).encode_images(&icons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::test_support::{NoOutputs, step};

    #[test]
    fn the_engine_has_an_id_and_every_native_edge() {
        let engine = ImageEngine::new();
        assert_eq!(engine.id(), "image");
        assert_eq!(engine.display_name(), "Built-in images");
        assert_eq!(engine.edges(), ops::EDGES);
    }

    #[test]
    fn a_skipped_output_writes_nothing() {
        let written = ImageEngine::new()
            .convert(
                &step("png", "jpg"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .expect("skip is not an error");
        assert!(written.is_empty());
    }

    #[test]
    fn foreign_formats_are_refused() {
        let err = ImageEngine::new()
            .convert(
                &step("pdf", "png"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("cannot convert pdf to png"));
    }

    #[test]
    fn a_builtin_engine_is_always_available() {
        assert_eq!(
            ImageEngine::new().status(),
            EngineStatus::Available {
                path: PathBuf::new(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }
        );
    }
}
