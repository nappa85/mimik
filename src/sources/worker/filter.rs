//! The second, irreversible worker filter. The namespace filter remains underneath it.
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};

pub(super) const INSTALL_FAILED: i32 = 26;
const KILLED: [i64; 12] = [
    libc::SYS_clone,
    libc::SYS_clone3,
    libc::SYS_fork,
    libc::SYS_vfork,
    libc::SYS_execve,
    libc::SYS_execveat,
    libc::SYS_socket,
    libc::SYS_socketpair,
    libc::SYS_memfd_create,
    libc::SYS_userfaultfd,
    libc::SYS_ptrace,
    libc::SYS_seccomp,
];

fn program() -> BpfProgram {
    SeccompFilter::new(
        KILLED.into_iter().map(|nr| (nr, vec![])).collect(),
        SeccompAction::Allow,
        SeccompAction::KillProcess,
        TargetArch::x86_64,
    )
    .expect("fixed worker filter")
    .try_into()
    .expect("fixed worker BPF")
}

pub(super) fn install() -> std::io::Result<()> {
    let program = program();
    let descriptor = libc::sock_fprog {
        len: u16::try_from(program.len()).expect("short worker filter"),
        filter: program.as_ptr().cast::<libc::sock_filter>().cast_mut(),
    };
    // Only the re-executed libtest worker can request an actual kernel install failure.
    #[cfg(test)]
    let descriptor = {
        let mut descriptor = descriptor;
        if std::env::var("MIMIK_PDF_TEST").as_deref() == Ok("filter-fail") {
            descriptor.len = 0;
        }
        descriptor
    };
    // SAFETY: the kernel copies this valid, bounded BPF program during the call; both
    // descriptor and instructions outlive it. no_new_privs was set by the first layer.
    let result = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_TSYNC,
            &raw const descriptor,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Interpret the compiler's syscall-dispatch instructions, independent of their layout.
    fn action(nr: i64) -> u32 {
        let code = program();
        let mut pc = 0;
        let mut acc = 0;
        loop {
            let insn = &code[pc];
            match insn.code {
                0x20 => {
                    acc = match insn.k {
                        0 => u32::try_from(nr).unwrap(),
                        4 => 0xc000_003e,
                        _ => panic!("unexpected load"),
                    }
                }
                0x15 => pc += usize::from(if acc == insn.k { insn.jt } else { insn.jf }),
                0x05 => pc += usize::try_from(insn.k).unwrap(),
                0x06 => return insn.k,
                _ => panic!("unexpected BPF instruction"),
            }
            pc += 1;
        }
    }
    #[test]
    fn exact_syscall_set_is_kill_process_default_allow() {
        let expected = [56, 435, 57, 58, 59, 322, 41, 53, 319, 323, 101, 317];
        assert_eq!(KILLED, expected);
        for nr in 0..=500 {
            assert_eq!(
                action(nr),
                if expected.contains(&nr) {
                    0x8000_0000
                } else {
                    0x7fff_0000
                },
                "syscall {nr}"
            );
        }
    }
}
