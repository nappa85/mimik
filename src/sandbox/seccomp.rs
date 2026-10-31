//! The shared seccomp program (assisted sheet plan §1.4a, "Seccomp"), compiled in pure
//! Rust with `seccompiler` (no `libseccomp`, no runtime shared library).
//!
//! Default action allow with listed refusals returning `EPERM` (`ENOSYS` for `clone3`). The
//! first instructions are hand-written BPF, because `seccompiler` matches syscall numbers one
//! by one and cannot express a range: the architecture check (anything but
//! `AUDIT_ARCH_X86_64` is killed), then every x32 syscall number (`nr & 0x40000000`) refused,
//! so x32 calls cannot bypass the rules, then `clone3` → `ENOSYS` (its flags cannot be
//! inspected; glibc falls back to `clone` for threads). `seccompiler`'s own program follows,
//! re-checking the architecture.
//!
//! What it refuses: new processes (`fork`, `vfork`, `clone` without `CLONE_THREAD`), sockets
//! other than TCP over IPv4 or IPv6 and stream Unix sockets, `socketpair`, TCP Fast Open
//! (`MSG_FASTOPEN` sends, `TCP_FASTOPEN_CONNECT`), reaching other processes' descriptors or
//! memory (`pidfd_open`, `pidfd_getfd`, `process_madvise`, `kcmp`, `ptrace`,
//! `process_vm_readv`/`writev`), `io_uring`, `bpf`, `perf_event_open`, the key calls,
//! mounts, namespaces, modules and `kexec`, and clearing the parent-death signal or setting
//! `no_new_privs` again. On the TCP levels it also restricts exec (below). Seccomp sees no
//! `connect()` address, so it is never described as network confinement.
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition,
    SeccompFilter, SeccompRule, TargetArch, sock_filter,
};
use std::collections::BTreeMap;

/// Where the program is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    /// Level 1: the sandbox's closure installs it just before it executes llama-server inside the
    /// sandbox, so exec stays allowed (the PID namespace contains whatever runs there).
    Sandbox,
    /// Level 3: the runner installs it, then executes the verified llama-server
    /// descriptor `exec_fd` with `execveat(…, AT_EMPTY_PATH)`. `execve` is refused and
    /// `execveat` is allowed only on that descriptor number with exactly `AT_EMPTY_PATH`. The
    /// descriptor is close-on-exec and the runner sets `RLIMIT_NOFILE` to its number, so after
    /// the exec no descriptor can ever have that number again (`open`, `dup2` and
    /// `F_DUPFD` all refuse a number at or above the limit, and an unprivileged process cannot
    /// raise the hard limit): no later exec is possible, from any thread.
    Tcp { exec_fd: u32 },
}

const EPERM: u32 = 1;
const ENOSYS: u32 = 38;

/// `seccomp_data` offsets.
const ARCH_OFFSET: u32 = 4;
const NR_OFFSET: u32 = 0;
const X32_SYSCALL_BIT: u32 = 0x4000_0000;
const AUDIT_ARCH_X86_64: u32 = 0x3e | 0x8000_0000 | 0x4000_0000;

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JEQ_K: u16 = 0x15;
const BPF_JGE_K: u16 = 0x35;
const BPF_RET_K: u16 = 0x06;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;

const AF_UNIX: u64 = 1;
const AF_INET: u64 = 2;
const AF_INET6: u64 = 10;
const SOCK_STREAM: u64 = 1;
const SOCK_TYPE_MASK: u64 = 0xf;
const IPPROTO_TCP: u64 = 6;
const CLONE_THREAD: u64 = 0x0001_0000;
const MSG_FASTOPEN: u64 = 0x2000_0000;
const TCP_FASTOPEN_CONNECT: u64 = 30;
const PR_SET_PDEATHSIG: u64 = 1;
const PR_SET_NO_NEW_PRIVS: u64 = 38;
const AT_EMPTY_PATH: u64 = 0x1000;

fn stmt(code: u16, k: u32) -> sock_filter {
    sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> sock_filter {
    sock_filter { code, jt, jf, k }
}

/// The hand-written first instructions.
fn prefix() -> Vec<sock_filter> {
    vec![
        stmt(BPF_LD_W_ABS, ARCH_OFFSET),
        jump(BPF_JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
        stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        stmt(BPF_LD_W_ABS, NR_OFFSET),
        jump(BPF_JGE_K, X32_SYSCALL_BIT, 0, 1),
        stmt(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM),
        jump(
            BPF_JEQ_K,
            u32::try_from(libc::SYS_clone3).unwrap_or(435),
            0,
            1,
        ),
        stmt(BPF_RET_K, SECCOMP_RET_ERRNO | ENOSYS),
    ]
}

fn cond(arg: u8, op: Op, value: u64) -> SeccompCondition {
    SeccompCondition::new(arg, Len::Dword, op, value).expect("argument index below 6")
}

fn rule(conditions: Vec<SeccompCondition>) -> SeccompRule {
    SeccompRule::new(conditions).expect("a rule has conditions")
}

/// Refused whatever their arguments.
const ALWAYS: &[i64] = &[
    libc::SYS_fork,
    libc::SYS_vfork,
    libc::SYS_socketpair,
    libc::SYS_pidfd_open,
    libc::SYS_pidfd_getfd,
    libc::SYS_process_madvise,
    libc::SYS_kcmp,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_open_tree,
    libc::SYS_move_mount,
    libc::SYS_fsopen,
    libc::SYS_fsmount,
    libc::SYS_fsconfig,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
];

/// socket(domain, type, protocol): refused unless TCP over IPv4 or IPv6 or a stream Unix
/// socket.
fn socket_rules() -> Vec<SeccompRule> {
    let mut socket = vec![rule(vec![
        cond(0, Op::Ne, AF_UNIX),
        cond(0, Op::Ne, AF_INET),
        cond(0, Op::Ne, AF_INET6),
    ])];
    for family in [AF_UNIX, AF_INET, AF_INET6] {
        for kind in (0..=SOCK_TYPE_MASK).filter(|kind| *kind != SOCK_STREAM) {
            socket.push(rule(vec![
                cond(0, Op::Eq, family),
                cond(1, Op::MaskedEq(SOCK_TYPE_MASK), kind),
            ]));
        }
    }
    for family in [AF_INET, AF_INET6] {
        socket.push(rule(vec![
            cond(0, Op::Eq, family),
            cond(2, Op::Ne, 0),
            cond(2, Op::Ne, IPPROTO_TCP),
        ]));
    }
    socket.push(rule(vec![cond(0, Op::Eq, AF_UNIX), cond(2, Op::Ne, 0)]));
    socket
}

/// The refusals: a syscall with no rules is refused always; with rules, when any rule
/// matches (every condition of a rule holds).
fn refusals(profile: Profile) -> BTreeMap<i64, Vec<SeccompRule>> {
    let mut map: BTreeMap<i64, Vec<SeccompRule>> =
        ALWAYS.iter().map(|number| (*number, Vec::new())).collect();
    // A process, not a thread.
    map.insert(
        libc::SYS_clone,
        vec![rule(vec![cond(0, Op::MaskedEq(CLONE_THREAD), 0)])],
    );
    map.insert(libc::SYS_socket, socket_rules());
    // TCP Fast Open: a send on an unconnected socket must not be a way around connect().
    let fast_open = |arg| {
        vec![rule(vec![cond(
            arg,
            Op::MaskedEq(MSG_FASTOPEN),
            MSG_FASTOPEN,
        )])]
    };
    map.insert(libc::SYS_sendto, fast_open(3));
    map.insert(libc::SYS_sendmsg, fast_open(2));
    map.insert(libc::SYS_sendmmsg, fast_open(3));
    map.insert(
        libc::SYS_setsockopt,
        vec![rule(vec![
            cond(1, Op::Eq, IPPROTO_TCP),
            cond(2, Op::Eq, TCP_FASTOPEN_CONNECT),
        ])],
    );
    map.insert(
        libc::SYS_prctl,
        vec![
            rule(vec![cond(0, Op::Eq, PR_SET_PDEATHSIG)]),
            rule(vec![cond(0, Op::Eq, PR_SET_NO_NEW_PRIVS)]),
        ],
    );
    if let Profile::Tcp { exec_fd } = profile {
        map.insert(libc::SYS_execve, Vec::new());
        map.insert(
            libc::SYS_execveat,
            vec![
                rule(vec![cond(0, Op::Ne, u64::from(exec_fd))]),
                rule(vec![cond(4, Op::Ne, AT_EMPTY_PATH)]),
            ],
        );
    }
    map
}

/// The program for `profile`.
pub(crate) fn program(profile: Profile) -> BpfProgram {
    let filter = SeccompFilter::new(
        refusals(profile),
        SeccompAction::Allow,
        SeccompAction::Errno(EPERM),
        TargetArch::x86_64,
    )
    .expect("allow and refuse differ");
    let compiled: BpfProgram = filter.try_into().expect("the program compiles");
    let mut program = prefix();
    program.extend(compiled);
    program
}

/// A program that makes the kernel look as if it had no Landlock: the three Landlock calls
/// answer `errno` (`ENOSYS` for a kernel built without it, `EOPNOTSUPP` for one that disabled
/// it at boot, `EPERM` for a filter around Mimik). Tests only: the live fallback tests install
/// it in the runner.
#[cfg(test)]
pub(crate) fn program_without_landlock(errno: u32) -> BpfProgram {
    // x86_64 syscall numbers (landlock_create_ruleset, landlock_add_rule,
    // landlock_restrict_self), like the rest of this file's x86_64-only program: the
    // simulation only runs where the shipped program does.
    const LANDLOCK_CALLS: [i64; 3] = [444, 445, 446];
    let filter = SeccompFilter::new(
        LANDLOCK_CALLS
            .iter()
            .map(|number| (*number, Vec::new()))
            .collect(),
        SeccompAction::Allow,
        SeccompAction::Errno(errno),
        TargetArch::x86_64,
    )
    .expect("allow and refuse differ");
    filter.try_into().expect("the program compiles")
}

/// Installs `program` on the calling thread (and sets `no_new_privs`, which seccomp needs).
///
/// # Errors
///
/// The kernel's refusal.
pub(crate) fn install(program: &[sock_filter]) -> Result<(), seccompiler::Error> {
    seccompiler::apply_filter(program)
}

#[cfg(test)]
#[path = "seccomp_tests.rs"]
mod tests;
