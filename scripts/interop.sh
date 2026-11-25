#!/usr/bin/env bash
# Run one language's interop vectors for every reference schema: generate the
# library with its interop harness, put the harness where the library's tests
# live, and run them. The vectors are encoded by the Rust backend; each harness
# decodes them, checks the values, and re-encodes them byte for byte.
#
# Usage: scripts/interop.sh <lang> [cddlc binary]   (default: target/debug/cddlc)
#   lang: rust c cpp csharp nodejs python go java lua
# Requires the language's toolchain, as its build tests do
# (crates/backend-buildtest); Python needs cbor2>=6.0 and pytest, Lua busted.
set -euo pipefail

lang="${1:?usage: scripts/interop.sh <lang> [cddlc binary]}"
cd "$(dirname "$0")/.."
cddlc="$(realpath "${2:-target/debug/cddlc}")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Add the harness to a generated CMake project as a second test.
add_cmake_test() {
    local pkg="$1" src="$2"
    local lib
    lib="$(sed -nE 's/^target_link_libraries\(test_roundtrip ([^ )]+).*/\1/p' "$pkg/CMakeLists.txt")"
    cat >> "$pkg/CMakeLists.txt" <<EOF

add_executable(test_interop $src)
target_link_libraries(test_interop $lib)
add_test(NAME interop COMMAND test_interop)
EOF
}

busted_bin() {
    command -v busted 2>/dev/null || echo "$HOME/.luarocks/bin/busted"
}

run_one() {
    local name="$1" out="$2"
    local harness="$out/$name-interop/$lang"
    local pkg
    pkg="$(find "$out" -maxdepth 1 -type d -name "$name-cbor*" | head -1)"
    case "$lang" in
        rust)
            cp "$harness/interop_test.rs" "$pkg/tests/"
            (cd "$pkg" && cargo test -q) ;;
        c)
            cp "$harness/test_interop.c" "$pkg/test/"
            add_cmake_test "$pkg" test/test_interop.c
            (cd "$pkg" && cmake -S . -B build -DCMAKE_BUILD_TYPE=Debug >/dev/null \
                && cmake --build build >/dev/null && ctest --test-dir build --output-on-failure) ;;
        cpp)
            cp "$harness/test_interop.cpp" "$pkg/test/"
            add_cmake_test "$pkg" test/test_interop.cpp
            (cd "$pkg" && cmake -S . -B build -DCMAKE_BUILD_TYPE=Debug >/dev/null \
                && cmake --build build >/dev/null && ctest --test-dir build --output-on-failure) ;;
        csharp)
            cp "$harness/InteropTests.cs" "$pkg/tests/"
            (cd "$pkg" && dotnet test --nologo -v quiet) ;;
        nodejs)
            cp "$harness/interop.test.ts" "$pkg/tests/"
            (cd "$pkg" && npm install --no-audit --no-fund >/dev/null && npm run build >/dev/null && npm test) ;;
        python)
            cp "$harness/test_interop.py" "$pkg/tests/"
            (cd "$pkg" && PYTHONPATH=src "${CDDLC_PYTHON:-python3}" -m pytest -q tests) ;;
        go)
            cp "$harness/interop_test.go" "$pkg/"
            (cd "$pkg" && go mod tidy && go test ./...) ;;
        java)
            local package
            package="$(sed -nE 's/^package ([A-Za-z0-9_.]+);/\1/p' "$harness/InteropTest.java")"
            cp "$harness/InteropTest.java" "$pkg/src/test/java/${package//.//}/"
            (cd "$pkg" && mvn -B -ntp -q test) ;;
        lua)
            cp "$harness/interop_spec.lua" "$pkg/spec/"
            (cd "$pkg" && "$(busted_bin)" -o TAP) ;;
        *)
            echo "unknown language: $lang" >&2
            exit 2 ;;
    esac
}

failed=()
for schema in crates/backend-buildtest/schemas/*.cddl; do
    name="$(basename "$schema" .cddl)"
    out="$work/$name"
    "$cddlc" generate "$schema" -l "$lang" --interop --interop-langs "$lang" -o "$out" >/dev/null 2>"$work/generate.log" \
        || { cat "$work/generate.log" >&2; failed+=("$name"); continue; }
    if run_one "$name" "$out" >"$work/run.log" 2>&1; then
        echo "== $lang $name: ok"
    else
        echo "== $lang $name: FAILED"
        cat "$work/run.log"
        failed+=("$name")
    fi
done

if (( ${#failed[@]} )); then
    echo "$lang interop failed: ${failed[*]}"
    exit 1
fi
echo "$lang interop: all schemas pass"
