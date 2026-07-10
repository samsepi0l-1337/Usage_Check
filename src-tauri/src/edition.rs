//! App edition helpers (Free vs Pro) — re-exports core edition metadata.

pub use usage_core::edition::all_providers;

/// Tray / window product name baked into the binary (matches Tauri `productName`).
pub fn product_name() -> &'static str {
    if cfg!(feature = "edition-pro") {
        "UsageCheck-Pro"
    } else {
        "UsageCheck-Free"
    }
}
