//! Pure argument building and output bookkeeping for the Ghostscript engine.
//!
//! Nothing in here launches `gs`; it only decides what to tell it and how to
//! read what it left behind, so every rule gets a unit test that runs in
//! microseconds instead of waiting on a PostScript interpreter.

use std::ffi::OsString;
use std::path::Path;

/// The resolution range Filee clamps `RenderDpi` to.
const MIN_DPI: u32 = 36;
const MAX_DPI: u32 = 1200;

/// The file name prefix of every page Ghostscript writes into the temp folder.
pub(crate) const PREFIX: &str = "p";

/// The flags every Ghostscript run starts with: no file system mischief from
/// the document, no prompts between pages, no chatter.
const BASE: [&str; 4] = ["-dSAFER", "-dBATCH", "-dNOPAUSE", "-dQUIET"];

/// A Ghostscript output device we render through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Device {
    /// RGBA PNG: the page stays transparent where the drawing leaves it empty.
    PngAlpha,
    /// RGB PNG on white paper, the way a PDF page is meant to look.
    Png16m,
    /// Baseline JPEG; quality comes from `-dJPEGQ`.
    Jpeg,
    /// 24-bit RGB TIFF, uncompressed.
    Tiff24nc,
}

impl Device {
    /// The device for converting `from` to the image format `to`, if we
    /// render that pair at all.
    ///
    /// PNG gets the alpha device for EPS, PS and AI (Filee's `pngalpha`,
    /// which keeps the drawing's transparent background) and the opaque one
    /// for PDF, where white paper is the honest answer.
    pub(crate) fn for_pair(from: &str, to: &str) -> Option<Self> {
        let vector = matches!(from, "eps" | "ps" | "ai");
        if !vector && from != "pdf" {
            return None;
        }
        match to {
            "png" if vector => Some(Self::PngAlpha),
            "png" => Some(Self::Png16m),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "tiff" | "tif" => Some(Self::Tiff24nc),
            _ => None,
        }
    }

    /// The device name for `-sDEVICE=`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::PngAlpha => "pngalpha",
            Self::Png16m => "png16m",
            Self::Jpeg => "jpeg",
            Self::Tiff24nc => "tiff24nc",
        }
    }

    /// The extension we give the temp files, without the dot.
    pub(crate) fn ext(self) -> &'static str {
        match self {
            Self::PngAlpha | Self::Png16m => "png",
            Self::Jpeg => "jpg",
            Self::Tiff24nc => "tiff",
        }
    }
}

/// One Ghostscript render run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderSpec {
    pub(crate) device: Device,
    /// Already clamped with [`clamp_dpi`].
    pub(crate) dpi: u32,
    /// JPEG quality, 1..=100; ignored by the other devices.
    pub(crate) quality: u8,
    /// Crop the page to the EPS bounding box.
    pub(crate) eps_crop: bool,
    /// First and last page (1-based, inclusive), or `None` for every page.
    pub(crate) pages: Option<(usize, usize)>,
}

/// Clamps a requested resolution to what Filee allows.
pub(crate) fn clamp_dpi(dpi: u32) -> u32 {
    dpi.clamp(MIN_DPI, MAX_DPI)
}

/// The output file pattern, relative to the run's working directory.
///
/// Relative on purpose: in `-sOutputFile` a `%` is a page-number escape, and a
/// temp folder called `/tmp/100%` would turn that into a fun afternoon. So gs
/// runs inside the temp folder and only ever sees this tame ASCII name.
pub(crate) fn output_pattern(device: Device) -> String {
    format!("{PREFIX}%d.{}", device.ext())
}

/// The full argument list for one render run of `input`.
pub(crate) fn render_args(spec: &RenderSpec, input: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = BASE.iter().map(OsString::from).collect();
    // gs prints its PostScript errors on stdout; send them to stderr, where the
    // process runner keeps the tail for the error message.
    args.push("-sstdout=%stderr".into());
    args.push(format!("-sDEVICE={}", spec.device.name()).into());
    args.push(format!("-r{}", spec.dpi).into());
    args.push("-dTextAlphaBits=4".into());
    args.push("-dGraphicsAlphaBits=4".into());
    if spec.device == Device::Jpeg {
        args.push(format!("-dJPEGQ={}", spec.quality.clamp(1, 100)).into());
    }
    if spec.eps_crop {
        args.push("-dEPSCrop".into());
    }
    if let Some((first, last)) = spec.pages {
        args.push(format!("-dFirstPage={first}").into());
        args.push(format!("-dLastPage={last}").into());
    }
    args.push(format!("-sOutputFile={}", output_pattern(spec.device)).into());
    args.push("-f".into());
    args.push(input.into());
    args
}

/// The arguments that make gs print a PDF's page count and nothing else.
///
/// The path travels in `-sFile` rather than inside the PostScript snippet, so
/// no amount of brackets or backslashes in a file name can break the string.
/// `-dSAFER` stays on; `--permit-file-read` lets exactly this one file in.
pub(crate) fn page_count_args(input: &Path) -> Vec<OsString> {
    let mut permit = OsString::from("--permit-file-read=");
    permit.push(input);
    let mut file = OsString::from("-sFile=");
    file.push(input);
    let mut args: Vec<OsString> = BASE.iter().map(OsString::from).collect();
    args.push("-dNODISPLAY".into());
    args.push(permit);
    args.push(file);
    args.push("-c".into());
    args.push("File (r) file runpdfbegin pdfpagecount = quit".into());
    args
}

/// The page count gs printed, ignoring any warnings around it. Zero is not a
/// page count, it's gs politely failing.
pub(crate) fn parse_page_count(stdout: &str) -> Option<usize> {
    stdout
        .lines()
        .rev()
        .find_map(|line| line.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
}

/// The run-relative index (1-based) of a page file gs wrote, such as
/// `p3.png` -> 3. gs numbers from 1 in every run, whatever `-dFirstPage` says.
pub(crate) fn index_of(file_name: &str, ext: &str) -> Option<usize> {
    let digits = file_name
        .strip_suffix(ext)?
        .strip_suffix('.')?
        .strip_prefix(PREFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&n| n > 0)
}

/// The output suffix for 1-based `page` when `selected` pages were asked for:
/// `_p{page}`, or nothing when there is only the one (same as Poppler).
pub(crate) fn suffix_for(page: usize, selected: usize) -> String {
    if selected == 1 {
        String::new()
    } else {
        format!("_p{page}")
    }
}

/// What an Adobe Illustrator file really is underneath.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AiKind {
    /// Illustrator 9 and later: a complete PDF ("Create PDF compatible file").
    Pdf,
    /// Saved without PDF compatibility (or Illustrator 8 and older).
    PostScript,
    /// Neither, so not an Illustrator file at all.
    Unknown,
}

/// Sniffs the first kilobyte of an `.ai` file, the way Filee does: `%PDF-`
/// anywhere in it means PDF (readers allow junk in front), `%!PS` or the DOS
/// EPS magic at the very start means PostScript.
pub(crate) fn ai_kind(head: &[u8]) -> AiKind {
    let head = &head[..head.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        AiKind::Pdf
    } else if head.starts_with(b"%!PS") || head.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) {
        AiKind::PostScript
    } else {
        AiKind::Unknown
    }
}

/// The interesting bit of what gs said before giving up: its `Error:` line
/// when there is one (the operand stack dump after it helps nobody), or the
/// usual short summary otherwise.
pub(crate) fn failure_detail(tail: &[String]) -> String {
    tail.iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with("Error:") || line.starts_with("**** Error"))
        .map(str::to_owned)
        .unwrap_or_else(|| crate::process::summarize(tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn spec(device: Device) -> RenderSpec {
        RenderSpec {
            device,
            dpi: 150,
            quality: 90,
            eps_crop: false,
            pages: None,
        }
    }

    #[test]
    fn vector_sources_get_alpha_png_and_pdf_gets_white_paper() {
        assert_eq!(Device::for_pair("eps", "png"), Some(Device::PngAlpha));
        assert_eq!(Device::for_pair("ps", "png"), Some(Device::PngAlpha));
        assert_eq!(Device::for_pair("ai", "png"), Some(Device::PngAlpha));
        assert_eq!(Device::for_pair("pdf", "png"), Some(Device::Png16m));
        for from in ["pdf", "eps", "ps", "ai"] {
            assert_eq!(Device::for_pair(from, "jpg"), Some(Device::Jpeg));
            assert_eq!(Device::for_pair(from, "tiff"), Some(Device::Tiff24nc));
        }
        assert_eq!(Device::for_pair("pdf", "webp"), None);
        assert_eq!(Device::for_pair("svg", "png"), None);
    }

    #[test]
    fn device_names_and_extensions() {
        let all = [
            Device::PngAlpha,
            Device::Png16m,
            Device::Jpeg,
            Device::Tiff24nc,
        ];
        let names: Vec<_> = all.iter().map(|d| d.name()).collect();
        let exts: Vec<_> = all.iter().map(|d| d.ext()).collect();
        assert_eq!(names, ["pngalpha", "png16m", "jpeg", "tiff24nc"]);
        assert_eq!(exts, ["png", "png", "jpg", "tiff"]);
    }

    #[test]
    fn dpi_is_clamped_like_filee() {
        assert_eq!(clamp_dpi(0), 36);
        assert_eq!(clamp_dpi(150), 150);
        assert_eq!(clamp_dpi(5000), 1200);
    }

    #[test]
    fn eps_to_png_crops_and_keeps_alpha() {
        let mut s = spec(Device::PngAlpha);
        s.eps_crop = true;
        assert_eq!(
            strings(&render_args(&s, Path::new("/in/a.eps"))),
            [
                "-dSAFER",
                "-dBATCH",
                "-dNOPAUSE",
                "-dQUIET",
                "-sstdout=%stderr",
                "-sDEVICE=pngalpha",
                "-r150",
                "-dTextAlphaBits=4",
                "-dGraphicsAlphaBits=4",
                "-dEPSCrop",
                "-sOutputFile=p%d.png",
                "-f",
                "/in/a.eps",
            ]
        );
    }

    #[test]
    fn pdf_page_run_to_jpeg_carries_range_and_quality() {
        let mut s = spec(Device::Jpeg);
        s.quality = 72;
        s.pages = Some((2, 5));
        assert_eq!(
            strings(&render_args(&s, Path::new("/in/a.pdf"))),
            [
                "-dSAFER",
                "-dBATCH",
                "-dNOPAUSE",
                "-dQUIET",
                "-sstdout=%stderr",
                "-sDEVICE=jpeg",
                "-r150",
                "-dTextAlphaBits=4",
                "-dGraphicsAlphaBits=4",
                "-dJPEGQ=72",
                "-dFirstPage=2",
                "-dLastPage=5",
                "-sOutputFile=p%d.jpg",
                "-f",
                "/in/a.pdf",
            ]
        );
    }

    #[test]
    fn jpeg_quality_is_clamped_and_other_devices_skip_it() {
        let mut s = spec(Device::Jpeg);
        s.quality = 0;
        assert!(strings(&render_args(&s, Path::new("a"))).contains(&"-dJPEGQ=1".to_string()));
        let tiff = strings(&render_args(&spec(Device::Tiff24nc), Path::new("a")));
        assert!(tiff.contains(&"-sDEVICE=tiff24nc".to_string()));
        assert!(tiff.contains(&"-sOutputFile=p%d.tiff".to_string()));
        assert!(!tiff.iter().any(|a| a.starts_with("-dJPEGQ")));
        assert!(
            !tiff
                .iter()
                .any(|a| a == "-dEPSCrop" || a.starts_with("-dFirstPage"))
        );
    }

    #[test]
    fn page_count_args_keep_the_path_out_of_the_postscript() {
        assert_eq!(
            strings(&page_count_args(Path::new("/in/(odd).pdf"))),
            [
                "-dSAFER",
                "-dBATCH",
                "-dNOPAUSE",
                "-dQUIET",
                "-dNODISPLAY",
                "--permit-file-read=/in/(odd).pdf",
                "-sFile=/in/(odd).pdf",
                "-c",
                "File (r) file runpdfbegin pdfpagecount = quit",
            ]
        );
    }

    #[test]
    fn page_count_is_the_last_number_printed() {
        assert_eq!(parse_page_count("3\n"), Some(3));
        assert_eq!(
            parse_page_count("   **** Warning: odd xref\n12\n"),
            Some(12)
        );
        assert_eq!(parse_page_count("0\n"), None);
        assert_eq!(parse_page_count("Error: /undefined\n"), None);
        assert_eq!(parse_page_count(""), None);
    }

    #[test]
    fn page_files_map_to_run_indices() {
        assert_eq!(index_of("p1.png", "png"), Some(1));
        assert_eq!(index_of("p12.tiff", "tiff"), Some(12));
        assert_eq!(index_of("p0.png", "png"), None);
        assert_eq!(index_of("p.png", "png"), None);
        assert_eq!(index_of("p1.jpg", "png"), None);
        assert_eq!(index_of("q1.png", "png"), None);
        assert_eq!(index_of("p1x.png", "png"), None);
        assert_eq!(index_of("p1png", "png"), None);
    }

    #[test]
    fn suffix_is_dropped_for_a_lone_page() {
        assert_eq!(suffix_for(2, 1), "");
        assert_eq!(suffix_for(1, 2), "_p1");
        assert_eq!(suffix_for(7, 9), "_p7");
    }

    #[test]
    fn ai_files_are_sniffed_like_filee() {
        assert_eq!(ai_kind(b"%PDF-1.5\n..."), AiKind::Pdf);
        assert_eq!(ai_kind(b"junk junk %PDF-1.4"), AiKind::Pdf);
        assert_eq!(ai_kind(b"%!PS-Adobe-3.0\n"), AiKind::PostScript);
        assert_eq!(ai_kind(&[0xC5, 0xD0, 0xD3, 0xC6, 0, 0]), AiKind::PostScript);
        assert_eq!(ai_kind(b"hello"), AiKind::Unknown);
        assert_eq!(ai_kind(b""), AiKind::Unknown);
        // Past the first kilobyte doesn't count.
        let mut late = vec![b' '; 1024];
        late.extend_from_slice(b"%PDF-1.4");
        assert_eq!(ai_kind(&late), AiKind::Unknown);
    }

    #[test]
    fn failure_detail_prefers_the_error_line() {
        let tail: Vec<String> = [
            "Error: /undefined in foo",
            "Operand stack:",
            "Current file position is 4",
            "GPL Ghostscript 10.08.0: Unrecoverable error, exit code 1",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(failure_detail(&tail), "Error: /undefined in foo");
        let plain = vec!["something odd".to_string()];
        assert_eq!(failure_detail(&plain), "something odd");
    }
}
