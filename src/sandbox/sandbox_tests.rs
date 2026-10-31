use super::*;
use std::path::PathBuf;

fn proc_root(dir: &Path, settings: &[(&str, &str)]) -> PathBuf {
    let root = dir.join("proc");
    for (name, value) in settings {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn only_disabled_user_namespaces_veto_the_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let proc = proc_root(dir.path(), &[("sys/user/max_user_namespaces", "0\n")]);
    assert_eq!(
        namespaces(&proc),
        Err(Unusable::UserNamespaces("user.max_user_namespaces = 0"))
    );
    // AppArmor's restriction and Debian's switch do not veto: the real start decides, and they
    // only explain its failure.
    for (setting, value) in [
        ("sys/kernel/apparmor_restrict_unprivileged_userns", "1\n"),
        ("sys/kernel/unprivileged_userns_clone", "0\n"),
    ] {
        let case = tempfile::tempdir_in(dir.path()).unwrap();
        let proc = proc_root(case.path(), &[(setting, value)]);
        assert_eq!(namespaces(&proc), Ok(()), "{setting}");
    }
}

#[test]
fn the_settings_that_may_explain_a_failed_start_are_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    for (setting, value, cause) in [
        (
            "sys/kernel/unprivileged_userns_clone",
            "0\n",
            Cause::UsernsClone,
        ),
        (
            "sys/kernel/apparmor_restrict_unprivileged_userns",
            "1\n",
            Cause::AppArmor,
        ),
    ] {
        let case = tempfile::tempdir_in(dir.path()).unwrap();
        let proc = proc_root(case.path(), &[(setting, value)]);
        assert_eq!(namespace_cause(&proc), Some(cause), "{setting}");
    }
    // Neither set to its restrictive value: no cause is named.
    let case = tempfile::tempdir_in(dir.path()).unwrap();
    let proc = proc_root(
        case.path(),
        &[
            ("sys/kernel/unprivileged_userns_clone", "1\n"),
            ("sys/kernel/apparmor_restrict_unprivileged_userns", "0\n"),
        ],
    );
    assert_eq!(namespace_cause(&proc), None);
}
