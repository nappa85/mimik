//! SRC-15 slice 6: the large-document gate. Opt-in (`MIMIK_LIVE_RUNNER` plus
//! `MIMIK_LIVE_LARGE_DIR`, a directory on the slow disk); `MIMIK_LIVE_REPORT` names a file that
//! receives one JSON line per measurement, with the build profile. Every input is generated,
//! deterministically, and kept in the large directory; a file is reused when its SHA-256 equals
//! that of the freshly built bytes. The runs go through the production options (budgets
//! included), so a budget that is too tight fails here. CPU time is the `wait4` rusage of the
//! runner and the descendants it waited for.
//!
//! The tests take [`serial`]: one sandbox tree at a time, so the process-tree checks are exact.
use super::corpus::check_rss;
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs::File,
    io::Write,
    sync::{Mutex, MutexGuard, PoisonError},
};

/// The directory for generated documents and staging. These tests run only when asked for, so a
/// missing variable is a failure, never a silent pass.
pub(crate) fn large_dir() -> PathBuf {
    assert!(
        enabled(),
        "MIMIK_LIVE_RUNNER (the built mimik binary) is required for the large-document gate"
    );
    let dir = PathBuf::from(std::env::var_os("MIMIK_LIVE_LARGE_DIR").expect(
        "MIMIK_LIVE_LARGE_DIR (a directory on the slow disk) is required for the large-document gate",
    ));
    std::fs::create_dir_all(&dir).expect("MIMIK_LIVE_LARGE_DIR is not creatable");
    dir
}

/// Held by every gate test: the CPU figures come from the process's waited-for children.
pub(crate) fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Print a measurement and append it to `MIMIK_LIVE_REPORT` as one JSON line. The build profile
/// of the test binary is added to every line.
pub(crate) fn report(value: &serde_json::Value) {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "profile".into(),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
        );
    }
    let line = value.to_string();
    println!("LIVE_LARGE {line}");
    if let Some(path) = std::env::var_os("MIMIK_LIVE_REPORT") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("MIMIK_LIVE_REPORT is not writable");
        writeln!(file, "{line}").expect("report line");
    }
}

/// Every process on the host as (pid, parent pid, start time in clock ticks), from `/proc`.
fn processes() -> Vec<(u32, u32, u64)> {
    std::fs::read_dir("/proc")
        .expect("/proc")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            // The command name may hold spaces and parentheses: fields start after the last `)`.
            let rest = &stat[stat.rfind(')')? + 2..];
            let fields: Vec<&str> = rest.split(' ').collect();
            // Field 3 (state) is index 0: the parent is field 4, the start time field 22.
            Some((
                pid,
                fields.get(1)?.parse().ok()?,
                fields.get(19)?.parse().ok()?,
            ))
        })
        .collect()
}

/// Every descendant of this process, by pid and start time (so a reused pid is not confused
/// with it). Take this while a sandbox tree is alive and hand it to [`assert_gone`].
pub(crate) fn descendants() -> Vec<(u32, u64)> {
    descendants_of(std::process::id())
}

fn descendants_of(root: u32) -> Vec<(u32, u64)> {
    let all = processes();
    let mut found: Vec<(u32, u64)> = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid, start) in &all {
            if ppid == parent && !found.contains(&(pid, start)) {
                found.push((pid, start));
                frontier.push(pid);
            }
        }
    }
    found
}

/// None of the recorded processes may still exist (a zombie that nobody reaped counts).
pub(crate) fn assert_gone(recorded: &[(u32, u64)]) {
    let now = processes();
    let alive: Vec<_> = recorded
        .iter()
        .filter(|(pid, start)| now.iter().any(|&(p, _, s)| p == *pid && s == *start))
        .collect();
    assert!(alive.is_empty(), "sandbox processes remain: {alive:?}");
}

/// Peak RSS of a finished run: the worker's own high-water mark when the parent killed it, the
/// `wait4` figure otherwise.
pub(super) fn peak_rss_kib(opts: &Options) -> u64 {
    use std::sync::atomic::Ordering;
    if opts.parent_killed.load(Ordering::Acquire) {
        opts.worker_peak_rss_kib.load(Ordering::Acquire)
    } else {
        opts.peak_rss_kib.load(Ordering::Acquire)
    }
}

/// Make the page cache cold for `path`: flush its dirty pages, then drop its whole length.
pub(crate) fn cold(path: &Path) {
    let file = File::open(path).expect("open for fadvise");
    file.sync_all().expect("fsync");
    rustix::fs::fadvise(&file, 0, None, rustix::fs::Advice::DontNeed).expect("posix_fadvise");
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    /// About 40 text lines a page with a running header, footer, page number and an image
    /// every fifth page.
    Manual(u32),
    /// Pages with a media box and nothing else: the 20,000-page frame.
    Minimal(u32),
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).expect("31 bits")
    }
}

const WORDS: [&str; 32] = [
    "the",
    "party",
    "enters",
    "a",
    "dark",
    "hall",
    "where",
    "stone",
    "doors",
    "open",
    "slowly",
    "roll",
    "initiative",
    "and",
    "describe",
    "your",
    "action",
    "to",
    "the",
    "warden",
    "of",
    "keep",
    "torch",
    "burns",
    "low",
    "rope",
    "ladder",
    "chest",
    "key",
    "ward",
    "ancient",
    "rune",
];
const IMAGE: usize = 100;

struct Builder {
    out: Vec<u8>,
    offsets: Vec<usize>,
}
impl Builder {
    fn object(&mut self, id: usize, body: &[u8]) {
        self.offsets[id] = self.out.len();
        writeln!(self.out, "{id} 0 obj").unwrap();
        self.out.extend_from_slice(body);
        self.out.extend_from_slice(b"\nendobj\n");
    }
    fn stream(&mut self, id: usize, dictionary: &str, data: &[u8]) {
        let mut body = format!("<< {dictionary} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.object(id, &body);
    }
}

fn page_text(number: u32, lcg: &mut Lcg) -> Vec<u8> {
    let mut text = String::from("BT /F1 9 Tf 40 815 Td (The Sunken Keep - Player Manual) Tj ET\n");
    text.push_str("BT /F1 10 Tf 40 780 Td 12 TL\n");
    for _ in 0..40 {
        let mut line = String::new();
        while line.len() < 72 {
            line.push_str(WORDS[lcg.next() % WORDS.len()]);
            line.push(' ');
        }
        writeln!(text, "({}) Tj T*", line.trim_end()).unwrap();
    }
    text.push_str("ET\n");
    write!(
        text,
        "BT /F1 9 Tf 40 24 Td (Chapter notes and tables) Tj ET\nBT /F1 9 Tf 520 24 Td (Page {number}) Tj ET\n"
    )
    .unwrap();
    text.into_bytes()
}

fn build(kind: Kind) -> Vec<u8> {
    let (count, manual) = match kind {
        Kind::Manual(count) => (count, true),
        Kind::Minimal(count) => (count, false),
    };
    // Object numbers: 1 catalog, 2 page tree, 3 font, then per page its page, content, image.
    let mut ids = Vec::new();
    let mut next = 4_usize;
    for number in 1..=count {
        let page = next;
        let image = (manual && number % 5 == 3).then_some(next + 2);
        next += if manual {
            2 + usize::from(image.is_some())
        } else {
            1
        };
        ids.push((page, image));
    }
    let mut b = Builder {
        out: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
        offsets: vec![0; next],
    };
    b.object(1, b"<< /Type /Catalog /Pages 2 0 R >>");
    let mut kids = String::new();
    for (page, _) in &ids {
        write!(kids, "{page} 0 R ").unwrap();
    }
    b.object(
        2,
        format!("<< /Type /Pages /Count {count} /Kids [ {kids}] >>").as_bytes(),
    );
    b.object(
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    let mut lcg = Lcg(0x5eed_1234_abcd_0001);
    for (index, &(page, image)) in ids.iter().enumerate() {
        let number = u32::try_from(index + 1).expect("page count");
        if !manual {
            b.object(
                page,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
            );
            continue;
        }
        let xobject = image.map_or(String::new(), |id| format!("/XObject << /Im1 {id} 0 R >>"));
        b.object(
            page,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> {xobject} >> /Contents {} 0 R >>",
                page + 1
            )
            .as_bytes(),
        );
        let mut content = page_text(number, &mut lcg);
        if image.is_some() {
            content.extend_from_slice(b"q 200 0 0 200 350 400 cm /Im1 Do Q\n");
        }
        b.stream(page + 1, "", &content);
        if let Some(id) = image {
            let data: Vec<u8> = (0..IMAGE * IMAGE)
                .map(|_| u8::try_from(lcg.next() & 0xff).expect("byte"))
                .collect();
            b.stream(
                id,
                &format!(
                    "/Type /XObject /Subtype /Image /Width {IMAGE} /Height {IMAGE} /ColorSpace /DeviceGray /BitsPerComponent 8"
                ),
                &data,
            );
        }
    }
    let table = b.out.len();
    write!(b.out, "xref\n0 {next}\n0000000000 65535 f \n").unwrap();
    for id in 1..next {
        writeln!(b.out, "{:010} 00000 n ", b.offsets[id]).unwrap();
    }
    write!(
        b.out,
        "trailer\n<< /Size {next} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n"
    )
    .unwrap();
    b.out
}

fn sha256_file(path: &Path) -> String {
    let mut hasher = Sha256::new();
    let mut file = File::open(path).expect("open");
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer).expect("read");
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hex::encode(hasher.finalize())
}

/// The generated document `name` in `dir` and its SHA-256: an existing regular file is reused
/// when its hash equals that of the freshly built bytes, otherwise it is replaced. The new file
/// is written to a unique temporary file and renamed over the name, so a symlink at the name is
/// replaced, never followed.
pub(crate) fn ensure_pdf(dir: &Path, name: &str, kind: Kind) -> (PathBuf, String) {
    let path = dir.join(name);
    let bytes = build(kind);
    let expected = hex::encode(Sha256::digest(&bytes));
    let regular = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
    if regular && sha256_file(&path) == expected {
        return (path, expected);
    }
    let mut file = tempfile::NamedTempFile::new_in(dir).expect("temporary file");
    file.write_all(&bytes).expect("write");
    file.as_file().sync_all().expect("fsync");
    file.persist(&path).expect("persist");
    (path, expected)
}

fn seconds(value: Duration) -> f64 {
    (value.as_secs_f64() * 1000.).round() / 1000.
}

/// CPU time of the finished run's tree (the runner's `wait4` rusage).
fn cpu_of(opts: &Options) -> Duration {
    Duration::from_millis(opts.cpu_ms.load(std::sync::atomic::Ordering::Acquire))
}

fn ratio(value: Duration, limit_seconds: f64) -> f64 {
    (value.as_secs_f64() / limit_seconds * 1000.).round() / 1000.
}

/// Inspect `path` cold under the production budgets and report the run: figures and their
/// ratio to the limit. A budget too tight fails the run itself (the worker is killed).
fn inspect_cold(name: &str, path: &Path, sha256: &str) -> Inspection {
    cold(path);
    let opts = Options::production().expect("production options");
    let size = std::fs::metadata(path).unwrap().len();
    let started = Instant::now();
    let inspected = super::launch::inspect_run(path, None, &Cancel::default(), &opts)
        .unwrap_or_else(|reason| panic!("{name}: inspection failed under its budgets: {reason:?}"));
    let wall = started.elapsed();
    let cpu = cpu_of(&opts);
    check_rss(name, "inspect", &opts);
    let pages = inspected.document.pages;
    let wall_limit = super::launch::inspect_wall_seconds(size, u64::from(pages));
    #[allow(clippy::cast_precision_loss)]
    let cpu_limit = super::launch::inspect_cpu_seconds(size) as f64;
    report(&serde_json::json!({
        "test": "inspect", "name": name, "sha256": sha256, "cache": "cold",
        "pages": pages, "file_bytes": size,
        "wall_s": seconds(wall), "cpu_s": seconds(cpu), "peak_rss_kib": peak_rss_kib(&opts),
        "parent_killed": opts.parent_killed.load(std::sync::atomic::Ordering::Acquire),
        "wall_limit_s": wall_limit, "wall_ratio": ratio(wall, wall_limit),
        "cpu_limit_s": cpu_limit, "cpu_ratio": ratio(cpu, cpu_limit),
    }));
    assert_eq!(inspected.pages.len(), pages as usize, "{name}");
    inspected
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER and MIMIK_LIVE_LARGE_DIR opt-in"]
fn live_large_inspect_500_5000_and_20000_pages() {
    let dir = large_dir();
    let _serial = serial();
    for (name, kind, pages) in [
        ("manual-500.pdf", Kind::Manual(500), 500),
        ("manual-5000.pdf", Kind::Manual(5000), 5000),
        ("minimal-20000.pdf", Kind::Minimal(20_000), 20_000),
    ] {
        let (path, sha256) = ensure_pdf(&dir, name, kind);
        assert_eq!(
            inspect_cold(name, &path, &sha256).document.pages,
            pages,
            "{name}"
        );
    }
}

/// Every page of `name` in the production 8-page batches, cold, against the per-page budgets.
fn render_every_page(dir: &Path, name: &str, kind: Kind) {
    let (path, sha256) = ensure_pdf(dir, name, kind);
    let inspected = inspect_cold(name, &path, &sha256);
    let (mut batches, mut wall_total, mut cpu_total) = (0_u32, Duration::ZERO, Duration::ZERO);
    let (mut wall_max, mut cpu_max, mut rss_max) = (Duration::ZERO, Duration::ZERO, 0_u64);
    let (mut wall_ratio, mut cpu_ratio, mut killed) = (0_f64, 0_f64, 0_u32);
    let (mut png_bytes, mut lines, mut rendered) = (0_u64, 0_usize, 0_usize);
    for batch in inspected.pages.chunks(8) {
        cold(&path);
        let opts = Options::production().expect("production options");
        let started = Instant::now();
        let mut got = 0_usize;
        super::launch::render_run(
            &path,
            None,
            batch.to_vec(),
            &Cancel::default(),
            &opts,
            |page| {
                got += 1;
                png_bytes += page.png.len() as u64;
                lines += page.lines.len();
                Ok(())
            },
        )
        .unwrap_or_else(|reason| {
            panic!(
                "{name}: render of pages {}.. failed: {reason:?}",
                batch[0].n
            )
        });
        let (wall, cpu) = (started.elapsed(), cpu_of(&opts));
        assert_eq!(
            got,
            batch.len(),
            "{name}: every page of the batch is delivered"
        );
        // The limits (30 s wall, 25 s CPU a page) are enforced by the run itself; the ratios
        // show how close the batches come.
        let count = f64::from(u32::try_from(batch.len()).expect("batch"));
        wall_ratio = wall_ratio.max(ratio(wall, 30. * count));
        cpu_ratio = cpu_ratio.max(ratio(cpu, 25. * count));
        killed += u32::from(
            opts.parent_killed
                .load(std::sync::atomic::Ordering::Acquire),
        );
        assert!(rss_within_bound(&opts), "{name}: RSS bound");
        batches += 1;
        rendered += got;
        wall_total += wall;
        cpu_total += cpu;
        wall_max = wall_max.max(wall);
        cpu_max = cpu_max.max(cpu);
        rss_max = rss_max.max(peak_rss_kib(&opts));
    }
    assert_eq!(rendered, inspected.pages.len(), "{name}");
    report(&serde_json::json!({
        "test": "render_all", "name": name, "sha256": sha256, "cache": "cold per batch",
        "pages": rendered, "batches": batches, "parent_killed_batches": killed,
        "wall_ratio_max": wall_ratio, "cpu_ratio_max": cpu_ratio,
        "wall_total_s": seconds(wall_total), "cpu_total_s": seconds(cpu_total),
        "wall_max_batch_s": seconds(wall_max), "cpu_max_batch_s": seconds(cpu_max),
        "peak_rss_max_kib": rss_max, "png_bytes": png_bytes, "text_lines": lines,
        "wall_per_page_s": seconds(wall_total / u32::try_from(rendered).expect("pages")),
        "cpu_per_page_s": seconds(cpu_total / u32::try_from(rendered).expect("pages")),
    }));
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER and MIMIK_LIVE_LARGE_DIR opt-in"]
fn live_large_render_every_page_of_500_pages() {
    let dir = large_dir();
    let _serial = serial();
    render_every_page(&dir, "manual-500.pdf", Kind::Manual(500));
}

#[test]
#[ignore = "host confinement: MIMIK_LIVE_RUNNER and MIMIK_LIVE_LARGE_DIR opt-in"]
fn live_large_render_every_page_of_5000_pages() {
    let dir = large_dir();
    let _serial = serial();
    render_every_page(&dir, "manual-5000.pdf", Kind::Manual(5000));
}

#[test]
fn the_generated_documents_are_deterministic_and_reused_by_their_hash() {
    let dir = tempfile::tempdir().unwrap();
    let (first, hash) = ensure_pdf(dir.path(), "m.pdf", Kind::Manual(12));
    assert_eq!(hash, sha256_file(&first));
    let bytes = std::fs::read(&first).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7"));
    assert_eq!(bytes, build(Kind::Manual(12)));
    // Pages 3 and 8 carry an image; the count and the tree agree.
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("/Subtype /Image").count(), 2);
    assert!(text.contains("/Count 12"));
    assert!(text.contains("(Page 12)"));
    // A file with the right hash is reused untouched; a damaged one is rebuilt.
    let stamp = std::fs::metadata(&first).unwrap().modified().unwrap();
    assert_eq!(
        ensure_pdf(dir.path(), "m.pdf", Kind::Manual(12)),
        (first.clone(), hash.clone())
    );
    assert_eq!(
        std::fs::metadata(&first).unwrap().modified().unwrap(),
        stamp
    );
    std::fs::write(&first, b"damaged").unwrap();
    ensure_pdf(dir.path(), "m.pdf", Kind::Manual(12));
    assert_eq!(std::fs::read(&first).unwrap(), bytes);
    // A symlink at the name is replaced, and what it pointed at is left alone.
    let target = dir.path().join("elsewhere");
    std::fs::write(&target, b"keep").unwrap();
    std::fs::remove_file(&first).unwrap();
    std::os::unix::fs::symlink(&target, &first).unwrap();
    ensure_pdf(dir.path(), "m.pdf", Kind::Manual(12));
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    assert!(
        std::fs::symlink_metadata(&first)
            .unwrap()
            .file_type()
            .is_file()
    );
    assert_eq!(std::fs::read(&first).unwrap(), bytes);
    let minimal = build(Kind::Minimal(5));
    assert!(String::from_utf8_lossy(&minimal).contains("/Count 5"));
    assert!(!String::from_utf8_lossy(&minimal).contains("/Contents"));
}

#[test]
fn the_process_tree_check_sees_grandchildren_and_notices_when_they_are_gone() {
    // A shell that starts a grandchild and waits for it.
    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 30 & wait"])
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let tree = loop {
        let tree = descendants_of(child.id());
        if !tree.is_empty() {
            break tree;
        }
        assert!(Instant::now() < until, "grandchild never appeared");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        std::panic::catch_unwind(|| assert_gone(&tree)).is_err(),
        "live processes must be reported"
    );
    // Only the grandchild is killed; the shell then ends by itself.
    for (pid, _) in &tree {
        let pid = rustix::process::Pid::from_raw(i32::try_from(*pid).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    }
    child.wait().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while std::panic::catch_unwind(|| assert_gone(&tree)).is_err() {
        assert!(Instant::now() < until, "processes did not go");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn every_cross_reference_offset_of_a_generated_document_points_at_its_object() {
    for kind in [Kind::Manual(12), Kind::Minimal(30)] {
        let bytes = build(kind);
        let table = bytes.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
        let text = String::from_utf8_lossy(&bytes[table..]).into_owned();
        let entries: Vec<&str> = text.lines().skip(3).take_while(|l| l.len() == 19).collect();
        assert!(entries.len() >= 29);
        for (index, entry) in entries.iter().enumerate() {
            let offset: usize = entry[..10].parse().unwrap();
            assert!(
                bytes[offset..].starts_with(format!("{} 0 obj", index + 1).as_bytes()),
                "object {}",
                index + 1
            );
        }
        let start: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(start, table);
    }
}
