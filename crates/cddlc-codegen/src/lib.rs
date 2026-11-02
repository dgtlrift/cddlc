//! # cddlc-codegen
//!
//! Backend trait and shared emit utilities for `cddlc`.

pub mod backend;
pub mod emit;

pub use backend::{
    header_lines, header_text, AllocStrategy, Backend, CodegenError, CodegenOptions,
    CodegenOutput, Format, GeneratedFile, Language, GENERATED,
};
pub use emit::{
    capacity_value, constraint_to_c_check, constraint_to_cpp_check, constraint_to_rust_check,
    literal_to_c, literal_to_rust, primitive_to_c, primitive_to_rust,
    to_pascal_case, to_screaming_snake, to_snake_case, IndentWriter,
    emit_doc_lines, with_source_docs, DocStyle,
};
