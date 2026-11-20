//! SRC-15: the worker opens the bound file by path, so a PDF's size adds no address space.
//! Every input is generated; sparse files take no disk and nothing is retained.
use super::corpus::check_rss;
use super::*;
use crate::sources::worker::tests::{gzip_pdf_disguise, sparse_pdf};

/// A quarter of `RLIMIT_AS`: far below the 75% gate, and far above what a seeking reader of a
/// one-page document needs. A copy of the file in memory would exceed it for the 4.5 GiB case.
const SMALL_RSS_KIB: u64 = 512 * 1024;

fn assert_small_rss(name: &str, opts: &Options) {
    use std::sync::atomic::Ordering;
    let measured = if opts.parent_killed.load(Ordering::Acquire) {
        opts.worker_peak_rss_kib.load(Ordering::Acquire)
    } else {
        opts.peak_rss_kib.load(Ordering::Acquire)
    };
    assert!(
        measured > 0 && measured < SMALL_RSS_KIB,
        "{name}: peak RSS {measured} KiB must be below {SMALL_RSS_KIB} KiB"
    );
}

fn render_first(path: &Path, pages: Vec<Page>, opts: &Options) -> Result<(), Reason> {
    let spec = Spec {
        operation: records::Operation::Render,
        pages: vec![1],
        settings: RenderSettings::default(),
        input: "/in/document.pdf".into(),
    };
    execute(path, &spec, &Cancel::default(), opts, |pipe| {
        let mut reader = records::RenderReader::new(pipe, pages).unwrap();
        let result = (|| {
            assert!(reader.next_page()?.is_some());
            assert!(reader.next_page()?.is_none());
            Ok(())
        })();
        (result, reader.terminal_error())
    })
}

/// Inspect and render page 1 of a sparse document with the production budgets.
fn sparse_case(name: &str, pad: u64) {
    let file = sparse_pdf(pad);
    let length = file.as_file().metadata().unwrap().len();
    assert!(length > pad);
    let opts = production_options();
    let inspected = inspection(file.path(), &opts, &Cancel::default()).unwrap();
    assert_eq!(inspected.document.pages, 1, "{name}");
    check_rss(name, "inspect", &opts);
    assert_small_rss(name, &opts);
    let opts = production_options();
    render_first(file.path(), inspected.pages, &opts).unwrap();
    check_rss(name, "render", &opts);
    assert_small_rss(name, &opts);
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_pdf_over_128_mib_is_opened_by_path() {
    if !enabled() {
        return;
    }
    sparse_case("sparse-129-MiB", 129 << 20);
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_xref_beyond_4_gib_is_read_with_64_bit_seeks() {
    if !enabled() {
        return;
    }
    sparse_case("sparse-4.5-GiB", 4608 << 20);
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_gzip_with_pdf_markers_is_refused_not_decompressed() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("document.pdf");
    std::fs::write(&path, gzip_pdf_disguise()).unwrap();
    let opts = production_options();
    // Decompressed, the stream is a valid one-page PDF; refused, it is not a PDF at all.
    assert_eq!(
        inspection(&path, &opts, &Cancel::default()).unwrap_err(),
        Reason::Document(Error::Unreadable)
    );
    check_rss("gzip-disguise", "inspect", &opts);
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER opt-in"]
fn live_missing_input_is_unreadable_and_starts_nothing() {
    if !enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let opts = production_options();
    let started = Instant::now();
    assert_eq!(
        inspection(&dir.path().join("absent.pdf"), &opts, &Cancel::default()).unwrap_err(),
        Reason::Document(Error::Unreadable)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}
