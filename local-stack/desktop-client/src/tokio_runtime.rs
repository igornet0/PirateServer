//! Shared multi-thread Tokio runtime for sync Tauri command entry points.

use parking_lot::Mutex;
use std::sync::Arc;
use tokio::runtime::Runtime;

static RUNTIME: Mutex<Option<Arc<Runtime>>> = Mutex::new(None);

/// Process-wide runtime. **Do not** call `block_on` from code already on this runtime's worker threads.
pub fn shared_runtime() -> Result<Arc<Runtime>, String> {
    let mut guard = RUNTIME.lock();
    if let Some(rt) = guard.as_ref() {
        return Ok(Arc::clone(rt));
    }
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("pirate-desktop")
            .build()
            .map_err(|e| e.to_string())?,
    );
    *guard = Some(Arc::clone(&rt));
    Ok(rt)
}

pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let rt = shared_runtime().expect("shared tokio runtime");
    rt.block_on(f)
}
