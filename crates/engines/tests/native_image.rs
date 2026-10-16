//! Golden tests for the built-in image engine. No external program runs:
//! the engine is plain Rust, and the outputs are checked by decoding them
//! with the same `image` crate (in its own reader, not the engine's code).

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{
    AnimationDecoder, DynamicImage, GenericImageView, ImageEncoder, ImageFormat, ImageReader, Rgb,
    RgbImage, Rgba, RgbaImage,
};
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Resize, Step,
    TiffCompression,
};
use morphring_engines::native_image::ImageEngine;
use tempfile::TempDir;

use common::fixture;

/// Hands out `<dir>/out<suffix>.<ext>`.
struct DirAllocator(PathBuf);

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.0.join(format!("out{suffix}.{ext}")))
    }
}

struct Run {
    _tmp: TempDir,
    result: Result<Vec<PathBuf>, EngineError>,
    progress: Vec<f32>,
}

impl Run {
    fn output(&self) -> &Path {
        match &self.result {
            Ok(paths) => {
                assert_eq!(paths.len(), 1, "one output expected, got {paths:?}");
                &paths[0]
            }
            Err(err) => panic!("conversion failed: {err}"),
        }
    }
}

fn convert(input: &Path, from: &'static str, to: &'static str, options: ConvertOptions) -> Run {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let step = Step {
        input: input.to_path_buf(),
        from: FormatId::new(from),
        to: FormatId::new(to),
        options,
        work_dir: tmp.path().join("work"),
    };
    let progress = RefCell::new(Vec::new());
    let result = ImageEngine::new().convert(
        &step,
        &mut DirAllocator(tmp.path().to_path_buf()),
        &|p| progress.borrow_mut().push(p),
        &CancelToken::new(),
    );
    Run {
        _tmp: tmp,
        result,
        progress: progress.into_inner(),
    }
}

fn open(path: &Path) -> DynamicImage {
    ImageReader::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .with_guessed_format()
        .unwrap_or_else(|e| panic!("sniff {}: {e}", path.display()))
        .decode()
        .unwrap_or_else(|e| panic!("{} does not decode: {e}", path.display()))
}

fn format_of(path: &Path) -> ImageFormat {
    ImageReader::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .with_guessed_format()
        .unwrap_or_else(|e| panic!("sniff {}: {e}", path.display()))
        .format()
        .unwrap_or_else(|| panic!("{} has no known format", path.display()))
}

/// A half-transparent 40 x 30 PNG with an opaque red stripe, written to `dir`.
fn alpha_png(dir: &Path) -> PathBuf {
    let mut img = RgbaImage::from_pixel(40, 30, Rgba([0, 0, 255, 0]));
    for x in 0..40 {
        for y in 0..10 {
            img.put_pixel(x, y, Rgba([255, 0, 0, 255]));
        }
    }
    let path = dir.join("alpha.png");
    img.save(&path)
        .unwrap_or_else(|e| panic!("write alpha png: {e}"));
    path
}

/// Inserts an EXIF APP1 segment carrying `orientation` right after the JPEG
/// SOI marker. Big-endian TIFF header, one IFD, one entry: tag 0x0112.
fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    assert_eq!(&jpeg[..2], [0xFF, 0xD8], "not a JPEG");
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM\x00\x2A\x00\x00\x00\x08"); // header, IFD at 8
    tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
    tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // Orientation
    tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_be_bytes()); // count
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]); // value padding
    tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
    let mut payload = b"Exif\x00\x00".to_vec();
    payload.extend_from_slice(&tiff);
    let len = u16::try_from(payload.len() + 2).unwrap_or_else(|e| panic!("segment too long: {e}"));

    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[test]
fn png_to_jpg_has_no_alpha_and_keeps_its_size() {
    let src = fixture("sample.png");
    let (w, h) = image::image_dimensions(&src).expect("fixture size");
    let run = convert(&src, "png", "jpg", ConvertOptions::default());
    let out = run.output();
    assert_eq!(out.extension().and_then(|e| e.to_str()), Some("jpg"));
    assert_eq!(format_of(out), ImageFormat::Jpeg);
    let img = open(out);
    assert!(!img.color().has_alpha());
    assert_eq!(img.dimensions(), (w, h));
    assert_eq!(run.progress.first(), Some(&0.0));
    assert_eq!(run.progress.last(), Some(&1.0));
    assert!(run.progress.windows(2).all(|p| p[0] <= p[1]));
}

#[test]
fn transparency_is_flattened_onto_the_background() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = alpha_png(tmp.path());
    let mut opts = ConvertOptions::default();
    opts.image.background = "#00FF00".to_owned();
    let run = convert(&src, "png", "jpg", opts);
    let img = open(run.output()).to_rgb8();
    let stripe = img.get_pixel(20, 5);
    let clear = img.get_pixel(20, 25);
    assert!(stripe[0] > 200 && stripe[1] < 60, "stripe is {stripe:?}");
    assert!(
        clear[1] > 200 && clear[0] < 60 && clear[2] < 60,
        "background is {clear:?}"
    );
}

#[test]
fn half_size_preset_halves_the_picture() {
    let src = fixture("sample.png");
    let (w, h) = image::image_dimensions(&src).expect("fixture size");
    let mut opts = ConvertOptions::default();
    opts.image.resize = Resize::Percent;
    opts.image.resize_percent = 50;
    let run = convert(&src, "png", "png", opts);
    let img = open(run.output());
    assert_eq!(img.dimensions(), (w / 2, h / 2));
}

#[test]
fn long_edge_never_upscales() {
    let src = fixture("sample.png");
    let size = image::image_dimensions(&src).expect("fixture size");
    let mut opts = ConvertOptions::default();
    opts.image.resize = Resize::LongEdge;
    opts.image.long_edge = 4000;
    let run = convert(&src, "png", "png", opts);
    assert_eq!(open(run.output()).dimensions(), size);
}

#[test]
fn exif_rotated_jpg_comes_out_upright() {
    let tmp = tempfile::tempdir().expect("temp dir");
    // 32 x 16: left half red, right half blue. Orientation 6 means "rotate
    // 90 degrees clockwise to view", so upright it is 16 x 32, red on top.
    let mut img = RgbImage::from_pixel(32, 16, Rgb([0, 0, 255]));
    for x in 0..16 {
        for y in 0..16 {
            img.put_pixel(x, y, Rgb([255, 0, 0]));
        }
    }
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 95)
        .write_image(img.as_raw(), 32, 16, image::ExtendedColorType::Rgb8)
        .expect("encode jpeg");
    let src = tmp.path().join("rotated.jpg");
    std::fs::write(&src, with_exif_orientation(&jpeg, 6)).expect("write jpeg");

    let run = convert(&src, "jpg", "png", ConvertOptions::default());
    let out = open(run.output()).to_rgb8();
    assert_eq!(out.dimensions(), (16, 32));
    let top = out.get_pixel(8, 4);
    let bottom = out.get_pixel(8, 28);
    assert!(top[0] > 200 && top[2] < 60, "top is {top:?}");
    assert!(bottom[2] > 200 && bottom[0] < 60, "bottom is {bottom:?}");
}

#[test]
fn grayscale_jpg_is_gray() {
    let run = convert(
        &fixture("sample.png"),
        "png",
        "jpg",
        ConvertOptions {
            image: morphring_core::ImageOptions {
                grayscale: true,
                ..Default::default()
            },
            ..ConvertOptions::default()
        },
    );
    assert!(matches!(open(run.output()), DynamicImage::ImageLuma8(_)));
}

#[test]
fn quality_changes_the_jpeg() {
    let src = fixture("sample.jpg");
    let size = |q| {
        let run = convert(
            &src,
            "jpg",
            "jpg",
            ConvertOptions {
                quality: Some(q),
                ..ConvertOptions::default()
            },
        );
        std::fs::metadata(run.output()).expect("output").len()
    };
    assert!(size(10) < size(100));
}

#[test]
fn every_target_decodes_as_itself() {
    let src = fixture("sample.png");
    let (w, h) = image::image_dimensions(&src).expect("fixture size");
    for (to, format) in [
        ("png", ImageFormat::Png),
        ("jpg", ImageFormat::Jpeg),
        ("gif", ImageFormat::Gif),
        ("bmp", ImageFormat::Bmp),
        ("tiff", ImageFormat::Tiff),
        ("qoi", ImageFormat::Qoi),
        ("ppm", ImageFormat::Pnm),
    ] {
        let run = convert(&src, "png", to, ConvertOptions::default());
        let out = run.output();
        assert_eq!(format_of(out), format, "{to}");
        assert_eq!(open(out).dimensions(), (w, h), "{to}");
    }
    // TGA has no magic bytes, so it is read by name.
    let run = convert(&src, "png", "tga", ConvertOptions::default());
    let tga = ImageReader::with_format(
        std::io::BufReader::new(std::fs::File::open(run.output()).expect("open tga")),
        ImageFormat::Tga,
    )
    .decode()
    .expect("tga decodes");
    assert_eq!(tga.dimensions(), (w, h));
}

#[test]
fn tga_and_ppm_sources_convert() {
    let src = fixture("sample.png");
    let tga = convert(&src, "png", "tga", ConvertOptions::default());
    let back = convert(tga.output(), "tga", "png", ConvertOptions::default());
    assert_eq!(format_of(back.output()), ImageFormat::Png);
    let ppm = convert(&src, "png", "ppm", ConvertOptions::default());
    let back = convert(ppm.output(), "ppm", "bmp", ConvertOptions::default());
    assert_eq!(format_of(back.output()), ImageFormat::Bmp);
}

#[test]
fn png_to_ico_holds_every_preset_size() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = alpha_png(tmp.path());
    let mut opts = ConvertOptions::default();
    opts.image.ico_sizes = vec![16, 48, 600];
    let run = convert(&src, "png", "ico", opts);
    let out = run.output();
    assert_eq!(format_of(out), ImageFormat::Ico);
    let bytes = std::fs::read(out).expect("read ico");
    // ICONDIR: reserved, type 1, count; 600 is out of range and dropped.
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 2);
    // The decoder picks the largest icon, padded to a square.
    assert_eq!(open(out).dimensions(), (48, 48));

    let back = convert(out, "ico", "png", ConvertOptions::default());
    assert_eq!(open(back.output()).dimensions(), (48, 48));
}

#[test]
fn gif_to_gif_keeps_the_animation() {
    let src = fixture("sample.gif");
    let run = convert(&src, "gif", "gif", ConvertOptions::default());
    let frames = |path: &Path| {
        image::codecs::gif::GifDecoder::new(std::io::BufReader::new(
            std::fs::File::open(path).expect("open gif"),
        ))
        .expect("gif decoder")
        .into_frames()
        .collect_frames()
        .expect("frames")
        .len()
    };
    assert!(frames(&src) > 1);
    assert_eq!(frames(run.output()), frames(&src));
}

#[test]
fn gif_to_png_takes_the_first_frame() {
    let run = convert(
        &fixture("sample.gif"),
        "gif",
        "png",
        ConvertOptions::default(),
    );
    assert_eq!(format_of(run.output()), ImageFormat::Png);
}

#[test]
fn a_jpeg_named_png_is_still_read() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = tmp.path().join("liar.png");
    std::fs::copy(fixture("sample.jpg"), &src).expect("copy");
    let run = convert(&src, "png", "bmp", ConvertOptions::default());
    assert_eq!(format_of(run.output()), ImageFormat::Bmp);
}

#[test]
fn a_broken_file_fails_cleanly() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let src = tmp.path().join("broken.png");
    std::fs::write(&src, b"\x89PNG\r\n\x1a\nnot really").expect("write");
    let run = convert(&src, "png", "jpg", ConvertOptions::default());
    let err = run.result.expect_err("garbage must not convert");
    assert!(err.to_string().contains("could not read"), "{err}");
    assert!(!tmp.path().join("out.jpg").exists());
}

#[test]
fn a_cancelled_step_writes_nothing() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let step = Step {
        input: fixture("sample.png"),
        from: FormatId::new("png"),
        to: FormatId::new("jpg"),
        options: ConvertOptions::default(),
        work_dir: tmp.path().join("work"),
    };
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = ImageEngine::new().convert(
        &step,
        &mut DirAllocator(tmp.path().to_path_buf()),
        &|_| {},
        &cancel,
    );
    assert!(matches!(result, Err(EngineError::Cancelled)));
    assert!(!tmp.path().join("out.jpg").exists());
}

#[test]
fn accepts_mirrors_the_options_it_implements() {
    let engine = ImageEngine::new();
    assert!(engine.accepts(&ConvertOptions::default()));
    let mut opts = ConvertOptions::default();
    opts.image.resize = Resize::Width;
    opts.image.grayscale = true;
    opts.image.tiff_compression = TiffCompression::None;
    opts.quality = Some(50);
    assert!(engine.accepts(&opts));

    opts.image.dpi = Some(300);
    assert!(!engine.accepts(&opts));
    opts.image.dpi = None;
    opts.image.webp_lossless = true;
    assert!(!engine.accepts(&opts));
    opts.image.webp_lossless = false;
    opts.image.tiff_compression = TiffCompression::Zip;
    assert!(!engine.accepts(&opts));
    opts.image.tiff_compression = TiffCompression::Lzw;
    opts.image.keep_metadata = false;
    assert!(!engine.accepts(&opts));
    opts.image.keep_color_profile = false;
    assert!(engine.accepts(&opts));
}
