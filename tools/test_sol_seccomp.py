import importlib.util
import struct
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "sol_seccomp", Path(__file__).with_name("sol_seccomp.py")
)
seccomp = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(seccomp)


def run(program, nr, arch=seccomp.AUDIT_ARCH_X86_64):
    """Interpret the filter for one call, as the kernel does."""
    data = {0: nr, 4: arch}
    pc, acc = 0, 0
    while True:
        code, jt, jf, k = program[pc]
        if code == seccomp.BPF_LD_W_ABS:
            acc = data[k]
        elif code == seccomp.BPF_JEQ_K:
            pc += jt if acc == k else jf
        elif code == seccomp.BPF_JGE_K:
            pc += jt if acc >= k else jf
        elif code == seccomp.BPF_RET_K:
            return k
        pc += 1


class Filter(unittest.TestCase):
    program = seccomp.program()
    errno = seccomp.SECCOMP_RET_ERRNO | seccomp.EPERM

    def test_every_denied_call_fails_with_eperm(self):
        for name, nr in seccomp.DENIED.items():
            self.assertEqual(run(self.program, nr), self.errno, name)

    def test_ordinary_calls_are_allowed(self):
        for nr in (0, 1, 2, 9, 56, 57, 59, 231, 257, 435):  # read … openat, clone3
            self.assertEqual(run(self.program, nr), seccomp.SECCOMP_RET_ALLOW, nr)

    def test_x32_and_foreign_architectures_are_killed(self):
        self.assertEqual(
            run(self.program, seccomp.X32_SYSCALL_BIT | 1), seccomp.SECCOMP_RET_KILL_PROCESS
        )
        self.assertEqual(run(self.program, 0, arch=0x40000003), seccomp.SECCOMP_RET_KILL_PROCESS)

    def test_the_encoding_is_eight_bytes_per_instruction(self):
        blob = seccomp.encode(self.program)
        self.assertEqual(len(blob), 8 * len(self.program))
        self.assertEqual(struct.unpack("<HBBI", blob[:8]), self.program[0])


if __name__ == "__main__":
    unittest.main()
