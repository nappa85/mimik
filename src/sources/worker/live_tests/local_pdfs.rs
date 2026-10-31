//! Owner-local acceptance: no PDF contents or output images leave memory.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;

#[test]
#[ignore = "owner opt-in: MIMIK_LIVE_LOCAL_PDFS (colon-separated) and MIMIK_LIVE_RUNNER"]
fn live_local_pdfs_inspect_and_render_every_page() {
    let Some(paths) = std::env::var_os("MIMIK_LIVE_LOCAL_PDFS") else {
        return;
    };
    assert!(enabled(), "local PDF checks require MIMIK_LIVE_RUNNER");
    assert!(
        !paths.is_empty(),
        "local PDF opt-in must name at least one path"
    );
    let mut all_passed = true;
    for path in std::env::split_paths(&paths) {
        assert!(
            !path.as_os_str().is_empty(),
            "local PDF paths must not be empty"
        );
        // Hash the path bytes, without reading the document into the parent or logging its name.
        let id = hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()));
        let opts = production_options();
        let start = Instant::now();
        let inspected = inspection(&path, &opts, &Cancel::default());
        let mut wait4 = opts.peak_rss_kib.load(Ordering::Acquire);
        let mut worker = opts.worker_peak_rss_kib.load(Ordering::Acquire);
        let mut passed = rss_within_bound(&opts);
        let mut batches = Vec::new();
        let mut page_seconds = Vec::new();
        let pages = inspected.as_ref().map_or(0, |report| report.document.pages);
        let inspect_outcome = inspected.as_ref().map(|_| ()).map_err(Clone::clone);
        if let Ok(report) = inspected {
            for (index, batch) in report.pages.chunks(8).enumerate() {
                let spec = Spec {
                    operation: records::Operation::Render,
                    pages: batch.iter().map(|page| page.n).collect(),
                    settings: RenderSettings::default(),
                    input: "/in/document.pdf".into(),
                };
                let batch_start = Instant::now();
                let result = execute(&path, &spec, &Cancel::default(), &opts, |pipe| {
                    let mut reader = records::RenderReader::new(pipe, batch.to_vec()).unwrap();
                    let result = (|| {
                        let mut previous = batch_start;
                        while let Some(page) = reader.next_page()? {
                            // Includes first-page launch cost; images are immediately dropped.
                            let now = Instant::now();
                            page_seconds.push((page.n, now.duration_since(previous).as_secs_f64()));
                            previous = now;
                        }
                        Ok(())
                    })();
                    (result, reader.terminal_error())
                });
                passed &= result.is_ok() && rss_within_bound(&opts);
                wait4 = wait4.max(opts.peak_rss_kib.load(Ordering::Acquire));
                worker = worker.max(opts.worker_peak_rss_kib.load(Ordering::Acquire));
                batches.push((index + 1, batch_start.elapsed().as_secs_f64(), result));
            }
            passed &= page_seconds.len() == report.pages.len();
        } else {
            passed = false;
        }
        println!(
            "local PDF {}: pages={pages}, total_seconds={:.3}, per_page_seconds={page_seconds:?}, wait4_peak_rss_kib={wait4} (natural-exit worker upper bound only), sampled_worker_vmhwm_kib={worker} (before parent kill), inspect={inspect_outcome:?}, batches={batches:?}",
            &id[..12],
            start.elapsed().as_secs_f64(),
        );
        all_passed &= passed;
    }
    assert!(
        all_passed,
        "local PDF acceptance failed; see hashed summaries"
    );
}
