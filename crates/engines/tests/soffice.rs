//! Golden tests for the LibreOffice engine. They skip when soffice is missing
//! (it always is in CI, PLAN.md D7), so run them on the dev machine before a tag.

mod common;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use common::fixture;
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::soffice::SofficeEngine;

/// Hands out `<dir>/<stem>.<ext>`, or nothing at all when `skip` is set.
struct DirAllocator {
    dir: PathBuf,
    stem: String,
    skip: bool,
}

impl DirAllocator {
    fn new(dir: &Path, stem: &str) -> Self {
        Self {
            dir: dir.to_path_buf(),
            stem: stem.to_string(),
            skip: false,
        }
    }
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        if self.skip {
            return None;
        }
        Some(self.dir.join(format!("{}{suffix}.{ext}", self.stem)))
    }
}

fn step(input: &str, from: &'static str, to: &'static str, work: &Path) -> Step {
    Step {
        input: fixture(input),
        from: FormatId::new(from),
        to: FormatId::new(to),
        options: ConvertOptions::default(),
        work_dir: work.to_path_buf(),
    }
}

fn convert(
    engine: &SofficeEngine,
    step: &Step,
    out_dir: &Path,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let mut out = DirAllocator::new(out_dir, "result");
    engine.convert(step, &mut out, &|_| {}, cancel)
}

fn first_bytes(path: &Path, n: usize) -> Vec<u8> {
    let mut buf = vec![0; n];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut buf))
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    buf
}

/// Every running process whose command line mentions `needle`.
fn processes_mentioning(needle: &str) -> Vec<String> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .filter_map(|e| {
            let raw = fs::read(e.path().join("cmdline")).ok()?;
            let line = String::from_utf8_lossy(&raw).replace('\0', " ");
            line.contains(needle)
                .then(|| format!("{}: {line}", e.file_name().to_string_lossy()))
        })
        .collect()
}

#[test]
fn docx_to_pdf() {
    require_engine!("soffice");
    let tmp = tempfile::tempdir().unwrap();
    let engine = SofficeEngine::new_in(tmp.path().join("data"));
    let progress = std::sync::Mutex::new(Vec::new());
    let mut out = DirAllocator::new(tmp.path(), "sample");
    let written = engine
        .convert(
            &step("sample.docx", "docx", "pdf", &tmp.path().join("work")),
            &mut out,
            &|p| progress.lock().unwrap().push(p),
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(written, vec![tmp.path().join("sample.pdf")]);
    assert_eq!(first_bytes(&written[0], 4), b"%PDF");
    let progress = progress.into_inner().unwrap();
    assert!(progress.windows(2).all(|w| w[0] <= w[1]), "{progress:?}");
    assert_eq!(progress.last(), Some(&1.0));
    // The private outdir is gone; only the profile stays behind for next time.
    let work: Vec<_> = fs::read_dir(tmp.path().join("work")).unwrap().collect();
    assert!(work.is_empty(), "leftovers in the work dir: {work:?}");
    assert!(
        tmp.path()
            .join("data/libreoffice-profiles/worker-0")
            .is_dir()
    );
}

#[test]
fn odt_to_docx_is_a_zip() {
    require_engine!("soffice");
    let tmp = tempfile::tempdir().unwrap();
    let engine = SofficeEngine::new_in(tmp.path().join("data"));
    let written = convert(
        &engine,
        &step("sample.odt", "odt", "docx", tmp.path()),
        tmp.path(),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(first_bytes(&written[0], 2), b"PK");
}

#[test]
fn two_conversions_at_once_both_succeed() {
    require_engine!("soffice");
    let tmp = tempfile::tempdir().unwrap();
    let engine = SofficeEngine::new_in(tmp.path().join("data"));
    let cancel = CancelToken::new();
    let jobs = [("sample.docx", "docx", "a"), ("sample.odt", "odt", "b")];
    let results: Vec<_> = thread::scope(|s| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(input, from, name)| {
                let engine = &engine;
                let cancel = &cancel;
                let out_dir = tmp.path().join(name);
                fs::create_dir_all(&out_dir).unwrap();
                s.spawn(move || {
                    convert(
                        engine,
                        &step(input, from, "pdf", &out_dir.join("work")),
                        &out_dir,
                        cancel,
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for result in results {
        let written = result.unwrap();
        assert_eq!(first_bytes(&written[0], 4), b"%PDF");
    }
    // Each of them had a profile of its own.
    for worker in ["worker-0", "worker-1"] {
        assert!(
            tmp.path()
                .join("data/libreoffice-profiles")
                .join(worker)
                .is_dir()
        );
    }
}

#[test]
fn three_engines_on_one_data_dir_all_succeed() {
    // Separate engines are what separate MorphRing processes look like (the
    // app, a CLI run and a `watch`): one pool each, one shared profiles
    // folder. Before the profile locks every one of them grabbed worker-0,
    // and LibreOffice let exactly one of them have it.
    require_engine!("soffice");
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let engines = [
        SofficeEngine::new_in(&data),
        SofficeEngine::new_in(&data),
        SofficeEngine::new_in(&data),
    ];
    let cancel = CancelToken::new();
    // Warm worker-0 up first. The clash needs a profile that already exists,
    // which is the everyday case, not the first-ever conversion.
    let warm = tmp.path().join("warm");
    convert(
        &engines[0],
        &step("sample.docx", "docx", "pdf", &warm.join("work")),
        &warm,
        &cancel,
    )
    .unwrap();
    // Everybody leaves the starting line together, like three impatient
    // processes started from three terminals at once.
    let start = std::sync::Barrier::new(engines.len());
    let results: Vec<_> = thread::scope(|s| {
        let handles: Vec<_> = engines
            .iter()
            .enumerate()
            .map(|(i, engine)| {
                let (cancel, start) = (&cancel, &start);
                let out_dir = tmp.path().join(format!("out{i}"));
                fs::create_dir_all(&out_dir).unwrap();
                s.spawn(move || {
                    start.wait();
                    convert(
                        engine,
                        &step("sample.docx", "docx", "pdf", &out_dir.join("work")),
                        &out_dir,
                        cancel,
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for result in results {
        let written = result.unwrap();
        assert_eq!(first_bytes(&written[0], 4), b"%PDF");
    }
    // Three overlapping runs, three different profiles.
    for worker in ["worker-0", "worker-1", "worker-2"] {
        assert!(data.join("libreoffice-profiles").join(worker).is_dir());
    }
}

#[test]
fn skipped_output_runs_nothing() {
    require_engine!("soffice");
    let tmp = tempfile::tempdir().unwrap();
    let engine = SofficeEngine::new_in(tmp.path().join("data"));
    let mut out = DirAllocator::new(tmp.path(), "result");
    out.skip = true;
    let written = engine
        .convert(
            &step("sample.docx", "docx", "pdf", tmp.path()),
            &mut out,
            &|_| {},
            &CancelToken::new(),
        )
        .unwrap();
    assert!(written.is_empty());
    assert!(!tmp.path().join("result.pdf").exists());
    assert!(!tmp.path().join("data").exists());
}

#[test]
fn unreadable_input_fails_with_a_message() {
    require_engine!("soffice");
    let tmp = tempfile::tempdir().unwrap();
    let engine = SofficeEngine::new_in(tmp.path().join("data"));
    let mut bad = step("sample.docx", "docx", "pdf", tmp.path());
    bad.input = tmp.path().join("missing.docx");
    let err = convert(&engine, &bad, tmp.path(), &CancelToken::new()).unwrap_err();
    assert!(
        matches!(&err, EngineError::Failed(msg) if msg.contains("LibreOffice produced no output")),
        "{err:?}"
    );
    assert!(!tmp.path().join("result.pdf").exists());
}

#[test]
fn cancel_kills_soffice_quickly() {
    require_engine!("soffice");
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let needle = data.to_string_lossy().into_owned();
    let engine = SofficeEngine::new_in(&data);
    let cancel = CancelToken::new();

    let (result, waited) = thread::scope(|s| {
        let worker = s.spawn(|| {
            convert(
                &engine,
                &step("sample.docx", "docx", "pdf", &tmp.path().join("work")),
                tmp.path(),
                &cancel,
            )
        });
        // Wait until soffice is really running with our profile, so we cancel a
        // live process and not a thread still looking for its shoes. A fresh
        // profile takes seconds to build, which is plenty of window.
        let deadline = Instant::now() + Duration::from_secs(20);
        while processes_mentioning(&needle).is_empty() && !worker.is_finished() {
            assert!(Instant::now() < deadline, "soffice never started");
            thread::sleep(Duration::from_millis(20));
        }
        let started = Instant::now();
        cancel.cancel();
        let result = worker.join().unwrap();
        (result, started.elapsed())
    });

    assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
    assert!(waited < Duration::from_secs(2), "cancel took {waited:?}");
    assert!(!tmp.path().join("result.pdf").exists());
    let left = processes_mentioning(&needle);
    assert!(left.is_empty(), "soffice survived the cancel: {left:?}");
}
