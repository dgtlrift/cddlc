# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
From 1.1.0 on, release-plz writes this file from the conventional commit messages.

## [Unreleased]

## [1.2.0](https://github.com/dgtlrift/cddlc/compare/v1.1.0...v1.2.0) - 2026-11-24

### Changed

A minor release, not a patch: generated code changes its API or its dependencies, so code that calls it may need updating:

- C++ enums are a `Kind kind` plus a `std::variant` payload; constructors of text, `nil` and integer-literal variants take no argument (`Status::make_Warn()`) (#49).
- C++ with `--alloc heap` makes arrays `std::vector<T> items`, with no capacity limit (#50).
- Python packages need cbor2 >= 6.0 and Python 3.11; struct fields are keyword-only (#48).
- Rust `--no-std`: `tstr` is `heapless::String<N>` and `bstr` `heapless::Vec<u8, N>` (`N` from `--max-str`), arrays are always `heapless::Vec` with their own `encode_<name>` / `decode_<name>` functions, and heapless is a plain dependency (#65).
- Rust JSON: integer-literal enum variants hold a marker type (`Priority::Variant2(PriorityVariant2)`) instead of a `u64` (#63).
- Dependencies of generated projects: TypeScript moves to cborg 7, jest 30, TypeScript 6 and an explicit `@types/node`, and its `tsconfig` lists `"types": ["node", "jest"]`; Rust to minicbor 2.3 (was `>=2`), heapless 0.9 and base64 0.23; C JSON pins cJSON to a commit instead of `master`; C++ JSON pins nlohmann/json v3.12.0 (#51, #62).

### Fixed

- *(rust)* make --no-std output build as no_std
- *(nodejs)* leave absent optional members out of CBOR maps
- *(rust)* Rust JSON bstr aliases compile; integer-literal enums keep their variant
- *(python)* rework the Python backend so every reference schema passes
- *(cpp)* convert C++ JSON members by kind and compare in tests
- *(cpp)* honor --alloc heap with std::vector arrays
- *(cpp)* rework enums so every variant type encodes and decodes
- *(nodejs)* keep CBOR floats apart from integers in mixed enums
- *(rust)* write and check the tag on tagged members
- *(cpp)* pin nlohmann/json for C++ JSON output
- *(interop)* make the Rust, C and TypeScript harnesses compile and run
- *(nodejs)* decode a CBOR enum's float variant
- *(interop)* build real values for named members; make the C++ harness compile
- *(c, cpp)* optional C++ members decode correctly; literal members get their literal in tests
- text enums use their CDDL literal on the wire in every backend
- *(cpp)* make every alias its own struct
- *(nodejs)* decode CBOR tags; encode arrays element by element
- *(nodejs)* int-literal enums, float32 and nested test values
- *(nodejs)* make the generated tests actually run
- *(c, cpp)* build on macOS with current nanocbor; pin nanocbor
- *(rust)* a present null in an optional JSON member is no longer dropped
- .size on an integer bounds its value, not a length
- members typed true, false or nil no longer generate bytes
- *(rust)* give enum variant roundtrip tests unique names

### Other

- Build tests: new C JSON, C++ JSON, Rust JSON, Rust `--no-std` and Python JSON tests; the Python test runs pytest with the interpreter in `CDDLC_PYTHON` (or `python3`) and reports a skip without cbor2 >= 6 and pytest.
- CI: `actions/checkout` v7; the release-plz action v0.5.148 with release-plz 0.4.3 (#62).
- *(cpp)* rewrite the README from real generated output
- *(cli)* give each test a unique temp dir and remove it afterwards
- *(release)* release PRs only from scripts/release.sh; the workflow only releases

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
