//! A shared library the model server needs and the host lacks (`libgomp.so.1` on a minimal
//! Ubuntu server): named before the sandbox starts, so the page can say which library and what
//! to install instead of reporting a server that never answered `/health` with nothing on its
//! error output. It is a best-effort diagnostic, not the loader: when it cannot tell, it says
//! nothing and the start goes on as it would without it.
//!
//! Nothing is executed. Every ELF file of the runtime build is inspected, the server and each
//! library and each backend the server loads with `dlopen` (`libggml-cpu-*.so` links the OpenMP
//! runtime, `libggml-vulkan.so` the Vulkan loader), by reading its `DT_NEEDED` names from the
//! ELF dynamic section (the `object` crate, DEP-26) out of the descriptor the runtime check
//! opened and hashed, so what is read is that file whatever happens to its path afterwards.
//! Non-ELF files (the licence) are skipped; a file larger than 256 MiB, unreadable, or an ELF
//! that cannot be parsed makes the answer "unknown".
//!
//! A name is found when it is another file of the build, or a library the sandbox could load
//! from the folders it binds ([`bwrap::LIBRARY_DIRS`]): in the loader's own default folders
//! (those folders and the multiarch ones directly under them), or at the path `ld.so.cache`
//! gives for it (the cache is parsed read-only), and in each case a regular file (or a link that
//! resolves) that is an ELF of the same class and machine as the file that needs it. A name that
//! is found nowhere is reported, but only when the search was complete; a name whose only
//! candidates lie in other places, or a search that hit its budget, met an unreadable folder or
//! was cancelled, is "unknown", never "missing".
use super::bwrap::LIBRARY_DIRS;
use super::files::Verified;
use object::read::elf::{Dyn, FileHeader};
use object::{Endianness, elf};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Read;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};

/// The most a library file may weigh for its dynamic section to be read (a bound on memory).
pub(crate) const MAX_BYTES: u64 = 256 << 20;

/// How far the search for a name outside the loader's default folders goes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    /// Directory entries looked at, over the whole search.
    pub entries: usize,
    /// Folders opened.
    pub dirs: usize,
    /// Candidate files opened to check them (from the default folders, the cache and the walk).
    pub opens: usize,
    /// Folders down from a library directory (`/usr/lib/gcc/x86_64-pc-linux-gnu/15` is 4; the
    /// deepest on a full Gentoo `/usr/lib` is 12).
    pub depth: usize,
}

impl Budget {
    pub(crate) const DEFAULT: Self = Self {
        entries: 1_500_000,
        dirs: 100_000,
        opens: 4_096,
        depth: 24,
    };
}

/// The identity of a file for compatibility: ELF class and machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Kind {
    machine: u16,
}

/// The kind of a 64-bit little-endian ELF from its first bytes (`None`: not one).
fn kind_of(head: &[u8]) -> Option<Kind> {
    if head.len() < 20 || &head[..4] != b"\x7fELF" || head[4] != 2 || head[5] != 1 {
        return None;
    }
    Some(Kind {
        machine: u16::from_le_bytes([head[18], head[19]]),
    })
}

/// Opens `path` without ever blocking (a FIFO, or a link to one, opens at once and is then
/// refused) and gives the file only if the descriptor is a regular file.
fn open_regular(path: &Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

/// Whether `path` is a regular file (a link must resolve to one) that is an ELF of `kind`.
fn compatible(path: &Path, kind: Kind) -> bool {
    let Some(file) = open_regular(path) else {
        return false;
    };
    let mut head = [0_u8; 20];
    file.take(20).read_exact(&mut head).is_ok() && kind_of(&head) == Some(kind)
}

/// The `DT_NEEDED` names of the 64-bit ELF `data`: `None` when it cannot be read as one; an
/// ELF without a dynamic section (a static one) needs nothing.
pub(crate) fn needed(data: &[u8]) -> Option<Vec<String>> {
    let header = elf::FileHeader64::<Endianness>::parse(data).ok()?;
    let endian = header.endian().ok()?;
    let sections = header.sections(endian, data).ok()?;
    // The section index returned with the entries is that of their string table.
    let Some((entries, strings_index)) = sections.dynamic(endian, data).ok()? else {
        return Some(Vec::new());
    };
    let strings = sections.strings(endian, data, strings_index).ok()?;
    let mut names = Vec::new();
    for entry in entries {
        if entry.tag(endian) == elf::DT_NEEDED {
            let name = entry.string(endian, strings).ok()?;
            names.push(String::from_utf8(name.to_vec()).ok()?);
        }
    }
    Some(names)
}

/// A plain library file name: nothing a path, a page or a command line would read as more.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

/// Reads `size` bytes of the descriptor from offset 0 (its own offset is left alone).
pub(crate) fn read_fd(fd: BorrowedFd<'_>, size: u64) -> Option<Vec<u8>> {
    if size > MAX_BYTES {
        return None;
    }
    let file = std::fs::File::from(fd.try_clone_to_owned().ok()?);
    let mut data = vec![0; usize::try_from(size).ok()?];
    file.read_exact_at(&mut data, 0).ok()?;
    Some(data)
}

/// The most of `ld.so.cache` that is read.
pub(crate) const CACHE_MAX: u64 = 64 << 20;
/// The longest name or path in the cache that is accepted.
const STRING_MAX: usize = 4096;
/// The most string bytes looked at over one parse (entries may share or overlap strings, so
/// this bounds the work a crafted cache can cause).
const PARSE_WORK: usize = 16 << 20;
/// The most entries a cache may declare (a real one has thousands; parsed entries cost memory,
/// so a cache this large is not read).
const CACHE_ENTRIES: usize = 100_000;

/// `(name, path)` of each library in a glibc `ld.so.cache`. The container is checked: the new
/// format at the start, or after the old format's own table (whose declared size locates it),
/// and the new header's declared entry table and string table must lie inside the file. `None`
/// when the cache is not one this recognises or is not sound: a bad container, an entry or
/// string outside its table, a string over [`STRING_MAX`], more than [`CACHE_ENTRIES`] entries
/// or [`PARSE_WORK`] of strings, or a cancel. `Some` with no entries is a cache that parsed and
/// is empty.
pub(crate) fn parse_cache(
    data: &[u8],
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<(String, PathBuf)>> {
    const OLD: &[u8] = b"ld.so-1.7.0";
    const MAGIC: &[u8] = b"glibc-ld.so.cache1.1";
    const HEADER: usize = 48;
    const ENTRY: usize = 24;
    let cache = if data.starts_with(MAGIC) {
        data
    } else if data.starts_with(OLD) {
        // The old header (magic, padding, a count) and its 12-byte entries, then the new cache
        // at the next 8-byte boundary.
        let nlibs = u32::from_le_bytes(data.get(12..16)?.try_into().ok()?) as usize;
        let end = nlibs.checked_mul(12)?.checked_add(16)?;
        let rest = data.get(end.checked_add(7)? & !7..)?;
        if !rest.starts_with(MAGIC) {
            return None;
        }
        rest
    } else {
        return None;
    };
    let word = |at: usize| -> Option<usize> {
        let bytes = cache.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?) as usize)
    };
    let count = word(20)?;
    let strings_len = word(24)?;
    if count > CACHE_ENTRIES {
        return None;
    }
    let table_end = count.checked_mul(ENTRY)?.checked_add(HEADER)?;
    let strings_end = table_end.checked_add(strings_len)?;
    if strings_end > cache.len() {
        return None;
    }
    let mut spent = 0_usize;
    // A string at `at` in the declared string table, borrowed, bounded and counted.
    let mut string = |at: usize| -> Option<&str> {
        if at < table_end || at >= strings_end {
            return None;
        }
        let tail = &cache[at..strings_end.min(at + STRING_MAX + 1)];
        let end = tail.iter().position(|byte| *byte == 0)?;
        spent = spent.checked_add(end + 1)?;
        if spent > PARSE_WORK {
            return None;
        }
        std::str::from_utf8(&tail[..end]).ok()
    };
    let mut found = Vec::new();
    for index in 0..count {
        if index % 1024 == 0 && cancelled() {
            return None;
        }
        // flags i32, key u32, value u32, osversion u32, hwcap u64.
        let at = HEADER + index * ENTRY;
        let (key, value) = (word(at + 4)?, word(at + 8)?);
        let name = string(key)?;
        let path = string(value)?;
        found.push((name.to_owned(), PathBuf::from(path)));
    }
    Some(found)
}

/// What reading a bounded file gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Slurp {
    Data(Vec<u8>),
    /// Not a regular file, or larger than the bound: the same on every try.
    Refused,
    /// An open or read error (no descriptors left, an I/O error): it may pass.
    Failed,
}

/// Reads at most `max` bytes of a regular file, never blocking (a FIFO, or a link to one, is
/// opened without blocking and then refused).
pub(crate) fn read_bounded(path: &Path, max: u64) -> Slurp {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(path)
    else {
        return Slurp::Failed;
    };
    let Ok(meta) = file.metadata() else {
        return Slurp::Failed;
    };
    if !meta.is_file() || meta.len() > max {
        return Slurp::Refused;
    }
    let mut data = Vec::new();
    if file.take(max + 1).read_to_end(&mut data).is_err() {
        return Slurp::Failed;
    }
    if data.len() as u64 > max {
        Slurp::Refused
    } else {
        Slurp::Data(data)
    }
}

/// The parsed `ld.so.cache`, kept for the process while the file does not change (its
/// identity, size and modification time): every start would otherwise read and parse it again.
/// Only a parsed cache or a definite refusal (not recognised, not sound, too large) is kept: a
/// transient read error (no descriptors, an I/O error) and a cancelled parse are not.
fn cached_entries(cancelled: &dyn Fn() -> bool) -> Option<Vec<(String, PathBuf)>> {
    static MEMO: Memo = std::sync::Mutex::new(None);
    cached_at(
        Path::new("/etc/ld.so.cache"),
        &MEMO,
        &|path| read_bounded(path, CACHE_MAX),
        cancelled,
    )
}

type CacheStamp = (u64, u64, u64, i64, i64);
type Entries = Option<Vec<(String, PathBuf)>>;
/// The kept parse of a cache file and the stamp it is valid for.
pub(crate) type Memo = std::sync::Mutex<Option<(CacheStamp, Entries)>>;

/// [`cached_entries`] for `path`, with the memo and the reader given.
pub(crate) fn cached_at(
    path: &Path,
    memo: &Memo,
    read: &dyn Fn(&Path) -> Slurp,
    cancelled: &dyn Fn() -> bool,
) -> Entries {
    let meta = std::fs::metadata(path).ok()?;
    let stamp = (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
    );
    if let Some((known, entries)) = memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        && *known == stamp
    {
        return entries.clone();
    }
    let parsed = match read(path) {
        Slurp::Data(data) => parse_cache(&data, cancelled),
        Slurp::Refused => None,
        Slurp::Failed => return None,
    };
    if !cancelled() {
        *memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((stamp, parsed.clone()));
    }
    parsed
}

/// What the search of the host's library folders found for the names it was asked about.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// Resolvable by the loader's own rules.
    Yes,
    /// Candidates exist only where the loader would not look: unknown.
    Elsewhere,
    /// Nowhere in a complete search.
    No,
}

/// The host side of the check: where to look, with the means to test and stop it.
pub(crate) struct Host<'a> {
    /// The loader's default folders (top-level library folders and their multiarch children).
    pub standard: Vec<PathBuf>,
    /// The cache's `(name, path)` entries; `None` when it could not be read or recognised,
    /// which is not the same as a cache that parsed and is empty.
    pub cache: Option<Vec<(String, PathBuf)>>,
    /// The folders the sandbox binds, walked for names found nowhere else.
    pub roots: Vec<PathBuf>,
    pub budget: Budget,
    pub cancelled: &'a dyn Fn() -> bool,
    /// The enumeration of the default folders did not finish (a budget, a read error or a
    /// cancel): the host's folders were not all seen, so nothing can be said to be missing.
    pub incomplete: bool,
}

/// `(device, inode)` of a folder, following links.
fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.dev(), meta.ino()))
}

impl Host<'_> {
    /// The host's real folders: the bound library folders (one per distinct folder, as `/lib`
    /// may be an alias of `/usr/lib`), their multiarch children, and the cache.
    pub(crate) fn of_system(cancelled: &dyn Fn() -> bool) -> Host<'_> {
        let mut seen = HashSet::new();
        let roots: Vec<PathBuf> = LIBRARY_DIRS
            .iter()
            .map(PathBuf::from)
            .filter(|dir| identity(dir).is_some_and(|id| seen.insert(id)))
            .collect();
        let cache = cached_entries(cancelled).map(|entries| {
            entries
                .into_iter()
                .filter(|(_, path)| {
                    LIBRARY_DIRS
                        .iter()
                        .any(|dir| path.starts_with(dir) && path.is_absolute())
                })
                .collect()
        });
        Host {
            standard: Vec::new(),
            cache,
            roots,
            budget: Budget::DEFAULT,
            cancelled,
            incomplete: false,
        }
        .with_multiarch()
    }

    /// `standard` as the roots plus their `*-linux-*` children. The enumeration counts against
    /// the entry budget and watches for a cancel; if it does not finish, the host is marked
    /// [`incomplete`](Host::incomplete).
    pub(crate) fn with_multiarch(mut self) -> Self {
        let mut standard = self.roots.clone();
        let mut seen = 0_usize;
        'roots: for root in &self.roots {
            let Ok(entries) = std::fs::read_dir(root) else {
                self.incomplete = true;
                continue;
            };
            for entry in entries {
                seen += 1;
                if seen > self.budget.entries || (self.cancelled)() {
                    self.incomplete = true;
                    break 'roots;
                }
                let Ok(entry) = entry else {
                    self.incomplete = true;
                    continue 'roots;
                };
                if entry.file_name().to_string_lossy().contains("-linux-")
                    && entry.file_type().is_ok_and(|kind| kind.is_dir())
                {
                    standard.push(entry.path());
                }
            }
        }
        self.standard = standard;
        self
    }

    /// Where `wanted` (name and the kind of its dependant) can be had. The default folders are
    /// asked first; only the names they do not settle go on to the cache, which is indexed once
    /// by name with its repeated paths dropped; the names still open share one walk. Every phase
    /// counts against the budgets and watches for a cancel: running out of either ends the
    /// phase and, for the names not settled, the answer is "unknown".
    pub(crate) fn find(&self, wanted: &BTreeMap<String, Kind>) -> BTreeMap<String, Found> {
        let mut result = BTreeMap::new();
        let mut pending: BTreeMap<&String, Kind> = BTreeMap::new();
        let mut opens = 0_usize;
        // Whether every phase that ran covered what it should.
        let mut whole = !self.incomplete;
        for (name, kind) in wanted {
            let mut settled = false;
            for dir in &self.standard {
                if opens >= self.budget.opens || (self.cancelled)() {
                    whole = false;
                    break;
                }
                opens += 1;
                if compatible(&dir.join(name), *kind) {
                    settled = true;
                    break;
                }
            }
            if settled {
                result.insert(name.clone(), Found::Yes);
            } else {
                pending.insert(name, *kind);
            }
        }
        if !pending.is_empty() {
            let (index, indexed) = self.index(&pending);
            whole &= indexed;
            let names: Vec<&String> = pending.keys().copied().collect();
            for name in names {
                let kind = pending[name];
                for path in index.get(name.as_str()).into_iter().flatten() {
                    if opens >= self.budget.opens || (self.cancelled)() {
                        whole = false;
                        break;
                    }
                    opens += 1;
                    if compatible(path, kind) {
                        result.insert(name.clone(), Found::Yes);
                        pending.remove(name);
                        break;
                    }
                }
            }
        }
        if pending.is_empty() {
            return result;
        }
        let (elsewhere, complete) = self.walk(&pending, opens);
        for (name, _) in pending {
            let found = if elsewhere.contains(name) {
                Found::Elsewhere
            } else if complete && whole && self.cache.is_some() {
                // Only complete phases and a cache that was read can say "nowhere".
                Found::No
            } else {
                Found::Elsewhere
            };
            result.insert(name.clone(), found);
        }
        result
    }

    /// The cache's distinct paths for each name of `pending`, and whether the scan finished.
    #[allow(clippy::type_complexity, reason = "one local pair")]
    fn index<'h>(
        &'h self,
        pending: &BTreeMap<&String, Kind>,
    ) -> (BTreeMap<&'h str, BTreeSet<&'h Path>>, bool) {
        let mut index: BTreeMap<&str, BTreeSet<&Path>> = BTreeMap::new();
        for (scanned, (name, path)) in self.cache.iter().flatten().enumerate() {
            if scanned % 1024 == 0 && (scanned >= self.budget.entries || (self.cancelled)()) {
                return (index, false);
            }
            if pending.contains_key(name) {
                index
                    .entry(name.as_str())
                    .or_default()
                    .insert(path.as_path());
            }
        }
        (index, true)
    }

    /// The names of `open` with a compatible candidate anywhere under the roots, and whether the
    /// walk saw everything (a budget hit, an unreadable folder or a cancel makes it incomplete).
    fn walk(&self, open: &BTreeMap<&String, Kind>, opened: usize) -> (BTreeSet<String>, bool) {
        struct State<'a> {
            open: &'a BTreeMap<&'a String, Kind>,
            found: BTreeSet<String>,
            entries: usize,
            opens: usize,
            dirs: usize,
            seen: HashSet<(u64, u64)>,
            complete: bool,
        }
        fn visit(host: &Host<'_>, state: &mut State<'_>, dir: &Path, depth: usize) {
            if !state.complete {
                return;
            }
            // A folder already walked (an alias of another root) costs nothing.
            match identity(dir) {
                Some(id) if !state.seen.insert(id) => return,
                Some(_) => {}
                None => {
                    state.complete = false;
                    return;
                }
            }
            if (host.cancelled)() || state.dirs >= host.budget.dirs {
                state.complete = false;
                return;
            }
            state.dirs += 1;
            let Ok(entries) = std::fs::read_dir(dir) else {
                state.complete = false;
                return;
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    state.complete = false;
                    return;
                };
                state.entries += 1;
                if state.entries > host.budget.entries || (host.cancelled)() {
                    state.complete = false;
                    return;
                }
                let Ok(kind) = entry.file_type() else {
                    state.complete = false;
                    return;
                };
                if kind.is_dir() {
                    if depth < host.budget.depth {
                        visit(host, state, &entry.path(), depth + 1);
                    } else {
                        // Folders below the bound were not looked at.
                        state.complete = false;
                    }
                    if !state.complete {
                        return;
                    }
                } else if kind.is_file() || kind.is_symlink() {
                    // A FIFO, socket or device entry is never opened.
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if let Some(expected) = state.open.get(&name) {
                        if state.opens >= host.budget.opens {
                            state.complete = false;
                            return;
                        }
                        state.opens += 1;
                        if compatible(&entry.path(), *expected) {
                            state.found.insert(name);
                        }
                    }
                }
            }
        }
        let mut state = State {
            open,
            found: BTreeSet::new(),
            entries: 0,
            opens: opened,
            dirs: 0,
            seen: HashSet::new(),
            complete: true,
        };
        for root in &self.roots {
            visit(self, &mut state, root, 0);
        }
        (state.found, state.complete)
    }
}

/// The libraries the runtime files need and nothing provides, from `read` (a runtime file's
/// bytes by name) over `files` (every file name of the build). `None`: unknown.
pub(crate) fn missing_from(
    files: &[String],
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    host: &Host<'_>,
) -> Option<Vec<String>> {
    let own: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let mut wanted: BTreeMap<String, Kind> = BTreeMap::new();
    for file in files {
        let data = read(file)?;
        // The licence and other files that are not ELF files are not libraries.
        let Some(kind) = kind_of(data.get(..20).unwrap_or(&data)) else {
            if data.starts_with(b"\x7fELF") {
                return None;
            }
            continue;
        };
        for name in needed(&data)? {
            if !plain(&name) {
                return None;
            }
            if !own.contains(name.as_str()) {
                wanted.entry(name).or_insert(kind);
            }
        }
    }
    let mut missing = Vec::new();
    for (name, found) in host.find(&wanted) {
        match found {
            Found::Yes => {}
            Found::Elsewhere => return None,
            Found::No => missing.push(name),
        }
    }
    Some(missing)
}

/// What the owner can install for `library`.
fn remedy(library: &str) -> String {
    if library.starts_with("libgomp.so") {
        "install libgomp1 (Debian/Ubuntu) or your distribution's OpenMP runtime".to_owned()
    } else if library.starts_with("libvulkan.so") {
        "install libvulkan1 (Debian/Ubuntu) or your distribution's Vulkan loader".to_owned()
    } else {
        "install the package of your distribution that provides it".to_owned()
    }
}

/// The start-time error for `missing` libraries of `server` (`None`: none).
pub(crate) fn message(server: &str, missing: &[String]) -> Option<String> {
    let first = missing.first()?;
    let also = if missing.len() > 1 {
        format!(" (also missing: {})", missing[1..].join(", "))
    } else {
        String::new()
    };
    Some(format!(
        "{server} may need {first}: {}{also}",
        remedy(first)
    ))
}

/// The error for the libraries the runtime build's `files` (the server `server` among them)
/// need and the host lacks, from their descriptors; `None` when none is missing or nothing
/// could be told.
pub(crate) fn explain(
    files: &[Verified],
    server: &str,
    cancelled: &dyn Fn() -> bool,
) -> Option<String> {
    let names: Vec<String> = files.iter().map(|file| file.name.clone()).collect();
    let find = |name: &str| files.iter().find(|file| file.name == name);
    let host = Host::of_system(cancelled);
    let missing = missing_from(
        &names,
        &|name| find(name).and_then(|file| read_fd(file.fd.as_fd(), file.identity.size)),
        &host,
    )?;
    message(server, &missing)
}

/// The same over a folder on disk (the live check of a pinned build).
#[cfg(test)]
pub(crate) fn explain_dir(dir: &Path) -> Option<Vec<String>> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let host = Host::of_system(&|| false);
    missing_from(&names, &|name| std::fs::read(dir.join(name)).ok(), &host)
}

#[cfg(test)]
#[path = "libs_tests.rs"]
pub(crate) mod tests;
