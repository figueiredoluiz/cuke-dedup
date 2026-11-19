//! SARIF rendering keeps temporary memory bounded for oversized caller-supplied paths.

use cuke_dedup::model::{AnalysisResult, Finding, SourceLocation};
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

const PATH_BYTES: usize = 8 * 1024 * 1024;

/// 8 MiB paths, UTF-8 and (on Unix) not, yield bounded URIs without copying the whole path.
/// One test: the allocator counters are process-wide.
#[test]
fn oversized_paths_render_sarif_with_bounded_temporary_memory() {
    let root = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
    assert_bounded(&root, root.join("a".repeat(PATH_BYTES)));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        // A lossy conversion of these bytes would allocate three times the path length.
        let name = vec![0xFF_u8; PATH_BYTES];
        assert_bounded(&root, root.join(std::ffi::OsStr::from_bytes(&name)));
    }
}

/// Renders one finding at `path` and asserts the sampled URI and peak temporary memory.
fn assert_bounded(root: &std::path::Path, path: PathBuf) {
    let placeholder = serde_json::json!({
        "path": "a", "line": 1, "column": 1, "end_line": 1, "end_column": 2,
    });
    let mut finding: Finding = serde_json::from_value(serde_json::json!({
        "rule": "duplicate-matcher", "severity": "error", "message": "m",
        "primary": placeholder, "related": [],
        "evidence": { "matcherDifference": "", "handlerEvidence": "" },
        "suggested_action": "", "suppression": null,
    }))
    .unwrap();
    // Set directly: JSON cannot carry a non-UTF-8 path.
    finding.primary = SourceLocation::new(path, 1, 1, 1, 2);
    let result = AnalysisResult::new(Vec::new(), Vec::new(), vec![finding]);
    let context = ReportContext::new(&result, root, 5.0);

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
