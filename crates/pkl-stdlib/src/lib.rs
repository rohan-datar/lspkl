//! Pkl standard library downloaded at build time and embedded for offline use.
//!
//! The version and archive checksum are pinned in `build.rs`. Both the VFS
//! and the type system use these assets; no runtime filesystem access is needed.

/// The source of `pkl:base`, from the same artifact as every other module.
pub const BASE: &str = include_str!(concat!(env!("OUT_DIR"), "/stdlib/base.pkl"));

include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
