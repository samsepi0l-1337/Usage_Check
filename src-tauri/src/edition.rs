//! App edition helpers — re-exports core provider metadata.
//!
//! Free vs Pro is no longer a compile-time edition; it is a runtime license
//! gate (see `crate::license`).

pub use usage_core::edition::all_providers;

/// Tray / window product name baked into the binary (matches Tauri `productName`).
pub fn product_name() -> &'static str {
    "UsageCheck"
}
