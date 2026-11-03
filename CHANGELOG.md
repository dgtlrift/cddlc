# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
From 1.1.0 on, release-plz writes this file from the conventional commit messages.

## [Unreleased]

## [1.1.0] - 2026-11-02

The release candidates v1.1.0-rc.1 to rc.3, as a release, now from one repository.

### Added

- `cddlc validate` and the `cddlc-validate` crate: check JSON and CBOR documents against a schema.
- CDDL comments become doc comments in every backend (#8).
- Each generated type's and field's doc names its CDDL source line; `--no-source-comments` leaves it out (#9).
- Every generated source, header and test file opens with the cddlc release, commit, branch and remote that made it, and its inputs; `cddlc --version` reports them (#10).
- A static x86-64 Linux binary (musl) with each release, with its SHA-256.

### Fixed

- An inline array or map field type is generated as its own named type, not as bytes (#6).
- nodejs: JSON output fixes; decoders check primitive types; fields named like the decoder's locals (#2, #3, #5).
- rust: optional `bstr` defaults, CBOR heap fixes, native byte strings in binary formats; the optional-field deref; `(*v)` patterns (#1, #4).

### Other

- One repository: the 14 crates, until now submodules each in their own repo, are merged in under `crates/` with their whole history (git subtree, at the commits this repository pinned). They share the workspace version.
- Releases are made with release-plz (`release-plz.toml`, `scripts/release.sh`, `.github/workflows/release-plz.yml`); nothing is published to crates.io.
