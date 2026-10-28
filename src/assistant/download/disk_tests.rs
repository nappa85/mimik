use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::sync::Mutex;

/// Fails (returns an error) the first time `point` is hit.
pub(crate) struct FailAt(pub Fault, Mutex<bool>);

impl FailAt {
    pub(crate) fn new(point: Fault) -> Arc<Self> {
        Arc::new(Self(point, Mutex::new(false)))
    }
}

impl Faults for FailAt {
    fn hit(&self, point: Fault) -> std::io::Result<()> {
        let mut fired = self.1.lock().unwrap();
        if point == self.0 && !*fired {
            *fired = true;
            return Err(std::io::Error::other("injected"));
        }
        Ok(())
    }
}

fn folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn disk(dir: &tempfile::TempDir) -> Disk {
    Disk::open(dir.path(), Arc::new(NoFaults)).unwrap()
}

fn partial_with(disk: &Disk, name: &str, bytes: &[u8]) {
    let file = disk.create_partial(name).unwrap();
    disk.write_at(&file, 0, bytes).unwrap();
}

#[test]
fn the_folders_are_private_and_the_partial_is_created_exclusively_with_mode_0600() {
    let dir = folder();
    let disk = disk(&dir);
    let partial = dir.path().join(PARTIAL_DIR);
    assert_eq!(std::fs::metadata(&partial).unwrap().mode() & 0o7777, 0o700);
    disk.create_partial("a").unwrap();
    assert_eq!(
        std::fs::metadata(partial.join("a")).unwrap().mode() & 0o7777,
        0o600
    );
    // An existing name is never reopened for creation.
    assert!(disk.create_partial("a").is_err());
    // A models folder that is not private is refused.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Disk::open(dir.path(), Arc::new(NoFaults)).is_err());
}

#[test]
fn a_partial_that_is_a_link_a_fifo_or_loosely_owned_is_refused_and_left_untouched() {
    let dir = folder();
    let disk = disk(&dir);
    let partial = dir.path().join(PARTIAL_DIR);

    // A symbolic link, even to a harmless file.
    let target = dir.path().join("target");
    std::fs::write(&target, b"keep").unwrap();
    symlink(&target, partial.join("link")).unwrap();
    assert!(disk.open_partial("link").is_err());
    assert!(disk.partial_state("link").is_err());

    // A hard link: never truncate a linked file.
    std::fs::write(partial.join("hard"), b"keep").unwrap();
    std::fs::hard_link(partial.join("hard"), dir.path().join("elsewhere")).unwrap();
    assert!(disk.open_partial("hard").is_err());
    assert_eq!(
        std::fs::read(dir.path().join("elsewhere")).unwrap(),
        b"keep"
    );
    assert_eq!(disk.partial_state("hard").unwrap().unwrap().links, 2);

    // A FIFO is refused without blocking (the test would hang on a blocking open).
    let fifo = partial.join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert!(disk.open_partial("fifo").is_err());

    // Group- or other-writable.
    std::fs::write(partial.join("loose"), b"x").unwrap();
    std::fs::set_permissions(
        partial.join("loose"),
        std::fs::Permissions::from_mode(0o620),
    )
    .unwrap();
    assert!(disk.open_partial("loose").is_err());
    std::fs::set_permissions(
        partial.join("loose"),
        std::fs::Permissions::from_mode(0o602),
    )
    .unwrap();
    assert!(disk.open_partial("loose").is_err());

    // A good one opens and reports its length.
    partial_with(&disk, "good", b"12345");
    let (_, len) = disk.open_partial("good").unwrap();
    assert_eq!(len, 5);
    assert!(disk.open_partial("absent").is_err());
    assert_eq!(disk.partial_state("absent").unwrap(), None);
}

#[test]
fn publishing_moves_the_partial_to_the_final_name_and_never_replaces() {
    for forced_link in [false, true] {
        let dir = folder();
        let disk = disk(&dir);
        disk.no_rename2
            .store(forced_link, std::sync::atomic::Ordering::SeqCst);
        partial_with(&disk, "p", b"model");
        disk.publish("p", "final.gguf").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("final.gguf")).unwrap(),
            b"model"
        );
        assert!(!dir.path().join(PARTIAL_DIR).join("p").exists());
        assert_eq!(
            std::fs::metadata(dir.path().join("final.gguf"))
                .unwrap()
                .nlink(),
            1
        );
        // A second publication onto the same name is refused and changes nothing.
        partial_with(&disk, "q", b"other");
        assert!(matches!(
            disk.publish("q", "final.gguf"),
            Err(DiskError::Exists)
        ));
        assert_eq!(
            std::fs::read(dir.path().join("final.gguf")).unwrap(),
            b"model"
        );
        assert_eq!(
            std::fs::read(dir.path().join(PARTIAL_DIR).join("q")).unwrap(),
            b"other"
        );
        // A symbolic link in the way is not replaced either.
        symlink("/nonexistent", dir.path().join("dangling.gguf")).unwrap();
        assert!(matches!(
            disk.publish("q", "dangling.gguf"),
            Err(DiskError::Exists)
        ));
    }
}

#[test]
fn the_link_fallback_leaves_a_recognisable_second_link_when_it_stops_midway() {
    let dir = folder();
    let faults = FailAt::new(Fault::AfterLink);
    let disk = Disk::open(dir.path(), faults).unwrap();
    disk.no_rename2
        .store(true, std::sync::atomic::Ordering::SeqCst);
    partial_with(&disk, "p", b"model");
    assert!(disk.publish("p", "final.gguf").is_err());
    // Both names exist and share the inode: this is what recovery recognises.
    let partial = disk.partial_state("p").unwrap().unwrap();
    let published = disk.final_state("final.gguf").unwrap().unwrap();
    assert_eq!((partial.inode, partial.links), (published.inode, 2));
    assert_eq!(published.inode, partial.inode);
    // The ordinary admission refuses the doubly linked partial; recovery removes the name.
    assert!(disk.open_partial("p").is_err());
    disk.remove_partial("p").unwrap();
    assert_eq!(disk.final_state("final.gguf").unwrap().unwrap().links, 1);
}

#[test]
fn a_failed_sync_or_read_is_reported_and_a_stop_during_hashing_is_a_cancel() {
    for point in [Fault::FileFsyncError] {
        let dir = folder();
        let disk = Disk::open(dir.path(), FailAt::new(point)).unwrap();
        let file = disk.create_partial("p").unwrap();
        assert!(disk.sync_file(&file).is_err());
        assert!(disk.sync_file(&file).is_ok(), "the fault fires once");
    }
    let dir = folder();
    let disk = Disk::open(dir.path(), FailAt::new(Fault::ReadError)).unwrap();
    partial_with(&disk, "p", b"abc");
    let (file, _) = disk.open_partial("p").unwrap();
    assert!(disk.hash(&file, "p", &|| false).is_err());
    assert_eq!(
        disk.hash(&file, "p", &|| false).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(matches!(
        disk.hash(&file, "p", &|| true),
        Err(DiskError::Cancelled)
    ));
    // Directory sync failures stop the publication before the row could say complete.
    for point in [Fault::DirFsyncError, Fault::BetweenDirSyncs] {
        let dir = folder();
        let disk = Disk::open(dir.path(), FailAt::new(point)).unwrap();
        partial_with(&disk, "p", b"model");
        assert!(disk.publish("p", "final.gguf").is_err(), "{point:?}");
        // Undone: nothing is published that may not survive a crash.
        assert!(!dir.path().join("final.gguf").exists(), "{point:?}");
        assert!(dir.path().join(PARTIAL_DIR).join("p").exists(), "{point:?}");
    }
}

#[test]
fn free_space_is_measured_and_can_be_replaced_by_a_test() {
    let dir = folder();
    let disk = disk(&dir);
    assert!(disk.free_space() > 0);
    let disk = disk.with_space(Arc::new(|_| 123));
    assert_eq!(disk.free_space(), 123);
}

#[test]
fn partials_are_listed_and_removed_by_name() {
    let dir = folder();
    let disk = disk(&dir);
    partial_with(&disk, "a", b"1");
    partial_with(&disk, "b", b"2");
    let mut names = disk.partials().unwrap();
    names.sort();
    assert_eq!(names, ["a", "b"]);
    disk.remove_partial("a").unwrap();
    disk.remove_partial("a").unwrap();
    assert_eq!(disk.partials().unwrap(), ["b"]);
}
