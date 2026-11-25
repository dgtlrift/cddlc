#!/usr/bin/env bash
# Run the C# interop vectors for every reference schema: generate the C#
# library with its interop harness, drop the harness into the library's
# tests, and `dotnet test` it. The vectors check the encoding byte for byte.
#
# Usage: scripts/interop-csharp.sh [cddlc binary]   (default: target/debug/cddlc)
# Requires: the .NET 8 SDK (dotnet in PATH)
set -euo pipefail

cd "$(dirname "$0")/.."
cddlc="${1:-target/debug/cddlc}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

failed=()
for schema in crates/backend-buildtest/schemas/*.cddl; do
    name="$(basename "$schema" .cddl)"
    out="$work/$name"
    "$cddlc" generate "$schema" -l csharp --interop --interop-langs csharp -o "$out" >/dev/null
    cp "$out/$name-interop/csharp/InteropTests.cs" "$out/$name-cbor/tests/"
    if (cd "$out/$name-cbor" && dotnet test --nologo -v quiet); then
        echo "== $name: ok"
    else
        echo "== $name: FAILED"
        failed+=("$name")
    fi
done

if (( ${#failed[@]} )); then
    echo "C# interop failed: ${failed[*]}"
    exit 1
fi
echo "C# interop: all schemas pass"
