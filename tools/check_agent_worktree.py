"""Check an agent's git worktree before any host tool touches it (AGENTS.md, implementers).

Run it from the main checkout, never from the worktree under check:

    python3 -I tools/check_agent_worktree.py --main . WORKTREE [--clean] [--review]

Without git, it refuses: a root `.git` that is not the regular file `git worktree add` wrote
(pointing at `<main>/.git/worktrees/<name>`, whose `gitdir` points back); any other `.git` or
`.jj` entry (a nested repository's configuration, such as `core.fsmonitor`, runs on the host's
next `git status` once it is staged); symbolic links; FIFOs, sockets and devices. Only then
does it run git, hardened (no fsmonitor, hooks, external diff or submodules), to refuse
gitlink and symlink entries in the index and to list ignored paths, which `git status` does
not show and the host may still execute (`__pycache__`, `target/`, a `.cargo/config.toml`).
It also refuses `.gitignore` files the run added or
changed, since they decide what counts as ignored. `--clean` deletes those ignored paths.
Run it only after the agent has exited (the checks and deletions are not atomic). `--review` refuses files a review snapshot would send
(tracked and untracked non-ignored) whose names the old review wrapper excluded.

Exit 0 when clean, 1 when refused (each reason on stderr), 2 on a usage error.
"""

import argparse
import fnmatch
import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path

VCS = (".git", ".jj")
# Names a review snapshot must never carry (the former tools/external_review.py exclusions).
REVIEW_EXCLUDED_NAMES = (".env", ".env.*", "*.sqlite3*", "*.pdf", "*.gguf")
REVIEW_EXCLUDED_DIRS = ("data", "target")
REVIEW_ALLOWED = (".env.example",)
GIT = (
    "git",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "diff.external=",
    "-c",
    "core.untrackedCache=false",
    "--no-pager",
)


def git(worktree, *args):
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "GIT_CONFIG_NOSYSTEM": "1"}
    env["GIT_CONFIG_GLOBAL"] = os.devnull
    result = subprocess.run([*GIT, "-C", str(worktree), *args], capture_output=True, env=env)
    if result.returncode != 0:
        sys.exit(
            f"error: git {' '.join(args)} failed: {result.stderr.decode(errors='replace').strip()}"
        )
    return result.stdout


def check_gitfile(main, worktree):
    """The root .git must be the plain gitdir file pointing at this worktree's admin dir."""
    try:
        return gitfile_reasons(main, worktree)
    except (OSError, UnicodeDecodeError) as error:
        return [f".git cannot be checked: {error.__class__.__name__}"]


def gitfile_reasons(main, worktree):
    path = worktree / ".git"
    info = os.lstat(path)
    if not stat.S_ISREG(info.st_mode):
        return [".git is not a regular file"]
    content = path.read_bytes()
    prefix = b"gitdir: "
    if not content.startswith(prefix) or not content.endswith(b"\n") or content.count(b"\n") != 1:
        return [".git is not a single gitdir line"]
    admin = Path(content[len(prefix) : -1].decode(errors="replace"))
    worktrees = (main / ".git" / "worktrees").resolve()
    if admin.parent != worktrees or admin.name in ("", ".", "..") or not admin.is_dir():
        return [f".git points outside {worktrees}: {admin}"]
    back = (admin / "gitdir").read_text().strip()
    if Path(back) != worktree / ".git":
        return [f"{admin}/gitdir points at {back}, not this worktree"]
    return []


# Files that make a directory a repository git would use without a `.git` name: a bare
# repository, or one split through `commondir` (its objects, refs and config live elsewhere).
REPOSITORY_FILES = ("HEAD", "commondir", "gitdir")


def bare_repository(path):
    """A directory git could take for a repository even without a `.git` name."""
    names = set(os.listdir(path))
    return bool(names & set(REPOSITORY_FILES)) or {"objects", "refs"} <= names


def walk(worktree):
    """Refusals for VCS entries, bare repositories, links, special and unreadable entries
    anywhere in the tree. Any error while listing refuses (an unreadable directory could hide
    a repository)."""
    reasons = []

    def failed(error):
        reasons.append(
            f"cannot list {Path(error.filename).relative_to(worktree)}: {error.strerror}"
        )

    for root, directories, files in os.walk(worktree, onerror=failed, followlinks=False):
        here = Path(root)
        if here != worktree and bare_repository(here):
            reasons.append(f"bare repository: {here.relative_to(worktree)}")
        for name in [*directories, *files]:
            path = here / name
            relative = path.relative_to(worktree)
            mode = os.lstat(path).st_mode
            if name.casefold() in VCS and relative != Path(".git"):
                reasons.append(f"nested VCS entry: {relative}")
            elif stat.S_ISLNK(mode):
                reasons.append(f"symbolic link: {relative}")
            elif not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
                reasons.append(f"special file: {relative}")
            elif mode & stat.S_IRUSR == 0 or (stat.S_ISDIR(mode) and mode & stat.S_IXUSR == 0):
                reasons.append(f"not readable by its owner: {relative}")
        directories[:] = [d for d in directories if d.casefold() not in VCS]
    return reasons


def ignore_rule_changes(worktree):
    """`.gitignore` files are what decides which paths count as ignored: one the agent added
    or changed could hide a planted file from `--clean` or make it delete real work."""
    changed = git(worktree, "diff", "--name-only", "-z", "HEAD", "--")
    untracked = git(worktree, "ls-files", "-z", "--others")
    reasons = []
    for entry in filter(None, (changed + b"\0" + untracked).split(b"\0")):
        relative = Path(os.fsdecode(entry))
        if relative.name.casefold() == ".gitignore":
            reasons.append(f"ignore rules changed by the run: {relative}")
    return reasons


def index_entries(worktree):
    reasons = []
    for entry in git(worktree, "ls-files", "-s", "-z").split(b"\0"):
        if entry.startswith((b"160000 ", b"120000 ")):
            kind = "gitlink" if entry.startswith(b"160000") else "symlink"
            reasons.append(
                f"{kind} in the index: {entry.split(b'\t', 1)[-1].decode(errors='replace')}"
            )
    return reasons


def ignored(worktree):
    listing = git(
        worktree, "ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"
    )
    return [Path(os.fsdecode(entry).rstrip("/")) for entry in listing.split(b"\0") if entry]


def review_refusals(worktree):
    reasons = []
    listing = git(worktree, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    for entry in filter(None, listing.split(b"\0")):
        relative = Path(os.fsdecode(entry))
        if relative.name in REVIEW_ALLOWED:
            continue
        if any(part in REVIEW_EXCLUDED_DIRS for part in relative.parts[:-1]) or any(
            fnmatch.fnmatch(relative.name, pattern) for pattern in REVIEW_EXCLUDED_NAMES
        ):
            reasons.append(f"a review snapshot would carry {relative}")
    return reasons


def main(argv):
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--main", type=Path, required=True, help="The main checkout")
    parser.add_argument("worktree", type=Path)
    parser.add_argument("--clean", action="store_true", help="Delete ignored paths")
    parser.add_argument(
        "--review", action="store_true", help="Also check what a review snapshot would send"
    )
    args = parser.parse_args(argv)
    main_checkout = args.main.resolve()
    worktree = args.worktree.absolute()
    if worktree.resolve() != worktree or not worktree.is_dir():
        parser.error("the worktree must be an existing directory given without symbolic links")
    if not (main_checkout / ".git").is_dir():
        parser.error("--main must be the main checkout")
    if worktree == main_checkout:
        # The main checkout is the coordinator's own: only what a review would send is checked.
        if args.clean or not args.review:
            parser.error("the main checkout takes --review only")
        reasons = review_refusals(worktree)
        for reason in reasons:
            print(f"refused: {reason}", file=sys.stderr)
        return 1 if reasons else 0
    reasons = check_gitfile(main_checkout, worktree) + walk(worktree)
    if reasons:
        # Git must not run on such a tree.
        for reason in reasons:
            print(f"refused: {reason}", file=sys.stderr)
        return 1
    reasons = index_entries(worktree) + ignore_rule_changes(worktree)
    if reasons:
        for reason in reasons:
            print(f"refused: {reason}", file=sys.stderr)
        return 1
    leftovers = ignored(worktree)
    for path in sorted(set(leftovers)):
        if args.clean:
            target = worktree / path
            if not os.path.lexists(target):
                continue  # under a directory already deleted
            try:
                if target.is_dir():
                    shutil.rmtree(target)
                else:
                    target.unlink()
            except OSError as error:
                reasons.append(f"cannot delete ignored path {path}: {error.strerror or error}")
                continue
            print(f"deleted ignored path: {path}", file=sys.stderr)
        else:
            reasons.append(f"ignored path (delete with --clean): {path}")
    if args.review:
        reasons += review_refusals(worktree)
    for reason in reasons:
        print(f"refused: {reason}", file=sys.stderr)
    return 1 if reasons else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
