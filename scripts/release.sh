#!/bin/sh
# cddlc's releases with release-plz (release-plz.toml), and the static binary attached to
# each. Needs release-plz, a Rust toolchain (rustup) and gh on the PATH, and GIT_TOKEN
# (for example "$(gh auth token)").
#   scripts/release.sh pr       opens or updates the release PR: next version, CHANGELOG.md
#   scripts/release.sh release  after that PR is merged: tags v<version>, makes the release
#   scripts/release.sh binary   builds the x86_64 musl binary of the tag at HEAD and
#                               attaches it, with its .sha256, to that tag's release
#   scripts/release.sh preview  writes the next version and changelog locally, to look at
# .github/workflows/release-plz.yml does the same in GitHub Actions.
set -eu
cd "$(dirname "$0")/.."
target=x86_64-unknown-linux-musl
case "${1:-}" in
  pr) exec release-plz release-pr --git-token "${GIT_TOKEN:?set GIT_TOKEN}" --forge github ;;
  release) exec release-plz release --git-token "${GIT_TOKEN:?set GIT_TOKEN}" --forge github ;;
  preview) exec release-plz update ;;
  binary)
    tag="$(git describe --tags --exact-match HEAD)"
    rustup target add "$target" >/dev/null
    cargo build --release --locked --target "$target" -p cddlc-cli
    asset="cddlc-$tag-$target"
    out="$(mktemp -d)"
    mkdir "$out/$asset"
    cp "target/$target/release/cddlc" LICENSE README.md "$out/$asset/"
    tar -czf "$out/$asset.tar.gz" -C "$out" "$asset"
    (cd "$out" && sha256sum "$asset.tar.gz" > "$asset.tar.gz.sha256")
    GH_TOKEN="${GIT_TOKEN:?set GIT_TOKEN}" gh release upload "$tag" \
      "$out/$asset.tar.gz" "$out/$asset.tar.gz.sha256" --clobber
    cat "$out/$asset.tar.gz.sha256"
    ;;
  *)
    echo "usage: scripts/release.sh pr | release | binary | preview" >&2
    exit 2
    ;;
esac
