//! Shared type-classification helpers used by the parser and database layers.
//!
//! These functions are hot-path during cross-file symbol resolution (millions of
//! calls on large repos). They use `LazyLock<HashSet>` to pay the set-construction
//! cost exactly once per process instead of on every invocation.

use std::collections::HashSet;
use std::sync::LazyLock;

/// Primitive and built-in types across all supported languages: the one list
/// the database, the symbol table and cross-file analysis share. Stored as
/// lowercase to allow case-insensitive matching.
pub static PRIMITIVE_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Python
        "int",
        "float",
        "str",
        "bool",
        "none",
        "any",
        "optional",
        "union",
        "list",
        "dict",
        "set",
        "tuple",
        // JavaScript / TypeScript
        "number",
        "string",
        "boolean",
        "void",
        "null",
        "undefined",
        "unknown",
        "never",
        // Java / C / C++ / Go
        "long",
        "short",
        "byte",
        "char",
        "double",
        // Rust
        "i8",
        "i16",
        "i32",
        "i64",
        "i128",
        "u8",
        "u16",
        "u32",
        "u64",
        "u128",
        "f32",
        "f64",
        "usize",
        "isize",
        // Generic wrappers treated as primitives for resolution purposes
        "array",
        "map",
        "promise",
        "option",
        "result",
        "box",
        "rc",
        "arc",
        "vec",
    ]
    .into_iter()
    .collect()
});

/// Returns `true` when `type_name` is a primitive or well-known built-in type
/// that should be excluded from cross-file symbol resolution.
///
/// The check is case-insensitive so `"Bool"`, `"BOOL"`, and `"bool"` all match.
/// Pre-lower-casing the input once at the call site avoids the allocation on
/// every call when the same name is checked repeatedly.
#[inline]
pub fn is_primitive_type(type_name: &str) -> bool {
    PRIMITIVE_TYPES.contains(type_name.to_lowercase().as_str())
}
