//! SARIF rendering keeps temporary memory bounded for oversized caller-supplied paths.

use cuke_dedup::model::{AnalysisResult, Finding};
use cuke_dedup::reporters::{render_sarif, ReportContext};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// System allocator that records live and peak bytes; this binary holds a single test.
struct PeakAllocator;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` with the caller's layout; counters only observe sizes.
unsafe impl GlobalAlloc for PeakAllocator {
    /// Forwards to `System` and raises the peak by the new live total.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    /// Forwards to `System` and lowers the live total.
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: PeakAllocator = PeakAllocator;

/// An 8 MiB path yields a bounded URI without copying the whole path during rendering.
#[test]
fn oversized_paths_render_sarif_with_bounded_temporary_memory() {
    const PATH_BYTES: usize = 8 * 1024 * 1024;
    let root = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
    let path = root.join("a".repeat(PATH_BYTES));
    let location = serde_json::json!({
        "path": path, "line": 1, "column": 1, "end_line": 1, "end_column": 2,
    });
    let finding: Finding = serde_json::from_value(serde_json::json!({
        "rule": "duplicate-matcher", "severity": "error", "message": "m",
        "primary": location, "related": [],
        "evidence": { "matcherDifference": "", "handlerEvidence": "" },
        "suggested_action": "", "suppression": null,
    }))
    .unwrap();
    let result = AnalysisResult::new(Vec::new(), Vec::new(), vec![finding]);
    let context = ReportContext::new(&result, &root, 5.0);

    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let sarif = render_sarif(&context).unwrap();
    let temporary = PEAK.load(Ordering::Relaxed) - before;

    assert!(sarif.contains("bytes%20omitted"), "path was not sampled");
    assert!(
        temporary < PATH_BYTES / 2,
        "rendering used {temporary} temporary bytes for a {PATH_BYTES}-byte path"
    );
}
