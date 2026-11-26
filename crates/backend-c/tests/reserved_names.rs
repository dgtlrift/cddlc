//! A rule whose `<rule>_t` the C library or the runtime already has is
//! `<module>_<rule>_t` (dgtlrift/cddlc#154).

use std::path::PathBuf;

use backend_c::CBackend;
use cddlc_codegen::{Backend, CodegenOptions};
use cddlc_ir::lower;
use cddlc_parser::parse_cddl;

fn header(src: &str) -> String {
    let parsed = parse_cddl(src, PathBuf::from("fs.cddl")).expect("parse");
    let m = lower(&parsed.value, 16, 64).expect("lower").module;
    let out = CBackend
        .generate(&m, &CodegenOptions::default())
        .expect("generate");
    out.files
        .iter()
        .filter(|f| f.path.ends_with(".h"))
        .map(|f| f.content.clone())
        .collect()
}

#[test]
fn posix_and_runtime_names_get_the_module_prefix() {
    let h = header("file = { mode: mode, time: time, label: cbor-str }\nmode = uint\ntime = { sec: int }\ncbor-str = tstr");
    assert!(h.contains("} fs_time_t;"), "{h}");
    assert!(h.contains("fs_mode_t;"), "{h}");
    assert!(h.contains("fs_cbor_str_t;"), "{h}");
    assert!(!h.contains(" mode_t;"), "{h}");
    assert!(!h.contains("} time_t;"), "{h}");
    // Other names are unchanged.
    assert!(h.contains("} file_t;"), "{h}");
}
