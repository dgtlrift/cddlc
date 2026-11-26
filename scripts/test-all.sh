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

# ── Run, at most $jobs groups at once ─────────────────────────────────────────

started=$(date +%s)
running=0
for g in "${want[@]}"; do
    if (( running >= jobs )); then
        wait -n
        running=$(( running - 1 ))
    fi
    run_group "$g" &
    running=$(( running + 1 ))
done
wait

# ── Summary ───────────────────────────────────────────────────────────────────

failed=0
summary="$out/summary.txt"
{
    echo
    printf '%-10s %-22s %-6s %s\n' group step result seconds
} >"$summary"
for g in "${want[@]}"; do
    while read -r name rc secs; do
        if (( rc == 0 )); then res=ok; else res=FAIL; failed=1; fi
        printf '%-10s %-22s %-6s %s\n' "$g" "$name" "$res" "$secs" >>"$summary"
    done <"$out/$g/status"
done
{
    echo
    echo "wall time: $(( $(date +%s) - started ))s; logs: $out/<group>/<step>.log"
} >>"$summary"
cat "$summary"
exit $failed
