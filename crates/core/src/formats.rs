//! The format registry: every format MorphRing knows, and how a file name maps
//! to one. Ported from Filee's `src/Filee.Core/Formats/FormatRegistry.cs` and
//! `FileFormat.cs`.
//!
//! Detection is by extension only; nothing here opens a file.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::contract::FormatId;

/// The broad family a format belongs to. Listings group formats by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Image,
    Pdf,
    Document,
    Spreadsheet,
    Presentation,
    Hwp,
    Text,
    Video,
    Audio,
    Ebook,
    Archive,
    Vector,
    Cad,
    Font,
}

impl Category {
    /// The category's name as shown in listings, e.g. `Image`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Pdf => "Pdf",
            Self::Document => "Document",
            Self::Spreadsheet => "Spreadsheet",
            Self::Presentation => "Presentation",
            Self::Hwp => "Hwp",
            Self::Text => "Text",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Ebook => "Ebook",
            Self::Archive => "Archive",
            Self::Vector => "Vector",
            Self::Cad => "Cad",
            Self::Font => "Font",
        }
    }
}

/// A file format MorphRing knows about.
#[derive(Debug, PartialEq, Eq)]
pub struct Format {
    /// Stable lower-case identifier used in converter edges, e.g. `jpg`.
    pub id: FormatId,
    /// Short label for display, e.g. `JPG`.
    pub name: &'static str,
    /// Format family.
    pub category: Category,
    /// Extensions without the dot. The first one is used for output files.
    /// Compound extensions such as `tar.gz` are allowed.
    pub extensions: &'static [&'static str],
    /// `false` for formats MorphRing only reads (camera RAW, Apple Pages, ...);
    /// these are never offered as a target.
    pub writable: bool,
}

impl Format {
    /// The extension (without the dot) used when writing this format. Empty for
    /// the folder pseudo-format.
    pub fn primary_extension(&self) -> &'static str {
        self.extensions.first().copied().unwrap_or("")
    }

    /// Whether this is the folder pseudo-format (the output of extracting an archive).
    pub fn is_folder(&self) -> bool {
        self.id == FOLDER
    }
}

/// Id of the pseudo-format whose output is a folder holding an archive's files.
pub const FOLDER: FormatId = FormatId::new("folder");

const fn rw(
    id: &'static str,
    name: &'static str,
    category: Category,
    extensions: &'static [&'static str],
) -> Format {
    Format {
        id: FormatId::new(id),
        name,
        category,
        extensions,
        writable: true,
    }
}

const fn ro(
    id: &'static str,
    name: &'static str,
    category: Category,
    extensions: &'static [&'static str],
) -> Format {
    Format {
        writable: false,
        ..rw(id, name, category, extensions)
    }
}

use Category::*;

// Copied line for line from FormatRegistry.cs. The order is the order users
// see in listings, so resist the urge to alphabetise it; I already resisted for you.
// To add a format: one line here, plus an edge in at least one engine.
static FORMATS: [Format; 173] = [
    // Raster images
    rw("png", "PNG", Image, &["png"]),
    rw("jpg", "JPG", Image, &["jpg", "jpeg", "jpe", "jfif"]),
    rw("webp", "WEBP", Image, &["webp"]),
    rw("tiff", "TIFF", Image, &["tiff", "tif"]),
    rw("bmp", "BMP", Image, &["bmp", "dib"]),
    rw("gif", "GIF", Image, &["gif"]),
    rw("ico", "ICO", Image, &["ico"]),
    rw("avif", "AVIF", Image, &["avif"]),
    rw("heic", "HEIC", Image, &["heic", "heif"]),
    rw("jxl", "JPEG XL", Image, &["jxl"]),
    rw("jp2", "JPEG 2000", Image, &["jp2", "j2k", "jpf", "jpx"]),
    rw("psd", "PSD", Image, &["psd"]),
    rw("psb", "PSB", Image, &["psb"]),
    rw("tga", "TGA", Image, &["tga", "icb", "vda", "vst"]),
    rw("qoi", "QOI", Image, &["qoi"]),
    // One id for the whole Netpbm family: pgm, pbm and pnm are extension
    // aliases of ppm, since one PNM encoder writes them all. Splitting it would
    // be four ids doing one job.
    rw("ppm", "PPM", Image, &["ppm", "pgm", "pbm", "pnm"]),
    rw("icns", "ICNS", Image, &["icns"]),
    ro("xcf", "XCF", Image, &["xcf"]),
    ro(
        "raw",
        "RAW",
        Image,
        &[
            "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "raf",
            "pef", "dcr", "kdc", "erf", "mos", "mrw", "x3f", "3fr", "fff", "iiq", "mef", "rwl",
            "srw", "raw",
        ],
    ),
    // Vector graphics
    rw("svg", "SVG", Vector, &["svg"]),
    rw("svgz", "SVGZ", Vector, &["svgz"]),
    rw("eps", "EPS", Vector, &["eps", "epsf", "epsi"]),
    rw("ps", "PS", Vector, &["ps"]),
    ro("ai", "AI", Vector, &["ai"]),
    ro("emf", "EMF", Vector, &["emf"]),
    ro("wmf", "WMF", Vector, &["wmf"]),
    ro("cdr", "CDR", Vector, &["cdr"]),
    ro("cgm", "CGM", Vector, &["cgm"]),
    ro("vsd", "VSD", Vector, &["vsd", "vsdx"]),
    rw("odg", "ODG", Vector, &["odg"]),
    // PDF
    rw("pdf", "PDF", Pdf, &["pdf"]),
    // Word processing
    rw("docx", "DOCX", Document, &["docx"]),
    rw("docm", "DOCM", Document, &["docm"]),
    rw("dotx", "DOTX", Document, &["dotx"]),
    rw("dotm", "DOTM", Document, &["dotm"]),
    rw("doc", "DOC", Document, &["doc"]),
    ro("dot", "DOT", Document, &["dot"]),
    rw("odt", "ODT", Document, &["odt"]),
    rw("rtf", "RTF", Document, &["rtf"]),
    ro("wps", "WPS", Document, &["wps"]),
    ro("wpd", "WPD", Document, &["wpd"]),
    ro("lwp", "LWP", Document, &["lwp"]),
    ro("abw", "ABW", Document, &["abw", "zabw"]),
    ro("pages", "Pages", Document, &["pages"]),
    ro("sdw", "SDW", Document, &["sdw"]),
    ro("pub", "PUB", Document, &["pub"]),
    ro("eml", "EML", Document, &["eml"]),
    // Korean word processor (Hancom)
    rw("hwpx", "HWPX", Hwp, &["hwpx"]),
    rw("hwp", "HWP", Hwp, &["hwp"]),
    // Spreadsheets
    rw("xlsx", "XLSX", Spreadsheet, &["xlsx"]),
    rw("xlsm", "XLSM", Spreadsheet, &["xlsm"]),
    rw("xltx", "XLTX", Spreadsheet, &["xltx"]),
    rw("xls", "XLS", Spreadsheet, &["xls", "xlt"]),
    rw("ods", "ODS", Spreadsheet, &["ods"]),
    rw("csv", "CSV", Spreadsheet, &["csv"]),
    rw("tsv", "TSV", Spreadsheet, &["tsv", "tab"]),
    ro("et", "ET", Spreadsheet, &["et"]),
    ro("numbers", "Numbers", Spreadsheet, &["numbers"]),
    ro("sdc", "SDC", Spreadsheet, &["sdc"]),
    // Presentations
    rw("pptx", "PPTX", Presentation, &["pptx"]),
    rw("pptm", "PPTM", Presentation, &["pptm"]),
    rw("potx", "POTX", Presentation, &["potx"]),
    rw("ppsx", "PPSX", Presentation, &["ppsx"]),
    rw("ppt", "PPT", Presentation, &["ppt", "pps", "pot"]),
    rw("odp", "ODP", Presentation, &["odp"]),
    ro("key", "Keynote", Presentation, &["key"]),
    ro("dps", "DPS", Presentation, &["dps"]),
    ro("sda", "SDA", Presentation, &["sda", "sdd"]),
    // Plain text / markup
    rw("txt", "TXT", Text, &["txt"]),
    rw("md", "Markdown", Text, &["md", "markdown"]),
    rw("html", "HTML", Text, &["html", "htm", "xhtml"]),
    rw("rst", "reStructuredText", Text, &["rst"]),
    rw("tex", "LaTeX", Text, &["tex", "latex"]),
    // E-books
    rw("epub", "EPUB", Ebook, &["epub"]),
    rw("mobi", "MOBI", Ebook, &["mobi"]),
    rw("azw3", "AZW3", Ebook, &["azw3"]),
    ro("azw", "AZW", Ebook, &["azw"]),
    ro("azw4", "AZW4", Ebook, &["azw4"]),
    ro("prc", "PRC", Ebook, &["prc"]),
    rw("fb2", "FB2", Ebook, &["fb2"]),
    rw("cbz", "CBZ", Ebook, &["cbz"]),
    ro("cbr", "CBR", Ebook, &["cbr"]),
    ro("cb7", "CB7", Ebook, &["cb7"]),
    ro("cbt", "CBT", Ebook, &["cbt"]),
    ro("cbc", "CBC", Ebook, &["cbc"]),
    rw("htmlz", "HTMLZ", Ebook, &["htmlz"]),
    rw("txtz", "TXTZ", Ebook, &["txtz"]),
    ro("chm", "CHM", Ebook, &["chm"]),
    rw("lit", "LIT", Ebook, &["lit"]),
    rw("lrf", "LRF", Ebook, &["lrf"]),
    rw("pdb", "PDB", Ebook, &["pdb"]),
    rw("pml", "PML", Ebook, &["pml"]),
    rw("rb", "RB", Ebook, &["rb"]),
    rw("snb", "SNB", Ebook, &["snb"]),
    rw("tcr", "TCR", Ebook, &["tcr"]),
    ro("oeb", "OEB", Ebook, &["oeb", "opf"]),
    // Video
    rw("mp4", "MP4", Video, &["mp4"]),
    rw("m4v", "M4V", Video, &["m4v"]),
    rw("mov", "MOV", Video, &["mov", "qt"]),
    rw("mkv", "MKV", Video, &["mkv"]),
    rw("webm", "WEBM", Video, &["webm"]),
    rw("avi", "AVI", Video, &["avi", "divx"]),
    rw("wmv", "WMV", Video, &["wmv", "asf"]),
    rw("flv", "FLV", Video, &["flv", "f4v"]),
    rw("mpg", "MPEG", Video, &["mpg", "mpeg", "mpe", "m1v", "m2v"]),
    rw("ts", "TS", Video, &["ts"]),
    rw("m2ts", "M2TS", Video, &["m2ts", "mts", "m2t"]),
    rw("3gp", "3GP", Video, &["3gp", "3gpp"]),
    rw("3g2", "3G2", Video, &["3g2"]),
    rw("ogv", "OGV", Video, &["ogv"]),
    rw("vob", "VOB", Video, &["vob"]),
    ro("mod", "MOD", Video, &["mod", "tod"]),
    rw("dv", "DV", Video, &["dv", "dif"]),
    rw("mxf", "MXF", Video, &["mxf"]),
    ro("rm", "RM", Video, &["rm", "rmvb"]),
    ro("swf", "SWF", Video, &["swf"]),
    ro("wtv", "WTV", Video, &["wtv", "dvr-ms", "dvr"]),
    ro("cavs", "CAVS", Video, &["cavs"]),
    // Audio
    rw("mp3", "MP3", Audio, &["mp3"]),
    rw("m4a", "M4A", Audio, &["m4a"]),
    rw("m4b", "M4B", Audio, &["m4b"]),
    rw("aac", "AAC", Audio, &["aac"]),
    rw("wav", "WAV", Audio, &["wav", "wave"]),
    rw("flac", "FLAC", Audio, &["flac"]),
    rw("ogg", "OGG", Audio, &["ogg", "oga"]),
    rw("opus", "OPUS", Audio, &["opus"]),
    rw("wma", "WMA", Audio, &["wma"]),
    rw("aiff", "AIFF", Audio, &["aiff", "aif", "aifc"]),
    rw("ac3", "AC3", Audio, &["ac3"]),
    rw("amr", "AMR", Audio, &["amr"]),
    rw("au", "AU", Audio, &["au", "snd"]),
    rw("caf", "CAF", Audio, &["caf"]),
    rw("weba", "WEBA", Audio, &["weba"]),
    rw("mka", "MKA", Audio, &["mka"]),
    rw("mp2", "MP2", Audio, &["mp2", "mpa"]),
    ro("dss", "DSS", Audio, &["dss"]),
    rw("voc", "VOC", Audio, &["voc"]),
    // Archives
    rw("zip", "ZIP", Archive, &["zip"]),
    rw("7z", "7Z", Archive, &["7z"]),
    ro("rar", "RAR", Archive, &["rar"]),
    rw("tar", "TAR", Archive, &["tar"]),
    rw("tgz", "TAR.GZ", Archive, &["tar.gz", "tgz", "taz"]),
    rw(
        "tbz2",
        "TAR.BZ2",
        Archive,
        &["tar.bz2", "tbz2", "tar.bz", "tbz"],
    ),
    rw("txz", "TAR.XZ", Archive, &["tar.xz", "txz"]),
    ro("tar7z", "TAR.7Z", Archive, &["tar.7z"]),
    ro("tlz", "TAR.LZ", Archive, &["tar.lz", "tlz"]),
    ro("tz", "TAR.Z", Archive, &["tar.z", "tz"]),
    rw("gz", "GZ", Archive, &["gz"]),
    rw("bz2", "BZ2", Archive, &["bz2", "bz"]),
    rw("xz", "XZ", Archive, &["xz"]),
    ro("lz", "LZ", Archive, &["lz"]),
    ro("lzma", "LZMA", Archive, &["lzma"]),
    ro("z", "Z", Archive, &["z"]),
    ro("jar", "JAR", Archive, &["jar"]),
    ro("cab", "CAB", Archive, &["cab"]),
    ro("cpio", "CPIO", Archive, &["cpio"]),
    ro("deb", "DEB", Archive, &["deb"]),
    ro("rpm", "RPM", Archive, &["rpm"]),
    ro("dmg", "DMG", Archive, &["dmg"]),
    ro("iso", "ISO", Archive, &["iso"]),
    ro("img", "IMG", Archive, &["img"]),
    ro("lha", "LHA", Archive, &["lha", "lzh"]),
    ro("arj", "ARJ", Archive, &["arj"]),
    ro("alz", "ALZ", Archive, &["alz"]),
    ro("egg", "EGG", Archive, &["egg"]),
    rw("folder", "Folder", Archive, &[""]),
    // CAD
    rw("dwg", "DWG", Cad, &["dwg"]),
    rw("dxf", "DXF", Cad, &["dxf"]),
    // Fonts
    rw("ttf", "TTF", Font, &["ttf"]),
    rw("otf", "OTF", Font, &["otf"]),
    rw("woff", "WOFF", Font, &["woff"]),
    rw("woff2", "WOFF2", Font, &["woff2"]),
    rw("eot", "EOT", Font, &["eot"]),
];

/// Every known format, in display order. Includes the folder pseudo-format.
pub fn all() -> &'static [Format] {
    &FORMATS
}

/// The format with the given id (case-insensitive), if any.
pub fn find_by_id(id: &str) -> Option<&'static Format> {
    FORMATS
        .iter()
        .find(|f| f.id.as_str().eq_ignore_ascii_case(id))
}

/// The format for an extension, with or without leading dots
/// (case-insensitive): `jpeg` and `.JPG` give jpg, `tar.gz` gives tgz.
pub fn find_by_extension(extension: &str) -> Option<&'static Format> {
    let ext = extension.trim_start_matches('.');
    // The folder's extension is "", so an empty query must not match it.
    if ext.is_empty() {
        return None;
    }
    FORMATS.iter().find(|f| {
        f.extensions
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext))
    })
}

/// Detects a file's format from its extension (see [`extension_of`]).
pub fn detect(path: impl AsRef<Path>) -> Option<&'static Format> {
    find_by_extension(&extension_of(path))
}

/// Resolves the text a user gave as a target format (`--to`): surrounding
/// whitespace and leading dots are ignored, then it is looked up as an id
/// first and as an extension second. `JPEG`, `.jpg` and `jpg` all give jpg.
pub fn lookup_target(text: &str) -> Option<&'static Format> {
    let key = text.trim().trim_start_matches('.');
    find_by_id(key).or_else(|| find_by_extension(key))
}

/// The extension of the file name in `path`, without the dot, with its
/// original casing. Known two-part extensions win over the last part alone:
/// `Backup.TAR.GZ` gives `TAR.GZ`, `notes.txt.gz` gives `gz`. A name with no
/// dot, or whose only dot is its first character (`.bashrc`), has no
/// extension and gives an empty string.
pub fn extension_of(path: impl AsRef<Path>) -> String {
    let Some(name) = path.as_ref().file_name() else {
        return String::new();
    };
    let bytes = name.as_encoded_bytes();
    let len = extension_len(bytes);
    String::from_utf8_lossy(&bytes[bytes.len() - len..]).into_owned()
}

/// The file name in `path` without its (possibly two-part) extension:
/// `backup.tar.gz` gives `backup`, `.bashrc` stays `.bashrc`.
pub fn name_without_extension(path: impl AsRef<Path>) -> OsString {
    let Some(name) = path.as_ref().file_name() else {
        return OsString::new();
    };
    let len = extension_len(name.as_encoded_bytes());
    if len == 0 {
        return name.to_owned();
    }
    // len + 1 always lands on the ASCII dot, so the cut is on a char boundary.
    prefix(name, name.as_encoded_bytes().len() - len - 1)
}

/// Length in bytes of the extension at the end of a file name (0 if none).
///
/// Works on the raw encoded bytes so a non-UTF-8 name on Linux still gets its
/// extension found; every extension we know is ASCII anyway.
fn extension_len(name: &[u8]) -> usize {
    // Longest compound first, like Filee. Two different compounds of the same
    // length can't both be a suffix of one name, so "first longest match" is
    // the same as Filee's sorted scan without keeping a sorted copy around.
    let compound = FORMATS
        .iter()
        .flat_map(|f| f.extensions.iter())
        .filter(|ext| ext.contains('.'))
        // The name must be longer than ".tar.gz", or a file called exactly
        // ".tar.gz" would end up with an empty stem.
        .filter(|ext| name.len() > ext.len() + 1)
        .filter(|ext| {
            let tail = &name[name.len() - ext.len() - 1..];
            tail[0] == b'.' && tail[1..].eq_ignore_ascii_case(ext.as_bytes())
        })
        .map(|ext| ext.len())
        .max();
    if let Some(len) = compound {
        return len;
    }
    // Filee asks .NET, which thinks ".bashrc" has the extension "bashrc". On
    // Linux that's a hidden file called bashrc, not a nameless bash-rc
    // document, so a leading dot doesn't count (PLAN.md T05 asks for this).
    match name.iter().rposition(|&b| b == b'.') {
        Some(dot) if dot > 0 => name.len() - dot - 1,
        _ => 0,
    }
}

/// The first `len` bytes of `name`, where `len` sits right before an ASCII dot.
fn prefix(name: &OsStr, len: usize) -> OsString {
    if let Some(text) = name.to_str() {
        return OsString::from(&text[..len]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(&name.as_bytes()[..len]).to_owned()
    }
    #[cfg(not(unix))]
    {
        // Non-UTF-8 names outside Unix are unpaired UTF-16 surrogates. Rare
        // enough that a lossy stem beats an unsafe slice.
        OsString::from(String::from_utf8_lossy(&name.as_encoded_bytes()[..len]).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(format: Option<&Format>) -> Option<&'static str> {
        format.map(|f| f.id.as_str())
    }

    #[test]
    fn extensions_are_unique_ignoring_case() {
        let mut seen = std::collections::HashSet::new();
        for format in all() {
            for ext in format.extensions {
                assert!(
                    seen.insert(ext.to_ascii_lowercase()),
                    "extension {ext:?} of {} is claimed twice",
                    format.id
                );
            }
        }
    }

    #[test]
    fn ids_are_unique_lower_case_and_have_extensions() {
        let mut seen = std::collections::HashSet::new();
        for format in all() {
            let id = format.id.as_str();
            assert!(seen.insert(id), "id {id} appears twice");
            assert_eq!(id, id.to_ascii_lowercase());
            assert!(!format.extensions.is_empty(), "{id} has no extension");
        }
    }

    #[test]
    fn table_matches_filee() {
        // PLAN.md and docs/reverse-engineering/07 say 171 and 56, but counting
        // FormatRegistry.cs itself (and doc 07's own table) gives 171 real
        // formats plus folder, 57 of them read-only. The source wins. MorphRing
        // adds one of its own on top (qoi), hence 173.
        assert_eq!(all().len(), 173);
        assert_eq!(all().iter().filter(|f| !f.writable).count(), 57);
        assert_eq!(all()[0].id.as_str(), "png");
        assert_eq!(all().last().map(|f| f.id.as_str()), Some("eot"));
    }

    #[test]
    fn folder_is_a_writable_pseudo_format() {
        let folder = find_by_id("folder").unwrap();
        assert!(folder.is_folder());
        assert!(folder.writable);
        assert_eq!(folder.primary_extension(), "");
        assert_eq!(folder.category, Category::Archive);
        assert!(find_by_extension("").is_none());
        assert!(find_by_extension(".").is_none());
        assert!(detect("archive").is_none());
    }

    #[test]
    fn primary_extension_is_the_first() {
        assert_eq!(find_by_id("jpg").unwrap().primary_extension(), "jpg");
        assert_eq!(find_by_id("tgz").unwrap().primary_extension(), "tar.gz");
        assert_eq!(find_by_id("tbz2").unwrap().primary_extension(), "tar.bz2");
    }

    #[test]
    fn extension_of_prefers_compound_and_keeps_casing() {
        assert_eq!(extension_of("Backup.TAR.GZ"), "TAR.GZ");
        assert_eq!(extension_of("/some/dir/backup.tar.gz"), "tar.gz");
        assert_eq!(extension_of("x.tar.bz2"), "tar.bz2");
        assert_eq!(extension_of("x.tar.bz"), "tar.bz");
        assert_eq!(extension_of("notes.txt.gz"), "gz");
        assert_eq!(extension_of("photo.JPG"), "JPG");
        assert_eq!(extension_of("clip.dvr-ms"), "dvr-ms");
    }

    #[test]
    fn extension_of_edge_cases() {
        assert_eq!(extension_of(".bashrc"), "");
        assert_eq!(extension_of("README"), "");
        assert_eq!(extension_of("trailing."), "");
        assert_eq!(extension_of(""), "");
        // A file named exactly ".tar.gz" has no stem, so only the last part counts.
        assert_eq!(extension_of(".tar.gz"), "gz");
        assert_eq!(extension_of("a.tar.gz"), "tar.gz");
        // Dots in folder names are not the file's business.
        assert_eq!(extension_of("my.folder/README"), "");
        assert_eq!(extension_of("my.folder/.hidden"), "");
    }

    #[test]
    fn name_without_extension_strips_compound() {
        assert_eq!(name_without_extension("backup.tar.gz"), "backup");
        assert_eq!(name_without_extension("dir/Backup.TAR.GZ"), "Backup");
        assert_eq!(name_without_extension("notes.txt.gz"), "notes.txt");
        assert_eq!(name_without_extension(".bashrc"), ".bashrc");
        assert_eq!(name_without_extension("README"), "README");
        assert_eq!(name_without_extension("trailing."), "trailing.");
        assert_eq!(
            name_without_extension("한글 report's \"quoted\" 100%.png"),
            "한글 report's \"quoted\" 100%"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_still_work() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(OsStr::from_bytes(b"caf\xe9.JPEG"));
        assert_eq!(extension_of(path), "JPEG");
        assert_eq!(id(detect(path)), Some("jpg"));
        assert_eq!(name_without_extension(path).as_bytes(), b"caf\xe9");
    }

    #[test]
    fn detects_formats() {
        // Filee's FormatRegistryTests, minus its ".md" row: a bare dotfile has
        // no extension here (see extension_of).
        assert_eq!(id(detect("a.JPEG")), Some("jpg"));
        assert_eq!(id(detect("b.tif")), Some("tiff"));
        assert_eq!(id(detect("c.HWPX")), Some("hwpx"));
        assert_eq!(id(detect("clip.MP4")), Some("mp4"));
        assert_eq!(id(detect("backup.tar.gz")), Some("tgz"));
        assert_eq!(id(detect("notes.txt.gz")), Some("gz"));
        assert_eq!(id(detect("x.xyz")), None);
        assert_eq!(id(detect("README")), None);
        assert_eq!(id(detect(".md")), None);
    }

    #[test]
    fn qoi_is_a_writable_image() {
        assert_eq!(id(detect("x.qoi")), Some("qoi"));
        assert_eq!(id(detect("X.QOI")), Some("qoi"));
        let qoi = find_by_id("qoi").unwrap();
        assert!(qoi.writable);
        assert_eq!(qoi.category, Category::Image);
    }

    #[test]
    fn pnm_family_shares_the_ppm_id() {
        for ext in ["ppm", "pgm", "pbm", "pnm"] {
            assert_eq!(id(detect(format!("x.{ext}"))), Some("ppm"), "{ext}");
        }
    }

    #[test]
    fn engine_route_ids_have_the_right_access() {
        // The v0.2 engines route to these ids; a typo or a flipped flag here
        // would only show up as a missing ring entry much later.
        for rw_id in [
            "eps", "ps", "svgz", "tgz", "rst", "tex", "md", "txt", "html", "rtf", "odt", "docx",
            "tar", "zip", "folder", "qoi", "ppm",
        ] {
            let format = find_by_id(rw_id).unwrap_or_else(|| panic!("{rw_id} is missing"));
            assert!(format.writable, "{rw_id} should be writable");
        }
        let ai = find_by_id("ai").unwrap();
        assert!(!ai.writable, "ai is read-only");
    }

    #[test]
    fn every_camera_raw_is_the_raw_format() {
        for ext in [
            "dng", "CR2", "cr3", "nef", "arw", "orf", "rw2", "raf", "srw", "raw",
        ] {
            assert_eq!(id(detect(format!("shot.{ext}"))), Some("raw"), "{ext}");
        }
        assert!(!find_by_id("raw").unwrap().writable);
    }

    #[test]
    fn find_by_extension_strips_dots_and_ignores_case() {
        assert_eq!(id(find_by_extension(".tar.gz")), Some("tgz"));
        assert_eq!(id(find_by_extension("TGZ")), Some("tgz"));
        assert_eq!(id(find_by_extension("jpeg")), Some("jpg"));
        assert_eq!(id(find_by_extension("..JPG")), Some("jpg"));
        assert_eq!(id(find_by_extension("xyz")), None);
    }

    #[test]
    fn find_by_id_ignores_case() {
        assert_eq!(id(find_by_id("PNG")), Some("png"));
        assert_eq!(id(find_by_id("jpeg")), None);
        assert_eq!(id(find_by_id("")), None);
    }

    #[test]
    fn lookup_target_tries_id_then_extension() {
        assert_eq!(id(lookup_target("JPEG")), Some("jpg"));
        assert_eq!(id(lookup_target(" .webp ")), Some("webp"));
        assert_eq!(id(lookup_target("tar.gz")), Some("tgz"));
        assert_eq!(id(lookup_target("folder")), Some("folder"));
        assert_eq!(id(lookup_target("FOLDER")), Some("folder"));
        assert_eq!(id(lookup_target("")), None);
        assert_eq!(id(lookup_target(".")), None);
        assert_eq!(id(lookup_target("nope")), None);
    }

    #[test]
    fn categories_appear_in_filee_order() {
        let mut order: Vec<Category> = Vec::new();
        for format in all() {
            if !order.contains(&format.category) {
                order.push(format.category);
            }
        }
        let names: Vec<&str> = order.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            [
                "Image",
                "Vector",
                "Pdf",
                "Document",
                "Hwp",
                "Spreadsheet",
                "Presentation",
                "Text",
                "Ebook",
                "Video",
                "Audio",
                "Archive",
                "Cad",
                "Font"
            ]
        );
    }
}
