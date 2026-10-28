#!/bin/sh
# Run Sol (openai/gpt-6.1-sol through opencode) as Mimik's implementer inside bubblewrap.
#
# usage: tools/sol_agent.sh <worktree> <task file> <state dir> [command...]
#   A command, when given, runs in the same sandbox instead of the agent (to check it).
#   CPUS (default 2): cores for the agent and its builds (weekdays 09-18: at most 2).
#   MODEL (default openai/gpt-6.1-sol), TIMEOUT seconds (default 7200).
#
# The sandbox sees, read-only: /usr, /bin, /sbin, /lib, /lib64, a few /etc files, the Rust
# toolchain (~/.rustup, ~/.cargo/bin), the Cargo registry and the opencode login file.
# Read-write: the worktree, <state dir>/home (HOME: opencode's config, database and logs),
# <state dir>/target (CARGO_TARGET_DIR) and <state dir>/mailbox. Nothing else of the user's
# home, no environment variables of the caller, no /media, no Docker socket, no other worktree, no git metadata (the worktree's
# .git file points outside the sandbox, so git does not work). The network is shared,
# because the agent must reach its model; Cargo runs offline. All namespaces but the
# network are new, nested user namespaces are disabled, and tools/sol_seccomp.py's filter
# denies the kernel interfaces a build never needs.
#
# Mailbox: the agent runs `ask "question"` (on PATH in the sandbox). It writes
# mailbox/question and waits for mailbox/answer. The coordinator watches for the question
# file, writes the answer file, and `ask` prints it and removes both.
set -eu

[ $# -ge 3 ] || { sed -n '4,8p' "$0" >&2; exit 2; }
worktree=$(realpath "$1")
task=$(realpath "$2")
state=$(realpath -m "$3")
shift 3
cpus=${CPUS:-2}
model=${MODEL:-openai/gpt-6.1-sol}
timeout=${TIMEOUT:-7200}
tools=$(dirname "$(realpath "$0")")
auth="$HOME/.local/share/opencode/auth.json"
command -v opencode >/dev/null || { echo "opencode is not installed" >&2; exit 2; }

[ -f "$worktree/Cargo.toml" ] || { echo "not a Mimik worktree: $worktree" >&2; exit 2; }
[ -f "$worktree/.git" ] || { echo "run it on a git worktree, not the main checkout" >&2; exit 2; }
[ -r "$auth" ] || { echo "opencode is not logged in" >&2; exit 2; }

mkdir -p "$state/home/.config/opencode" "$state/home/.local/share/opencode" \
    "$state/target" "$state/mailbox" "$state/bin"
cp "$tools/sol_opencode.json" "$state/home/.config/opencode/opencode.json"
: > "$state/home/.local/share/opencode/auth.json"
python3 -I "$tools/sol_seccomp.py" "$state/seccomp.bpf"
cat > "$state/bin/ask" <<'EOF'
#!/bin/sh
# ask "question": wait for the coordinator's answer (up to an hour).
[ $# -ge 1 ] || { echo 'usage: ask "question"' >&2; exit 2; }
rm -f /mailbox/answer
printf '%s\n' "$*" > /mailbox/question.tmp && mv /mailbox/question.tmp /mailbox/question
i=0
while [ ! -f /mailbox/answer ]; do
    i=$((i + 1)); [ $i -le 3600 ] || { echo "no answer within an hour; decide yourself and say so" ; exit 1; }
    sleep 1
done
cat /mailbox/answer; rm -f /mailbox/answer /mailbox/question
EOF
chmod +x "$state/bin/ask"

cores=$(seq -s, 0 $((cpus - 1)))
etc=""
for file in resolv.conf hosts nsswitch.conf passwd group ld.so.cache ld.so.conf localtime \
    ssl ca-certificates gai.conf host.conf; do
    [ -e "/etc/$file" ] && etc="$etc --ro-bind /etc/$file /etc/$file"
done

# Secrets files in the worktree are replaced by an empty read-only file.
masks=""
for file in "$worktree"/.env "$worktree"/.env.*; do
    [ -e "$file" ] && masks="$masks --ro-bind /dev/null $file"
done

# shellcheck disable=SC2086 # $etc and $masks are argument lists.
exec 9<"$state/seccomp.bpf"
exec timeout "$timeout" taskset -c "$cores" bwrap \
    --die-with-parent --new-session \
    --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup-try \
    --disable-userns --seccomp 9 \
    --ro-bind /usr /usr --ro-bind /bin /bin --ro-bind /sbin /sbin \
    --ro-bind /lib /lib --ro-bind /lib64 /lib64 \
    $etc \
    --proc /proc --dev /dev --tmpfs /tmp \
    --bind "$state/home" "$HOME" \
    --ro-bind "$auth" "$HOME/.local/share/opencode/auth.json" \
    --ro-bind "$HOME/.rustup" "$HOME/.rustup" \
    --ro-bind "$HOME/.cargo/bin" "$HOME/.cargo/bin" \
    --ro-bind "$HOME/.cargo/registry" "$HOME/.cargo/registry" \
    --ro-bind "$state/bin/ask" /opt/sol/ask \
    --bind "$state/mailbox" /mailbox \
    --bind "$state/target" /target \
    --bind "$worktree" "$worktree" \
    $masks \
    --chdir "$worktree" \
    --clearenv \
    --setenv HOME "$HOME" --setenv USER "$(id -un)" --setenv LANG C.UTF-8 --setenv TERM dumb \
    --setenv PATH "/opt/sol:$HOME/.cargo/bin:/usr/bin:/bin" \
    --setenv CARGO_TARGET_DIR /target \
    --setenv CARGO_BUILD_JOBS "$cpus" \
    --setenv CARGO_NET_OFFLINE true \
    "${@:-opencode}" ${1:+} $( [ $# -eq 0 ] && printf '%s' '' )
