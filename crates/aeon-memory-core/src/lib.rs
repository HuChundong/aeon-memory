pub mod aeon_memory_core;
pub mod config;
pub mod embedding;
pub mod error;
pub mod fts_query;
pub mod hooks;
pub mod llm;
pub mod offload;
pub mod persona;
pub mod pipeline;
pub mod profile;
pub mod prompt;
pub mod record;
pub mod scene;
pub mod search;
pub mod seed;
pub mod tools;
pub mod types;
pub mod utils;

pub use aeon_memory_core::{AeonMemoryCore, AeonMemoryCoreOptions, AeonMemoryStatus};
pub use error::{AeonMemoryCoreError, AeonMemoryResult};

/// Encodes digest bytes as the lowercase hexadecimal identifiers persisted by Aeon.
///
/// This deliberately does not depend on a digest crate's formatting traits: digest 0.11
/// returns an array type that no longer implements `LowerHex`.
pub(crate) fn lowercase_hex(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;

    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}
