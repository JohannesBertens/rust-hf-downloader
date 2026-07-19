//! Integration tests for `QueueState` accounting used by the download manager.

use rust_hf_downloader::models::QueueState;

#[test]
fn add_increments_size_and_bytes() {
    let mut q = QueueState::new(0, 0);
    q.add(2, 1024);
    assert_eq!(q.size, 2);
    assert_eq!(q.bytes, 1024);
}

#[test]
fn remove_saturates_instead_of_underflowing() {
    let mut q = QueueState::new(1, 100);
    q.remove(5, 1000); // would underflow without saturation
    assert_eq!(q.size, 0);
    assert_eq!(q.bytes, 0);
    assert!(q.is_empty());
}

#[test]
fn mixed_add_remove_tracks_correctly() {
    let mut q = QueueState::new(0, 0);
    q.add(3, 300);
    q.add(1, 50);
    q.remove(2, 200);
    assert_eq!(q.size, 2);
    assert_eq!(q.bytes, 150);
}
