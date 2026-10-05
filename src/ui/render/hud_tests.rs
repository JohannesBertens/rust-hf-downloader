use super::hud::{chunk_map_spans, pad_name};
use super::*;
use crate::models::{ChunkProgress, DownloadProgress, QueueItemSummary, VerificationProgress};
use ratatui::Terminal;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

fn dl_progress(done: usize, total_chunks: usize) -> DownloadProgress {
    let mut chunk_completed = vec![false; total_chunks];
    for (i, c) in chunk_completed.iter_mut().enumerate().take(done) {
        *c = true;
        let _ = i;
    }
    DownloadProgress {
        model_id: "a/b".to_string(),
        filename: "model.gguf".to_string(),
        downloaded: 5,
        total: 10,
        speed_mbps: 0.0,
        chunks: Vec::new(),
        verifying: false,
        num_chunks: total_chunks,
        chunk_completed,
    }
}

#[test]
fn hud_height_is_zero_when_idle() {
    let none: Option<DownloadProgress> = None;
    let data = ActivityHudData {
        download_progress: &none,
        queue_size: 0,
        queue_bytes: 0,
        queue_items: &[],
        verification_progress: &[],
        verification_queue_size: 0,
        verification_queue_bytes: 0,
        verified_ok: 0,
        verified_fail: 0,
    };
    assert_eq!(activity_hud_height(&data), 0);
}

#[test]
fn hud_height_counts_rows_plus_footer() {
    let dl = dl_progress(2, 4);
    let v1 = VerificationProgress {
        filename: "a".to_string(),
        verified_bytes: Arc::new(AtomicU64::new(0)),
        total_bytes: 1,
        speed_mbps: 0.0,
    };
    let items: Vec<QueueItemSummary> = (0..5)
        .map(|i| QueueItemSummary {
            filename: format!("f{i}"),
            total_size: 1,
        })
        .collect();
    let none: Option<DownloadProgress> = None;
    // 1 DL + 1 VF + 3 Q + 1 "+more" = 6 rows + footer = 7
    let data = ActivityHudData {
        download_progress: &Some(dl),
        queue_size: 5,
        queue_bytes: 5,
        queue_items: &items,
        verification_progress: &[v1],
        verification_queue_size: 0,
        verification_queue_bytes: 0,
        verified_ok: 0,
        verified_fail: 0,
    };
    assert_eq!(activity_hud_height(&data), 9); // 6 rows + footer + borders
    let _ = none;
}

#[test]
fn hud_height_caps_at_budget() {
    let dl = dl_progress(0, 2);
    let vfs: Vec<VerificationProgress> = (0..4)
        .map(|i| VerificationProgress {
            filename: format!("v{i}"),
            verified_bytes: Arc::new(AtomicU64::new(0)),
            total_bytes: 1,
            speed_mbps: 0.0,
        })
        .collect();
    let items: Vec<QueueItemSummary> = (0..4)
        .map(|i| QueueItemSummary {
            filename: format!("f{i}"),
            total_size: 1,
        })
        .collect();
    // 1 + 4 + 3 + 1 = 9 rows uncapped -> capped to 8 + footer + borders
    let data = ActivityHudData {
        download_progress: &Some(dl),
        queue_size: 4,
        queue_bytes: 4,
        queue_items: &items,
        verification_progress: &vfs,
        verification_queue_size: 0,
        verification_queue_bytes: 0,
        verified_ok: 0,
        verified_fail: 0,
    };
    assert_eq!(activity_hud_height(&data), 11);
}

#[test]
fn chunk_map_marks_done_active_pending() {
    // 4 chunks: 0 done, 1 done, 2 active, 3 pending -> 4 cells 1:1
    let completed = vec![true, true, false, false];
    let map = chunk_map_spans(&completed, &[2], 4);
    assert_eq!(map.len(), 4);
    let chars: String = map.iter().map(|s| s.content.to_string()).collect();
    assert_eq!(chars, "\u{2588}\u{2588}\u{2593}\u{2591}");
}

#[test]
fn chunk_map_scales_down_without_gaps() {
    // 20 chunks (first 10 done), 10 cells -> each cell = 2 chunks
    let completed: Vec<bool> = (0..20).map(|i| i < 10).collect();
    let map = chunk_map_spans(&completed, &[], 10);
    let chars: String = map.iter().map(|s| s.content.to_string()).collect();
    assert_eq!(
        chars,
        "\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}"
    );
}

#[test]
fn pad_name_pads_to_fixed_column_width() {
    assert_eq!(pad_name("ab", 6), "ab    ");
    assert_eq!(pad_name("abcdefg", 6), "abc~fg"); // truncates to width, no pad needed
    assert_eq!(pad_name("模　型.gguf", 12).chars().count(), 12);
}

#[test]
fn snapshot_activity_hud_renders_matrix() {
    let dl = DownloadProgress {
        model_id: "unsloth/Mistral".to_string(),
        filename: "model-Q4_K_M.gguf".to_string(),
        downloaded: 4_700_000_000,
        total: 10_240_000_000,
        speed_mbps: 32.8,
        chunks: vec![ChunkProgress {
            chunk_id: 12,
            start: 0,
            end: 0,
            downloaded: 1,
            total: 2,
            speed_mbps: 4.0,
            is_active: true,
        }],
        verifying: false,
        num_chunks: 20,
        chunk_completed: (0..20).map(|i| i < 11).collect(),
    };
    let vb = |n: u64| VerificationProgress {
        filename: format!("shard-0000{n}.safetensors"),
        verified_bytes: Arc::new(AtomicU64::new(n * 1_000_000_000)),
        total_bytes: 5_000_000_000,
        speed_mbps: 2000.0,
    };
    let vfs = vec![vb(4), vb(3), vb(2), vb(1)];
    let items: Vec<QueueItemSummary> = vec![
        QueueItemSummary {
            filename: "shard-5.gguf".to_string(),
            total_size: 6_657_199_915,
        },
        QueueItemSummary {
            filename: "shard-6.gguf".to_string(),
            total_size: 6_657_199_915,
        },
        QueueItemSummary {
            filename: "shard-7.gguf".to_string(),
            total_size: 6_657_199_915,
        },
        QueueItemSummary {
            filename: "tokenizer.json".to_string(),
            total_size: 1_048_576,
        },
    ];
    let data = ActivityHudData {
        download_progress: &Some(dl),
        queue_size: 4,
        queue_bytes: 19_972_000_000,
        queue_items: &items,
        verification_progress: &vfs,
        verification_queue_size: 9,
        verification_queue_bytes: 12_884_901_888,
        verified_ok: 2,
        verified_fail: 0,
    };
    let height = activity_hud_height(&data);
    assert_eq!(height, 11); // 8 capped rows + footer + borders

    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 12)).unwrap();
    terminal
        .draw(|frame| {
            let area = Rect {
                x: 0,
                y: 0,
                width: 120,
                height,
            };
            render_activity_hud(frame, area, &data);
        })
        .unwrap();
    snap_ui("snapshot_activity_hud_renders_matrix", &terminal);
}
