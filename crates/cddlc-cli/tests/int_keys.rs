//! Every backend writes an integer map key as an integer, not as the member's
//! name (dgtlrift/cddlc#176). The build tests and interop run it end to end;
//! this checks the generated code for the languages CI builds only.

use std::path::Path;
use std::process::Command;

fn generate(lang: &str, out: &Path) -> String {
    let schema = Path::new(env!("CARGO_MANIFEST_DIR")).join("../backend-buildtest/schemas/int_keys.cddl");
    let ok = Command::new(env!("CARGO_BIN_EXE_cddlc"))
        .args(["generate", "-l", lang, "-o"]).arg(out).arg(schema)
        .status().expect("run cddlc").success();
    assert!(ok, "{lang}: generate failed");
    let mut src = String::new();
    for entry in walk(out) {
        if !entry.to_string_lossy().contains("test") {
            src.push_str(&std::fs::read_to_string(&entry).unwrap_or_default());
        }
    }
    src
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}

#[test]
fn integer_keys_are_written_as_integers() {
    let tmp = tempfile::tempdir().unwrap();
    // (language, what it writes for the keys, what writing the name would look like)
    let cases: &[(&str, &[&str], &str)] = &[
        ("rust", &["e.u64(1)?;", "e.i64(-1)?;"], "e.str(\"field_1\")"),
        ("c", &["nanocbor_fmt_uint(enc, 1ULL);", "nanocbor_fmt_int(enc, -1LL);"], "nanocbor_put_tstr(enc, \"field_1\")"),
        ("cpp", &["nanocbor_fmt_uint(enc, 1ULL);", "nanocbor_fmt_int(enc, -1LL);"], "nanocbor_put_tstr(enc, \"field_1\")"),
        ("csharp", &["writer.WriteUInt64(1UL);", "writer.WriteInt64(-1L);"], "WriteTextString(\"field_1\")"),
        ("nodejs", &["enc.uint(1n);", "enc.int(-1n);", "useMaps: true"], "enc.text(\"field_1\")"),
        ("python", &["m[1] =", "m[-1] ="], "m[\"field_1\"]"),
        ("go", &["e.put(uint64(1))", "e.put(int64(-1))", "m[uint64(1)]"], "e.put(\"field_1\")"),
        ("java", &["m.put(1L,", "m.put(-1L,"], "m.put(\"field_1\""),
        ("lua", &["R.int(1)", "R.int(-1)"], "{ \"field_1\","),
        ("dart", &["CborSmallInt(1)", "CborSmallInt(-1)"], "CborString('field_1')"),
    ];
    for (lang, wants, name_as_key) in cases {
        let src = generate(lang, &tmp.path().join(lang));
        assert!(!src.contains(name_as_key), "{lang}: writes the member's name as the key");
        for want in *wants {
            assert!(src.contains(want), "{lang}: no `{want}`");
        }
    }
}
