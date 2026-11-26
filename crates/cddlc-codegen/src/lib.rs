//! # cddlc-codegen
//!
//! Backend trait and shared emit utilities for `cddlc`.

pub mod backend;
pub mod emit;

pub use backend::{
    cddl_boilerplate, comment_lines, header_comment, header_json, header_lines, header_text,
    AllocStrategy, CommentStyle, Backend, CodegenError, CodegenOptions,
    CodegenOutput, Format, GeneratedFile, JavaFlavor, Language, GENERATED,
};
pub use emit::{
    record_optionals_before, record_repeated, repeat_bounds, default_is_encodable, field_key_json, has_int_keys, dcbor_key_order, rest_keys_are_text, map_with_non_text_rest, check_rest_keys, is_indirect, inline_decl_order, rename_types, c_reserved_renames, rust_reserved_renames, csharp_reserved_renames, reserved_renames, same_map_shape,
    capacity_value, constraint_to_c_check, constraint_to_cpp_check, constraint_to_rust_check,
    literal_to_c, literal_to_rust, primitive_to_c, primitive_to_rust,
    to_pascal_case, to_screaming_snake, to_snake_case, IndentWriter,
    emit_doc_lines, with_source_docs, DocStyle, enum_variant_text, rust_field_ident,
    bits_mask_bytes, bits_mask_u64, module_uses_bits, module_uses_any,
};
