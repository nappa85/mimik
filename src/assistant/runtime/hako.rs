//! llama-server's sandbox profile and assistant-facing diagnostics.
use super::sandbox::{self, GpuBinds, Item};
#[cfg(test)]
pub(crate) use crate::sandbox::apply_landlock;
use crate::sandbox::{Access, Bind, RunFile, Tmpfs, TmpfsFlags, masks, parents, text};
pub(crate) use crate::sandbox::{Grant, Identity, Landlock, Plan, Rule, Secret};
use std::path::{Path, PathBuf};

/// A runtime file to bind: its name under [`sandbox::RUNTIME`], the host path it was verified
/// at and its identity.
#[derive(Clone, Debug)]
pub(crate) struct File {
    pub name: String,
    pub source: PathBuf,
    pub identity: Identity,
}

/// What the launcher gives the plan.
#[derive(Debug)]
pub(crate) struct Inputs<'a> {
    pub root: PathBuf,
    /// The host's `/proc/self/mountinfo`: what is mounted under a bound directory.
    pub mountinfo: String,
    pub runtime: Vec<File>,
    pub model: (PathBuf, Identity),
    pub generation: (PathBuf, Identity),
    pub token: Secret,
    /// Further read-only files `(source, target, identity)` (the probe's own executable).
    pub extra: Vec<(PathBuf, String, Identity)>,
    pub gpu: Option<&'a GpuBinds>,
    pub env: Vec<(String, String)>,
    pub argv: Vec<String>,
}

/// The device nodes of the minimal `/dev`: no terminal, no `pts`, no `ptmx`.
pub(crate) const DEVICES: &[&str] = &["null", "zero", "full", "random", "urandom"];

fn ro(source: &Path, target: &str, identity: Option<Identity>) -> Bind {
    Bind {
        source: text(source),
        target: target.to_owned(),
        access: Access::ReadOnly,
        identity,
    }
}

/// The sandbox's own tmpfs mounts: `/run` (the token's file and the socket directory's bind),
/// `/dev`, `/tmp` and `/dev/shm`, each bounded.
fn fixed_tmpfs() -> Vec<Tmpfs> {
    [
        ("/run", 64 << 10, 0o755),
        ("/dev", 1 << 20, 0o755),
        ("/tmp", sandbox::TMP_BYTES, 0o1777),
        ("/dev/shm", sandbox::SHM_BYTES, 0o1777),
    ]
    .into_iter()
    .map(|(target, bytes, mode)| Tmpfs {
        target: target.to_owned(),
        bytes,
        mode,
        read_only: false,
        flags: TmpfsFlags::PRIVATE_DATA,
    })
    .collect()
}

/// The plan for `inputs`: llama-server's sandbox allowlist, consumed by shared construction.
pub(crate) fn plan(inputs: &Inputs<'_>) -> Plan {
    profile(inputs, sandbox::system_items())
}

fn profile(inputs: &Inputs<'_>, mut items: Vec<Item>) -> Plan {
    let mut binds = Vec::new();
    let mut symlinks = Vec::new();
    let mut dirs = Vec::new();
    if let Some(gpu) = inputs.gpu {
        items.extend(gpu.items());
    }
    for item in items {
        match item {
            Item::RoBind(path) => binds.push(ro(&path, &text(&path), None)),
            Item::Symlink { target, at } => symlinks.push((text(&target), text(&at))),
            Item::Dir(path) => dirs.push(text(&path)),
        }
    }
    if let Some(gpu) = inputs.gpu {
        binds.push(Bind {
            source: text(&gpu.dev),
            target: gpu.inside(),
            access: Access::Device,
            identity: None,
        });
    }
    for device in DEVICES {
        let path = format!("/dev/{device}");
        binds.push(Bind {
            source: path.clone(),
            target: path,
            access: Access::Device,
            identity: None,
        });
    }
    for file in &inputs.runtime {
        binds.push(ro(
            &file.source,
            &format!("{}/{}", sandbox::RUNTIME, file.name),
            Some(file.identity),
        ));
    }
    binds.push(ro(&inputs.model.0, sandbox::MODEL, Some(inputs.model.1)));
    for (source, target, identity) in &inputs.extra {
        binds.push(ro(source, target, Some(*identity)));
    }
    binds.push(Bind {
        source: text(&inputs.generation.0),
        target: sandbox::SOCKET_DIR.to_owned(),
        access: Access::ReadWrite,
        identity: Some(inputs.generation.1),
    });
    for (original, link) in [
        ("/proc/self/fd", "/dev/fd"),
        ("/proc/self/fd/0", "/dev/stdin"),
        ("/proc/self/fd/1", "/dev/stdout"),
        ("/proc/self/fd/2", "/dev/stderr"),
    ] {
        symlinks.push((original.to_owned(), link.to_owned()));
    }
    for (_, link) in &symlinks {
        dirs.extend(parents(link));
    }
    dirs.sort();
    dirs.dedup();
    let masks = masks(&binds, &inputs.mountinfo);
    let mut plan = Plan {
        root: text(&inputs.root),
        files: vec![RunFile {
            path: sandbox::TOKEN.to_owned(),
            content: inputs.token.clone(),
        }],
        proc: true,
        rules: Vec::new(),
        binds,
        tmpfs: fixed_tmpfs().into_iter().chain(masks).collect(),
        symlinks,
        dirs,
        workdir: sandbox::RUNTIME.to_owned(),
        argv: inputs.argv.clone(),
        env: inputs.env.clone(),
    };
    plan.rules = profile_rules(&plan);
    plan
}

/// Select llama-server's Landlock rules, narrowing the mounts to the uses it needs.
/// Execute is only for the program and system library directories (the ELF interpreter);
/// the loader maps the runtime libraries without that right. The shared layer adds `/proc`.
fn profile_rules(plan: &Plan) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut add = |path: &str, grant: Grant| {
        let rule = Rule {
            path: path.to_owned(),
            grant,
        };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    };
    add("/dev", Grant::List);
    // The token is read, never listed or written (the rest of `/run` has no rule).
    add(sandbox::TOKEN, Grant::ReadFile);
    // The runtime's directory holds only its bound files, but ggml lists it to find its
    // backends.
    add(sandbox::RUNTIME, Grant::List);
    // The scaffolding directories of the GPU path's `/sys` links (not the top-level ones).
    for dir in &plan.dirs {
        if Path::new(dir).components().count() > 2 {
            add(dir, Grant::List);
        }
    }
    for path in ["/tmp", "/dev/shm"] {
        add(path, Grant::ReadWrite);
    }
    for bind in &plan.binds {
        let program = plan.argv.first().is_some_and(|argv0| *argv0 == bind.target);
        let grant = match bind.access {
            Access::ReadOnlyDevice => Grant::ReadFile,
            Access::ReadOnly if bind.target == sandbox::MODEL => Grant::ReadFile,
            Access::ReadOnly
                if program || sandbox::LIBRARY_DIRS.contains(&bind.target.as_str()) =>
            {
                Grant::ReadExec
            }
            Access::ReadOnly => Grant::Read,
            Access::ReadWrite => Grant::ReadWrite,
            Access::Device => {
                // The device's directory is listed by a rule of its own: libdrm and Mesa
                // enumerate `/dev/dri` (the rule on `/dev` also reaches it, but the table
                // does not lean on that).
                if let Some(parent) = Path::new(&bind.target).parent().and_then(Path::to_str)
                    && parent != "/"
                {
                    add(parent, Grant::List);
                }
                Grant::Device
            }
        };
        add(&bind.target, grant);
    }
    rules
}

const APPARMOR_TEMPLATE: &str = include_str!("../../../apparmor/mimik");
const APPARMOR_PLACEHOLDER: &str = "@MIMIK_EXE@";

/// The `AppArmor` profile for this executable, with its absolute path as the attachment.
///
/// # Errors
///
/// The path is relative or has a character a profile attachment would interpret.
pub(crate) fn apparmor_profile(exe: &Path) -> Result<String, String> {
    let name = exe
        .to_str()
        .ok_or_else(|| "the executable's path is not text".to_owned())?;
    if !exe.is_absolute() {
        return Err("the executable's path is not absolute".into());
    }
    // `/proc/self/exe` of a binary replaced or removed while it runs ends in this: no profile
    // can attach to it.
    if name.ends_with(" (deleted)") {
        return Err("the executable has been replaced or removed since it started".into());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| c.is_control() || "\"\\{}[]*?^,@".contains(*c))
    {
        return Err(format!(
            "the executable's path has a character AppArmor would interpret ({bad:?})"
        ));
    }
    Ok(APPARMOR_TEMPLATE.replace(APPARMOR_PLACEHOLDER, name))
}

/// The profile `mimik assistant-apparmor-profile [--for <path>]` prints: for `--for`'s path (a
/// packager's or a test setup's, which needs no `sed`), otherwise for the running binary.
///
/// # Errors
///
/// An argument it does not know, or a path the profile cannot take.
pub(crate) fn apparmor_profile_for(args: &[String]) -> Result<String, String> {
    match args {
        [] => std::env::current_exe()
            .map_err(|_| "Mimik's own path is unknown".to_owned())
            .and_then(|exe| apparmor_profile(&exe)),
        [flag, path] if flag == "--for" => apparmor_profile(Path::new(path)),
        _ => Err("usage: assistant-apparmor-profile [--for <absolute path of the binary>]".into()),
    }
}

/// `text` as one shell word.
pub(crate) fn shell_word(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The page's remedy for a failed Hakoniwa start, by the one setting that may explain it
/// (worded as a possibility: the probe's failure itself decides nothing).
pub(crate) fn remedy(cause: sandbox::Cause, exe: &Path) -> String {
    match cause {
        sandbox::Cause::AppArmor => apparmor_message(exe),
        sandbox::Cause::UsernsClone => "kernel.unprivileged_userns_clone = 0 may be why: it forbids the private sandbox of the model server. An administrator can allow it with `sudo sysctl kernel.unprivileged_userns_clone=1` (and keep it in a file under /etc/sysctl.d), then press Restart.".to_owned(),
    }
}

/// The assistant page's text for Ubuntu's `AppArmor` restriction of unprivileged user
/// namespaces: the profile for this very binary, generated by it, and the steps to install
/// it (the page's text, checked on Ubuntu 24.04). The profile is written to a file first,
/// so a failure of the generating command installs nothing.
pub(crate) fn apparmor_message(exe: &Path) -> String {
    let command = shell_word(&exe.to_string_lossy());
    format!(
        "Ubuntu's AppArmor restriction of unprivileged user namespaces may be why Mimik cannot create the private sandbox of the model server. An administrator can allow it once for this program: run `{command} assistant-apparmor-profile > mimik.apparmor && sudo install -m 0644 mimik.apparmor /etc/apparmor.d/mimik && sudo apparmor_parser -r /etc/apparmor.d/mimik`, then press Restart. The profile only gives this one program that permission; it does not confine or unconfine anything else."
    )
}

/// What the Hakoniwa sandbox needs of the host before its probe: unprivileged user
/// namespaces (the one setting that rules them out for certain; the probe decides otherwise).
/// There is no helper program to find.
///
/// # Errors
///
/// A host setting that certainly disables unprivileged user namespaces.
pub(crate) fn preflight() -> Result<(), sandbox::Unusable> {
    sandbox::namespaces(Path::new("/proc"))
}

/// Reads the runner's start report: the reaper and optional server identity/end state.
pub(crate) fn started(line: &str) -> Option<sandbox::Started> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(sandbox::Started {
        child_pid: i32::try_from(value.get("child-pid")?.as_i64()?).ok()?,
        pid_namespace: super::endpoint::NAMESPACE_UNREPORTED,
        inner_pid: match value.get("inner-pid") {
            Some(inner) => Some(i32::try_from(inner.as_i64()?).ok()?),
            None => None,
        },
        inner_start: match value.get("inner-start") {
            Some(start) => Some(start.as_u64()?),
            None => None,
        },
        ended: match value.get("ended") {
            Some(ended) => ended.as_bool()?,
            None => false,
        },
    })
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod profile_tests;

#[cfg(test)]
#[path = "hako_tests.rs"]
mod hako_tests;
