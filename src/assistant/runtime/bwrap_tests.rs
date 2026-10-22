use super::*;

/// A fake bwrap that prints `options` as its help text, with `mode`.
fn fake(dir: &Path, name: &str, options: &[&str], mode: u32) -> PathBuf {
    let path = dir.join(name);
    let mut script = String::from("#!/bin/sh\ncat <<'EOF'\nusage: bwrap [OPTIONS...] [--] COMMAND [ARGS...]\n");
    for option in options {
        script.push_str(&format!("    {option} ARG     Something\n"));
    }
    script.push_str("EOF\n");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn all_options() -> Vec<&'static str> {
    let mut options = OPTIONS.to_vec();
    options.push(SIZE);
    options
}

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
fn the_first_fixed_path_that_exists_is_used_with_every_option_probed() {
    let dir = tempfile::tempdir().unwrap();
    let proc = proc_root(dir.path(), &[]);
    let good = fake(dir.path(), "good", &all_options(), 0o755);
    let missing = dir.path().join("missing");
    let found = preflight_at(&[&missing, &good], &proc).unwrap();
    assert_eq!(found, Bwrap { path: good.clone(), size: true });
    let no_size = fake(dir.path(), "no-size", OPTIONS, 0o755);
    assert!(!preflight_at(&[&no_size], &proc).unwrap().size);
    assert_eq!(preflight_at(&[&missing], &proc), Err(Unusable::Missing));
    for (index, option) in OPTIONS.iter().enumerate() {
        let mut options = all_options();
        options.remove(index);
        let lacking = fake(dir.path(), &format!("lacking-{index}"), &options, 0o755);
        assert_eq!(
            preflight_at(&[&lacking], &proc),
            Err(Unusable::Option(option)),
            "{option}"
        );
    }
    // A prefix is not the option (`--bind-fd` is not `--ro-bind-fd`).
    let prefixes = fake(
        dir.path(),
        "prefixes",
        &all_options()
            .into_iter()
            .filter(|option| *option != "--ro-bind")
            .chain(["--ro-bind-tryhard"])
            .collect::<Vec<_>>(),
        0o755,
    );
    assert_eq!(
        preflight_at(&[&prefixes], &proc),
        Err(Unusable::Option("--ro-bind"))
    );
}

#[test]
fn a_set_id_bwrap_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let proc = proc_root(dir.path(), &[]);
    for mode in [0o4755, 0o2755] {
        let setid = fake(dir.path(), &format!("setid-{mode:o}"), &all_options(), mode);
        // The bits stick for the owner on most file systems; skip where they do not.
        if std::fs::metadata(&setid).unwrap().permissions().mode() & 0o6000 == 0 {
            continue;
        }
        assert_eq!(
            preflight_at(&[&setid], &proc),
            Err(Unusable::SetId(setid.display().to_string()))
        );
    }
}

#[test]
fn disabled_user_namespaces_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let good = fake(dir.path(), "good", &all_options(), 0o755);
    for (settings, expected) in [
        (
            vec![("sys/user/max_user_namespaces", "0\n")],
            Unusable::UserNamespaces("user.max_user_namespaces = 0"),
        ),
        (
            vec![
                ("sys/user/max_user_namespaces", "128000\n"),
                ("sys/kernel/unprivileged_userns_clone", "0\n"),
            ],
            Unusable::UserNamespaces("kernel.unprivileged_userns_clone = 0"),
        ),
        (
            vec![("sys/kernel/apparmor_restrict_unprivileged_userns", "1\n")],
            Unusable::AppArmor,
        ),
    ] {
        let case = tempfile::tempdir_in(dir.path()).unwrap();
        let proc = proc_root(case.path(), &settings);
        assert_eq!(preflight_at(&[&good], &proc), Err(expected));
    }
    let case = tempfile::tempdir_in(dir.path()).unwrap();
    let proc = proc_root(
        case.path(),
        &[
            ("sys/user/max_user_namespaces", "128000\n"),
            ("sys/kernel/apparmor_restrict_unprivileged_userns", "0\n"),
        ],
    );
    assert!(preflight_at(&[&good], &proc).is_ok());
}

fn profile(size: bool) -> Profile {
    Profile {
        size,
        status_fd: 4,
        seccomp_fd: 5,
        token_fd: 6,
        socket_dir_fd: 7,
        model_fd: 8,
        runtime: vec![(9, "llama-server".into()), (10, "libllama.so.0".into())],
        extra: Vec::new(),
        env: Vec::new(),
        command: vec!["/opt/llama/llama-server".into(), "--offline".into()],
    }
}

fn pairs(args: &[OsString]) -> Vec<(String, String)> {
    let text: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    text.windows(2).map(|w| (w[0].clone(), w[1].clone())).collect()
}

#[test]
fn the_profile_unshares_everything_and_binds_only_the_allowlist() {
    let args = profile(true).arguments();
    let text: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    for required in [
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--remount-ro",
    ] {
        assert!(text.contains(&required.to_owned()), "{required}");
    }
    assert!(!text.iter().any(|a| a == "--share-net"));
    let pairs = pairs(&args);
    assert!(pairs.contains(&("--cap-drop".into(), "ALL".into())));
    // No bind of the host root, nothing bound read-write but the socket directory.
    for (option, value) in &pairs {
        if option == "--ro-bind" || option == "--bind" || option == "--dev-bind" {
            assert_ne!(value, "/", "{option}");
        }
        assert_ne!(option, "--bind");
        assert_ne!(option, "--dev-bind");
    }
    assert!(pairs.contains(&("--bind-fd".into(), "7".into())));
    assert!(text.windows(3).any(|w| w == ["--bind-fd", "7", SOCKET_DIR]));
    assert!(text.windows(3).any(|w| w == ["--ro-bind-fd", "8", MODEL]));
    assert!(text.windows(3).any(|w| w == ["--ro-bind-fd", "9", "/opt/llama/llama-server"]));
    assert!(text.windows(3).any(|w| w == ["--ro-bind-data", "6", TOKEN]));
    assert!(pairs.contains(&("--seccomp".into(), "5".into())));
    assert!(pairs.contains(&("--json-status-fd".into(), "4".into())));
    assert!(text.windows(4).any(|w| w == ["--size", "67108864", "--tmpfs", "/tmp"]));
    assert!(text.windows(4).any(|w| w == ["--size", "268435456", "--tmpfs", "/dev/shm"]));
    assert_eq!(&text[text.len() - 2..], ["/opt/llama/llama-server", "--offline"]);
    // The minimal environment.
    let set: Vec<&[String]> = text.windows(3).filter(|w| w[0] == "--setenv").collect();
    let names: Vec<&str> = set.iter().map(|w| w[1].as_str()).collect();
    assert_eq!(names, ["PATH", "LANG", "HOME"]);
    // Without --size the temporary mounts have none.
    let plain = profile(false).arguments();
    assert!(!plain.iter().any(|a| a == "--size"));
}

#[test]
fn status_lines_from_the_installed_bwrap_shape() {
    let line = r#"{ "child-pid": 3183140, "cgroup-namespace": 4026532683, "ipc-namespace": 4026532681, "mnt-namespace": 4026532573, "net-namespace": 4026532685, "pid-namespace": 4026532682, "uts-namespace": 4026532639 }"#;
    assert_eq!(
        started(line),
        Some(Started {
            child_pid: 3_183_140,
            pid_namespace: 4_026_532_682
        })
    );
    assert_eq!(exit_code(line), None);
    assert_eq!(exit_code(r#"{ "exit-code": 143 }"#), Some(143));
    assert_eq!(started(r#"{ "exit-code": 0 }"#), None);
    assert_eq!(started("not json"), None);
}
