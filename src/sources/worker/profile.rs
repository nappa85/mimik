//! PDF-only profile: no model, socket directory, GPU, sysfs or proc.
use crate::sandbox::{
    Access, Bind, Grant, Identity, Profile, Rule, RunFile, Secret, Tmpfs, TmpfsFlags, masks,
    parents,
    system::{self, Item},
    text,
};
use std::path::Path;

pub(super) const EXECUTABLE: &str = "/opt/mimik/mimik";
const LOADER: &str = "/opt/mimik/loader";
pub(super) const INPUT: &str = "/in/document.pdf";
pub(super) const SPEC: &str = "/run/spec.json";
pub(super) const SPEC_LIMIT: usize = 64 << 10;

pub(super) fn encode(spec: &super::Spec) -> Result<String, super::Error> {
    let mut writer = super::records::LimitedBytes {
        bytes: Vec::new(),
        limit: SPEC_LIMIT,
    };
    serde_json::to_writer(&mut writer, spec).map_err(|_| super::Error::TooLarge)?;
    String::from_utf8(writer.bytes).map_err(|_| super::Error::Unreadable)
}

pub(super) struct Inputs<'a> {
    pub root: &'a Path,
    pub executable: (&'a Path, Identity),
    pub input: (&'a Path, Identity),
    pub loader: &'a Path,
    pub spec: String,
    pub mountinfo: &'a str,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub(super) fn build(input: Inputs<'_>) -> Profile {
    build_with(input, system::library_items())
}

fn build_with(input: Inputs<'_>, items: Vec<Item>) -> Profile {
    let mut binds = Vec::new();
    let mut symlinks = Vec::new();
    let mut dirs = Vec::new();
    let ro = |source: &Path, target: String, identity| Bind {
        source: text(source),
        target,
        identity,
        access: Access::ReadOnly,
    };
    for item in items {
        match item {
            Item::RoBind(path) => binds.push(ro(&path, text(&path), None)),
            Item::Symlink { target, at } => symlinks.push((text(&target), text(&at))),
            Item::Dir(path) => dirs.push(text(&path)),
        }
    }
    binds.extend([
        ro(
            input.executable.0,
            EXECUTABLE.into(),
            Some(input.executable.1),
        ),
        ro(input.input.0, INPUT.into(), Some(input.input.1)),
        // A separate target avoids Hakoniwa's O_APPEND touch of the root-owned
        // interpreter under an already-bound system library directory.
        ro(input.loader, LOADER.into(), None),
    ]);
    // The shared closure needs a real null device for stdin. Keep the bind read-only,
    // without nodev (which would prevent opening it), and grant only read access.
    binds.push(Bind {
        source: "/dev/null".into(),
        target: "/dev/null".into(),
        identity: None,
        access: Access::ReadOnlyDevice,
    });
    for (_, link) in &symlinks {
        dirs.extend(parents(link));
    }
    dirs.sort();
    dirs.dedup();
    let mut rules = vec![
        Rule {
            path: "/in".into(),
            grant: Grant::Read,
        },
        Rule {
            path: SPEC.into(),
            grant: Grant::ReadFile,
        },
        Rule {
            path: "/tmp".into(),
            grant: Grant::Scratch,
        },
    ];
    for bind in &binds {
        if bind.target == INPUT {
            continue;
        }
        rules.push(Rule {
            path: bind.target.clone(),
            grant: if bind.target == EXECUTABLE || bind.target == LOADER {
                Grant::ReadExec
            } else if bind.target == "/dev/null" {
                Grant::ReadFile
            } else {
                Grant::Read
            },
        });
    }
    let tmpfs = [
        ("/tmp", 16 << 20, 0o1777),
        // Allow a full 64 KiB spec plus filesystem bookkeeping.
        ("/run", 128 << 10, 0o700),
    ]
    .into_iter()
    .map(|(target, bytes, mode)| Tmpfs {
        target: target.into(),
        bytes,
        mode,
        read_only: false,
        flags: TmpfsFlags::PRIVATE_DATA,
    })
    .chain(masks(&binds, input.mountinfo))
    .collect();
    Profile {
        root: text(input.root),
        files: vec![RunFile {
            path: SPEC.into(),
            content: Secret::new(input.spec),
        }],
        proc: false,
        rules,
        binds,
        tmpfs,
        symlinks,
        dirs,
        workdir: "/tmp".into(),
        // Execute the explicitly bound loader, which then loads Mimik and its
        // dependencies. PT_INTERP's original path stays readable via the library bind.
        argv: std::iter::once(LOADER.into()).chain(input.argv).collect(),
        env: input.env,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pdf_profile_is_pinned() {
        let profile = build_with(
            Inputs {
                root: Path::new("/host/root"),
                executable: (Path::new("/host/mimik"), (1, 2)),
                input: (Path::new("/host/document"), (3, 4)),
                loader: Path::new("/usr/lib/ld.so"),
                spec: "{}".into(),
                mountinfo: "",
                argv: vec![
                    EXECUTABLE.into(),
                    "pdf-worker".into(),
                    "--spec".into(),
                    SPEC.into(),
                ],
                env: vec![],
            },
            vec![
                Item::RoBind("/usr/lib".into()),
                Item::RoBind("/etc/ld.so.cache".into()),
            ],
        );
        profile.validate().unwrap();
        // Hakoniwa touches a file target O_APPEND before binding it. Never place
        // such a target beneath a system directory bind (root-owned file).
        assert!(
            profile.binds.iter().all(|bind| bind.target == "/usr/lib"
                || !Path::new(&bind.target).starts_with("/usr/lib"))
        );
        assert!(!profile.proc);
        assert_eq!(
            profile
                .binds
                .iter()
                .map(|b| (b.target.as_str(), b.access, b.identity))
                .collect::<Vec<_>>(),
            vec![
                ("/usr/lib", Access::ReadOnly, None),
                ("/etc/ld.so.cache", Access::ReadOnly, None),
                (EXECUTABLE, Access::ReadOnly, Some((1, 2))),
                (INPUT, Access::ReadOnly, Some((3, 4))),
                (LOADER, Access::ReadOnly, None),
                ("/dev/null", Access::ReadOnlyDevice, None),
            ]
        );
        assert_eq!(
            profile
                .tmpfs
                .iter()
                .map(|t| (t.target.as_str(), t.bytes, t.mode, t.read_only, t.flags))
                .collect::<Vec<_>>(),
            vec![
                ("/tmp", 16 << 20, 0o1777, false, TmpfsFlags::PRIVATE_DATA),
                ("/run", 128 << 10, 0o700, false, TmpfsFlags::PRIVATE_DATA),
            ]
        );
        assert_eq!(
            profile.files,
            vec![RunFile {
                path: SPEC.into(),
                content: Secret::new("{}".into())
            }]
        );
        assert_eq!(
            profile
                .rules
                .iter()
                .map(|r| (r.path.as_str(), r.grant))
                .collect::<Vec<_>>(),
            vec![
                ("/in", Grant::Read),
                (SPEC, Grant::ReadFile),
                ("/tmp", Grant::Scratch),
                ("/usr/lib", Grant::Read),
                ("/etc/ld.so.cache", Grant::Read),
                (EXECUTABLE, Grant::ReadExec),
                (LOADER, Grant::ReadExec),
                ("/dev/null", Grant::ReadFile),
            ]
        );
        assert_eq!(profile.symlinks, [] as [(String, String); 0]);
        assert_eq!(profile.dirs, [] as [String; 0]);
        assert_eq!(profile.workdir, "/tmp");
    }

    #[test]
    fn spec_size_bound_includes_all_encoded_bytes() {
        let mut spec = super::super::Spec {
            operation: super::super::records::Operation::Inspect,
            pages: vec![],
            settings: super::super::RenderSettings::default(),
            input: "".into(),
        };
        let overhead = encode(&spec).unwrap().len();
        spec.input = "a".repeat(SPEC_LIMIT - overhead).into();
        assert_eq!(encode(&spec).unwrap().len(), SPEC_LIMIT);
        spec.input.push("b");
        assert_eq!(encode(&spec), Err(super::super::Error::TooLarge));
    }
}
