#!/usr/bin/env bash
# Run the workspace tests, every language's build tests and interop, with the
# languages in parallel.
#
# Usage: scripts/test-all.sh [-j N] [-o DIR] [--no-interop] [lang ...]
#   -j N          groups run at once (default: half the CPUs, at least 1)
#   -o DIR        where this run's logs go (default: target/test-all/<time>-<pid>)
#   --no-interop  build tests only
#   lang ...      only these groups: rust nodejs java dart cpp csharp c
#                 workspace python go lua (default: all, longest first)
#
# Each group runs its steps in order; groups run in parallel. Nothing two
# groups write is shared:
#   - each group has its own directory under the run's, with a log per step
#     and a `status` file (step, exit code, seconds);
#   - each group has its own TMPDIR, outside the repository (a Rust crate
#     generated under it would be taken for part of the cddlc workspace), so
#     the build tests' and interop's work directories and the tools'
#     temporary files stay apart; it's removed when the run ends;
#   - each run has its own directory, so runs don't overwrite each other;
#   - the Rust workspace is built once, before the groups start, and the build
#     tests run from that test binary, so no two `cargo` processes share its
#     target directory;
#   - the Java group runs its Maven builds one at a time, as `mvn` doesn't
#     share `~/.m2` safely. npm, pub, NuGet, Go and Cargo lock their caches.
#
# Other machines (CDDLC_TEST_HOSTS="user@host1 user@host2", spaces or commas):
# the groups are shared between this machine and those, longest first, to
# whichever has the least work so far. Each host, over ssh (BatchMode, so a
# key must log in), keeps a checkout under
# ${CDDLC_TEST_REMOTE_DIR:-~/cddlc-dist-build}/<branch>/repo: cloned from
# `origin` once, then checked out at this branch's pushed commit (its
# upstream, or its merge base with origin/main), and overwritten with every
# file that differs here (committed, uncommitted or untracked; files deleted
# here are deleted there). Its `target/` is kept, so builds stay warm. The
# host runs its share with its own scripts/test-all.sh, and its logs are
# copied back here, under the same layout. A host that can't be reached or
# synced has its groups run here instead. The host needs the toolchains and
# git; CDDLC_TEST_REMOTE_ENV (e.g. "CDDLC_PYTHON=/home/me/venv/bin/python") is set
# for its run, and CDDLC_REQUIRE_TOOLS, CDDLC_REQUIRE_REGISTRIES and
# CDDLC_NETWORK_RETRIES are passed on.
#
# Toolchains a group needs and lacks are reported and skipped, as the build
# tests and interop do (CDDLC_REQUIRE_TOOLS makes them fail). Python uses
# CDDLC_PYTHON if set.
# Exit status: 0 when every step passed, 1 otherwise.
set -uo pipefail

cd "$(dirname "$0")/.."
root="$(pwd)"

jobs=$(( $(nproc 2>/dev/null || echo 2) / 2 ))
(( jobs < 1 )) && jobs=1
out=""
interop=1
want=()
while (( $# )); do
    case "$1" in
        -j) jobs="${2:?-j needs a number}"; shift 2 ;;
        -o) out="${2:?-o needs a directory}"; shift 2 ;;
        --no-interop) interop=0; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        -*) echo "unknown option $1" >&2; exit 2 ;;
        *) want+=("$1"); shift ;;
    esac
done

# Longest first, so the long groups don't start last (a full run on 8 CPUs:
# Rust ~20 min of steps, Node ~17, Java ~16, Dart ~11, C++ ~8, C# ~8).
all=(rust nodejs java dart cpp csharp c workspace python go lua)
(( ${#want[@]} )) || want=("${all[@]}")
for g in "${want[@]}"; do
    [[ " ${all[*]} " == *" $g "* ]] || { echo "unknown group $g (one of: ${all[*]})" >&2; exit 2; }
done

out="${out:-$root/target/test-all/$(date +%Y%m%d-%H%M%S)-$$}"
mkdir -p "$out"
out="$(realpath "$out")"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/cddlc-test-all-XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
echo "test-all: $out (${#want[@]} groups, $jobs at once)"

# ── Build once ────────────────────────────────────────────────────────────────

prep="$out/prep.log"
echo "test-all: building ..."
if ! { cargo build -q --bins \
        && cargo test -q --workspace --no-run \
        && cargo test -q -p backend-buildtest --test build_validation --no-run; } >"$prep" 2>&1; then
    echo "test-all: build failed, see $prep"
    tail -20 "$prep"
    exit 1
fi
# The build tests' binary, run directly so the groups don't take Cargo's lock.
buildtests="$(cargo test -q -p backend-buildtest --test build_validation --no-run --message-format=json 2>/dev/null \
    | grep -o '"executable":"[^"]*build_validation[^"]*"' | tail -1 | sed 's/.*"executable":"//; s/"$//')"
if [[ ! -x "$buildtests" ]]; then
    echo "test-all: can't find the build_validation test binary" | tee -a "$prep"
    exit 1
fi
cddlc="$root/target/debug/cddlc"

# ── Groups ────────────────────────────────────────────────────────────────────

# The steps of a group: `build <test>` or `interop <lang>` or `workspace`.
steps_of() {
    local i=$interop
    case "$1" in
        workspace) echo "workspace" ;;
        rust)    echo "build rust"; echo "build rust_json"; echo "build rust_no_std"; (( i )) && echo "interop rust" ;;
        c)       echo "build c"; echo "build c_json"; (( i )) && echo "interop c" ;;
        cpp)     echo "build cpp"; echo "build cpp_json"; (( i )) && echo "interop cpp" ;;
        csharp)  echo "build csharp"; echo "build csharp_json"; (( i )) && echo "interop csharp" ;;
        python)  echo "build python"; echo "build python_json"; (( i )) && echo "interop python" ;;
        go)      echo "build go"; echo "build go_json"; (( i )) && echo "interop go" ;;
        java)    echo "build java"; echo "build java_json"; echo "build java_ee"; echo "build java_ee_json"; (( i )) && echo "interop java" ;;
        nodejs)  echo "build nodejs"; (( i )) && echo "interop nodejs" ;;
        lua)     echo "build lua"; echo "build lua_json"; (( i )) && echo "interop lua" ;;
        dart)    echo "build dart"; echo "build dart_json" ;;
    esac
    return 0
}

# Runs one group's steps in order, in its own directory and TMPDIR.
run_group() {
    local g="$1" dir="$out/$1" tmp="$scratch/$1"
    mkdir -p "$dir" "$tmp"
    : >"$dir/status"
    local kind arg name log start rc
    while read -r kind arg; do
        name="${kind}${arg:+-$arg}"
        log="$dir/$name.log"
        start=$(date +%s)
        case "$kind" in
            workspace) TMPDIR="$tmp" cargo test -q --workspace --no-fail-fast >"$log" 2>&1 ;;
            build)     (cd crates/backend-buildtest && TMPDIR="$tmp" "$buildtests" --ignored --exact "test_build_$arg") >"$log" 2>&1 ;;
            interop)   TMPDIR="$tmp" scripts/interop.sh "$arg" "$cddlc" >"$log" 2>&1 ;;
        esac
        rc=$?
        echo "$name $rc $(( $(date +%s) - start ))" >>"$dir/status"
        printf 'test-all: %-8s %-22s %s (%ss)\n' "$g" "$name" "$( (( rc == 0 )) && echo ok || echo "FAIL ($rc)")" "$(( $(date +%s) - start ))"
    done < <(steps_of "$g")
}

# ── Other machines ────────────────────────────────────────────────────────────

# Minutes a group's steps take, measured on 8 CPUs: what the groups are shared by.
weight() {
    case "$1" in
        rust) echo 20 ;; nodejs) echo 17 ;; java) echo 16 ;; dart) echo 11 ;;
        cpp) echo 8 ;; csharp) echo 8 ;; c) echo 5 ;; *) echo 1 ;;
    esac
}

hosts=()
read -r -a hosts <<<"$(echo "${CDDLC_TEST_HOSTS:-}" | tr ',' ' ')"
ssh_opts=(-o BatchMode=yes -o ConnectTimeout=10)

# A command line for the host's shell: ssh joins its arguments with spaces,
# so each is quoted.
rcmd() {
    local s="" a
    for a in "$@"; do s+="$(printf '%q' "$a") "; done
    echo "$s"
}
remote_root="${CDDLC_TEST_REMOTE_DIR:-~/cddlc-dist-build}"
branch="$(git rev-parse --abbrev-ref HEAD)"
branch_dir="$(echo "$branch" | tr -c 'A-Za-z0-9._-' '_')"

# This branch's commit the hosts can fetch: its upstream when that's pushed
# and behind it, else where it left origin/main.
base_commit() {
    local up
    if up="$(git rev-parse --verify -q '@{u}' 2>/dev/null)" && git merge-base --is-ancestor "$up" HEAD; then
        echo "$up"
    else
        git merge-base HEAD origin/main
    fi
}

# Brings a host's checkout to this one: `base`, then every file that differs.
sync_host() {
    local host="$1" base="$2" log="$3" url
    url="$(git remote get-url origin)"
    ssh "${ssh_opts[@]}" "$host" "$(rcmd bash -s -- "$remote_root" "$branch_dir" "$url" "$base")" >>"$log" 2>&1 <<'EOS' || return 1
set -eu
root="${1/#\~/$HOME}"; dir="$root/$2"
mkdir -p "$dir"
# One run per checkout at a time; a lock older than 6 hours is stale.
if ! mkdir "$dir/.lock" 2>/dev/null; then
    if [ -n "$(find "$dir/.lock" -maxdepth 0 -mmin +360 2>/dev/null)" ]; then
        rm -rf "$dir/.lock"; mkdir "$dir/.lock"
    else
        echo "busy: another run holds $dir/.lock"; exit 1
    fi
fi
[ -d "$dir/repo/.git" ] || git clone -q "$3" "$dir/repo"
cd "$dir/repo"
git fetch -q origin
git checkout -q --force --detach "$4"
git clean -q -fd
EOS
    # Files that differ here from `base`: committed, uncommitted and untracked.
    { git diff --name-only --diff-filter=d -z "$base"; git ls-files -z --others --exclude-standard; } \
        | tar --null -T - -cf - 2>>"$log" \
        | ssh "${ssh_opts[@]}" "$host" "cd $remote_root/$branch_dir/repo && tar -xf -" >>"$log" 2>&1 || return 1
    git diff --name-only --diff-filter=D -z "$base" \
        | ssh "${ssh_opts[@]}" "$host" "cd $remote_root/$branch_dir/repo && xargs -0 -r rm -f --" >>"$log" 2>&1 || return 1
}

# Runs `groups` on `host` and copies their logs back; prints the groups it
# couldn't run there to `fallback`.
run_remote() {
    local host="$1" fallback="$2"; shift 2
    local groups=("$@") hdir="$out/hosts/$(echo "$host" | tr -c 'A-Za-z0-9._-' '_')"
    local log="$hdir/remote.log" base run
    mkdir -p "$hdir"
    base="$(base_commit)"
    echo "test-all: $host: syncing (base $(git rev-parse --short "$base"), ${groups[*]})"
    if ! sync_host "$host" "$base" "$log"; then
        echo "test-all: $host: can't sync, running ${groups[*]} here (see $log)"
        printf '%s\n' "${groups[@]}" >>"$fallback"
        return 0
    fi
    run="$(basename "$out")"
    local env=()
    for v in CDDLC_REQUIRE_TOOLS CDDLC_REQUIRE_REGISTRIES CDDLC_NETWORK_RETRIES; do
        [[ -n "${!v:-}" ]] && env+=("$v=${!v}")
    done
    ssh "${ssh_opts[@]}" "$host" "$(rcmd bash -s -- "$remote_root" "$branch_dir" "$run" "$interop" \
        "${CDDLC_TEST_REMOTE_ENV:-} ${env[*]}" "${groups[@]}")" <<'EOS' 2>&1 | tee -a "$log" | sed -u "s/^test-all:/test-all: [$host]/" | grep --line-buffered '^test-all:'
root="${1/#\~/$HOME}"; dir="$root/$2"; run="$3"; interop="$4"; envs="$5"; shift 5
cd "$dir/repo" || exit 1
flags=(); [ "$interop" = 1 ] || flags+=(--no-interop)
env CDDLC_TEST_HOSTS= $envs scripts/test-all.sh -o "$dir/out/$run" "${flags[@]}" "$@"
rc=$?
rmdir "$dir/.lock" 2>/dev/null
exit $rc
EOS
    # The host's logs, under this run's layout; a group it didn't finish
    # runs here.
    local got="$hdir/out"
    mkdir -p "$got"
    ssh "${ssh_opts[@]}" "$host" "cd $remote_root/$branch_dir/out/$run 2>/dev/null && tar -cf - ." 2>>"$log" | tar -C "$got" -xf - 2>>"$log"
    for g in "${groups[@]}"; do
        if [[ -s "$got/$g/status" ]]; then
            rm -rf "${out:?}/$g"
            mv "$got/$g" "$out/$g"
            echo "$host" >"$out/$g/host"
        else
            echo "test-all: $host: no results for $g, running it here"
            echo "$g" >>"$fallback"
        fi
    done
}

# ── Run, at most $jobs groups at once ─────────────────────────────────────────

started=$(date +%s)

# Share the groups: longest first, each to the machine with the least so far.
mapfile -t want < <(for g in "${want[@]}"; do echo "$(weight "$g") $g"; done | sort -k1,1nr -s | awk '{print $2}')
declare -A load share
slots=(local "${hosts[@]}")
for h in "${slots[@]}"; do load[$h]=0; share[$h]=""; done
for g in "${want[@]}"; do
    best=local
    for h in "${slots[@]}"; do (( load[$h] < load[$best] )) && best=$h; done
    load[$best]=$(( load[$best] + $(weight "$g") ))
    share[$best]+="$g "
done
fallback="$out/fallback"
: >"$fallback"
remotes=()
for h in "${hosts[@]}"; do
    [[ -n "${share[$h]}" ]] || continue
    # shellcheck disable=SC2086
    run_remote "$h" "$fallback" ${share[$h]} &
    remotes+=($!)
done
read -r -a local_groups <<<"${share[local]}"

started=${started:-$(date +%s)}
run_local() {
    local running=0 g
    for g in "$@"; do
        if (( running >= jobs )); then
            wait -n
            running=$(( running - 1 ))
        fi
        run_group "$g" &
        running=$(( running + 1 ))
    done
    wait
}
run_local "${local_groups[@]}" &
local_pid=$!
for p in "${remotes[@]}"; do wait "$p"; done
wait "$local_pid"
# Groups a host couldn't run.
if [[ -s "$fallback" ]]; then
    mapfile -t again <"$fallback"
    run_local "${again[@]}"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

failed=0
summary="$out/summary.txt"
{
    echo
    printf '%-10s %-22s %-6s %-7s %s\n' group step result seconds where
} >"$summary"
for g in "${want[@]}"; do
    while read -r name rc secs; do
        if (( rc == 0 )); then res=ok; else res=FAIL; failed=1; fi
        where=local; [[ -s "$out/$g/host" ]] && where="$(cat "$out/$g/host")"
        printf '%-10s %-22s %-6s %-7s %s\n' "$g" "$name" "$res" "$secs" "$where" >>"$summary"
    done <"$out/$g/status"
done
{
    echo
    echo "wall time: $(( $(date +%s) - started ))s; logs: $out/<group>/<step>.log"
} >>"$summary"
cat "$summary"
exit $failed
