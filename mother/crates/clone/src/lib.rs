//! Carrier Clone — file-level (dup) definition sync + manifest building.
//!
//! Definition layer is shipped as individual files (path -> bytes) + a manifest
//! (path -> SHA-256 + state hash), never as a compressed blob.

pub mod defaults;
pub mod hub;
mod loader;
pub mod manifest;
pub mod manifest_builder;
pub mod market;
pub mod system_creator;
pub mod tar_source;

pub use defaults::{CLONE_FORMAT_SPEC, CLONE_FORMAT_SPEC_VERSION, DEFAULT_SELF_GROWTH_FLOW};

pub use loader::{parse_template_manifest_lenient, TemplateManifest};
pub use manifest::{
    build_manifest, is_bak, is_test_dir, sha256_hex, validate_install_format,
    write_files_to_workspace, Manifest,
};
pub use manifest_builder::build_manifest_from_workspace;
