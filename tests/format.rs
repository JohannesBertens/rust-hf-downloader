//! Integration tests for display formatting helpers.

use rust_hf_downloader::utils::{format_number, format_size};

#[test]
fn format_size_humanizes_units() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(2 * 1_048_576), "2.00 MB");
    assert_eq!(format_size(3 * 1_073_741_824), "3.00 GB");
}

#[test]
fn format_number_uses_compact_suffixes() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(999), "999");
    assert_eq!(format_number(1_500), "1.5K");
    assert_eq!(format_number(2_300_000), "2.3M");
}
