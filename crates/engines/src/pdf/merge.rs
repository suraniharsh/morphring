//! Merging several PDFs into one, the PDF engine's merge mode (the Merge PDF
//! preset). The job converts every non-PDF input to PDF first, so by the time
//! we get here it's PDFs all the way down.
//!
//! Ported from lopdf's `examples/merge.rs`, with a few repairs:
//! - pages keep their order. The example sorts pages by object id, which is
//!   only the page order when the PDF's writer was feeling tidy.
//! - attributes a page inherits from its `Pages` ancestors (Resources,
//!   MediaBox, CropBox, Rotate) are copied onto the page before the old tree
//!   is thrown away, so pages don't lose their fonts or turn sideways.
//! - one fresh Pages node and one fresh Catalog. The old catalogs, outlines,
//!   forms and the rest of the old document scaffolding are dropped; whatever
//!   nothing points to any more is pruned on the way out.
//!
//! Annotations (links included) live on the pages and come along for free;
//! a link that points at a page by reference still lands on the right page
//! after renumbering. Links to named destinations lose their target, since
//! the name trees lived in the catalogs we dropped.
//!
//! Encrypted inputs are refused: re-saving them would either break them or
//! quietly strip their protection, and neither is ours to decide.

use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId};
use morphring_core::{CancelToken, EngineError, Merger, OutputAllocator};

use crate::process;

/// Page attributes a page may inherit from its parents (ISO 32000-1, 7.7.3.4).
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// How deep we follow `Parent` links before calling it a loop. Real page
/// trees are a handful of levels deep; a cycle is forever.
const MAX_TREE_DEPTH: usize = 64;

/// The PDF version written when the inputs don't ask for a newer one.
const MIN_VERSION: &str = "1.5";

/// The PDF engine's merger, made with lopdf.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LopdfMerger;

/// The PDF engine's merger.
pub(crate) fn merger() -> Option<&'static dyn Merger> {
    Some(&LopdfMerger)
}

impl Merger for LopdfMerger {
    fn merge(
        &self,
        inputs: &[PathBuf],
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        if inputs.is_empty() {
            return Err(EngineError::Failed("there are no PDFs to merge".to_owned()));
        }
        // One slice of the bar per input, plus one for writing the result.
        let slices = (inputs.len() + 1) as f32;

        let mut merged = Merged::new();
        for (i, input) in inputs.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            merged.append(load(input)?);
            progress((i + 1) as f32 / slices);
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }

        let Some(target) = out.allocate("pdf", "") else {
            return Ok(Vec::new());
        };
        let mut document = merged.finish();
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        save_into_place(&mut document, &target)?;
        progress(1.0);
        Ok(vec![target])
    }
}

/// Loads one input, turning encrypted files away at the door.
fn load(path: &Path) -> Result<Document, EngineError> {
    let name = path.display();
    let document = Document::load(path).map_err(|e| match e {
        lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword => protected(path),
        lopdf::Error::IO(io) => EngineError::Io(io),
        other => EngineError::Failed(format!("cannot read {name} as a PDF: {other}")),
    })?;
    // lopdf happily opens a file whose user password is empty, so "it
    // loaded" is no proof it was never locked.
    if document.was_encrypted() || document.is_encrypted() {
        return Err(protected(path));
    }
    Ok(document)
}

fn protected(path: &Path) -> EngineError {
    EngineError::Failed(format!(
        "{} is password-protected; remove the protection before merging it",
        path.display()
    ))
}

/// The merged document under construction.
struct Merged {
    /// Every object kept so far, already renumbered so nobody collides.
    objects: Vec<(ObjectId, Object)>,
    /// The pages, in reading order.
    pages: Vec<ObjectId>,
    /// The id the next input's objects start from. 1 and 2 are reserved
    /// for the new Pages node and Catalog.
    next_id: u32,
    version: String,
}

/// The new Pages node and Catalog get these ids; inputs start after them.
const PAGES_ID: ObjectId = (1, 0);
const CATALOG_ID: ObjectId = (2, 0);

impl Merged {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
            pages: Vec::new(),
            next_id: 3,
            version: MIN_VERSION.to_owned(),
        }
    }

    /// Adds every page of `doc`, in order, after the pages already here.
    fn append(&mut self, mut doc: Document) {
        doc.renumber_objects_with(self.next_id);
        self.next_id = doc.max_id + 1;
        if version_key(&doc.version) > version_key(&self.version) {
            self.version.clone_from(&doc.version);
        }

        let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        // Resolve the inherited bits while the old tree is still standing.
        let flattened: Vec<(ObjectId, Dictionary)> = pages
            .iter()
            .filter_map(|&id| flatten_page(&doc, id).map(|page| (id, page)))
            .collect();

        for (id, object) in doc.objects {
            // Old catalogs and page tree nodes go; the new ones replace them.
            // Everything else (fonts, images, annotations) rides along, and
            // prune() sweeps up what nobody needs any more.
            if matches!(object.type_name(), Ok(b"Catalog" | b"Pages")) {
                continue;
            }
            self.objects.push((id, object));
        }
        for (id, page) in flattened {
            // Same id as the page already in `objects`; the later entry wins
            // when we collect them into the document.
            self.objects.push((id, Object::Dictionary(page)));
            self.pages.push(id);
        }
    }

    /// Builds the final document: one Pages node, one Catalog, nothing stray.
    fn finish(self) -> Document {
        let mut document = Document::with_version(self.version.as_str());
        document.objects.extend(self.objects);

        let kids: Vec<Object> = self.pages.iter().copied().map(Object::Reference).collect();
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", kids);
        pages.set("Count", self.pages.len() as i64);
        document.objects.insert(PAGES_ID, Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(PAGES_ID));
        document
            .objects
            .insert(CATALOG_ID, Object::Dictionary(catalog));
        document.trailer.set("Root", Object::Reference(CATALOG_ID));

        // Old outlines, forms, structure trees and info dictionaries hang off
        // nothing now. Into the bin.
        document.prune_objects();
        document.max_id = document.objects.keys().map(|id| id.0).max().unwrap_or(0);
        document.renumber_objects();
        document
    }
}

/// The page `id` with its inherited attributes copied in and its parent set
/// to the new Pages node. `None` if `id` is not a dictionary at all.
fn flatten_page(doc: &Document, id: ObjectId) -> Option<Dictionary> {
    let mut page = doc.get_dictionary(id).ok()?.clone();
    let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
    let mut depth = 0;
    while let Some(parent_id) = parent {
        depth += 1;
        if depth > MAX_TREE_DEPTH {
            break;
        }
        let Ok(node) = doc.get_dictionary(parent_id) else {
            break;
        };
        for key in INHERITABLE {
            // The nearest ancestor wins, and the page itself beats them all.
            if !page.has(key)
                && let Ok(value) = node.get(key)
            {
                page.set(key, value.clone());
            }
        }
        parent = node.get(b"Parent").and_then(Object::as_reference).ok();
    }
    // A page with no MediaBox anywhere is broken, but readers guess Letter;
    // say it out loud so the new tree doesn't change the guess.
    if !page.has(b"MediaBox") {
        page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
    }
    page.set("Parent", Object::Reference(PAGES_ID));
    Some(page)
}

/// "1.7" as (1, 7), for comparing versions without tripping on "1.10".
fn version_key(version: &str) -> (u32, u32) {
    let mut parts = version.trim().split('.');
    let mut next = || parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (next(), next())
}

/// Saves `document` next to `target`, then moves it into place, so a
/// half-written PDF never shows up where the user is looking.
fn save_into_place(document: &mut Document, target: &Path) -> Result<(), EngineError> {
    let folder = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let tmp = tempfile::Builder::new()
        .prefix(".morphring-merge-")
        .tempdir_in(folder)?;
    let staged = tmp.path().join("merged.pdf");
    let mut file = std::io::BufWriter::new(std::fs::File::create(&staged)?);
    document
        .save_to(&mut file)
        .map_err(|e| EngineError::Failed(format!("could not write the merged PDF: {e}")))?;
    let file = file.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    drop(file);
    process::move_into_place(&staged, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// A document with `n` pages, all of them inheriting their MediaBox and
    /// Rotate from a middle Pages node, the way some writers like it.
    fn nested(n: usize) -> Document {
        let mut doc = Document::with_version("1.4");
        let root_id = doc.new_object_id();
        let middle_id = doc.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..n {
            let page = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => middle_id,
            });
            kids.push(Object::Reference(page));
        }
        doc.objects.insert(
            middle_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Parent" => root_id,
                "Kids" => kids,
                "Count" => n as i64,
                "Rotate" => 90,
            }),
        );
        doc.objects.insert(
            root_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(middle_id)],
                "Count" => n as i64,
                "MediaBox" => vec![0.into(), 0.into(), 100.into(), 200.into()],
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => root_id,
        });
        doc.trailer.set("Root", catalog);
        doc
    }

    #[test]
    fn pages_keep_what_they_inherited() {
        let mut merged = Merged::new();
        merged.append(nested(2));
        merged.append(nested(1));
        let doc = merged.finish();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 3);
        for id in pages.values() {
            let page = doc.get_dictionary(*id).unwrap();
            assert_eq!(page.get(b"Rotate").unwrap().as_i64().unwrap(), 90);
            let media = page.get(b"MediaBox").unwrap().as_array().unwrap();
            assert_eq!(media[3].as_i64().unwrap(), 200);
        }
    }

    #[test]
    fn only_one_catalog_and_one_pages_node_survive() {
        let mut merged = Merged::new();
        merged.append(nested(1));
        merged.append(nested(1));
        let doc = merged.finish();
        let count = |name: &[u8]| {
            doc.objects
                .values()
                .filter(|o| o.type_name().is_ok_and(|t| t == name))
                .count()
        };
        assert_eq!(count(b"Catalog"), 1);
        assert_eq!(count(b"Pages"), 1);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(version_key("1.10") > version_key("1.7"));
        assert!(version_key("2.0") > version_key("1.7"));
        assert_eq!(version_key("junk"), (0, 0));
    }
}
