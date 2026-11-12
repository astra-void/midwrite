//! Cancellation of executing runs by SIGINT, SIGTERM or SIGHUP.
//!
//! Without a handler, Ctrl-C ends Midwrite at once: no destructor runs, so
//! the disposable PostgreSQL cluster (a daemon started by `pg_ctl`) keeps
//! running with its shared memory, the replay directory stays, and a harness
//! in its own process group is not signaled. With the handler, the first
//! signal only sets a flag: the run loops treat it like their deadline, kill
//! their workers and return, and the normal cleanup runs. A second signal
//! exits immediately.

use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);
static SIGNALS: AtomicU32 = AtomicU32::new(0);
static INSTALL: Once = Once::new();

const MESSAGE: &[u8] =
    b"\nmidwrite: stopping and cleaning up (press Ctrl-C again to exit at once)\n";

extern "C" fn on_signal(signal: libc::c_int) {
    // Only async-signal-safe operations: atomics, write(2), _exit(2).
    if SIGNALS.fetch_add(1, Ordering::SeqCst) > 0 {
        // SAFETY: _exit is async-signal-safe.
        unsafe { libc::_exit(128 + signal) };
    }
    CANCELLED.store(true, Ordering::SeqCst);
    // SAFETY: write(2) to stderr with a static buffer is async-signal-safe.
    unsafe {
        libc::write(2, MESSAGE.as_ptr().cast(), MESSAGE.len());
    }
}

/// Install the handler (once per process). Called by every entry point
/// that starts PostgreSQL or executes a harness. Handlers are reset to the
/// default by `exec`, so harness processes and PostgreSQL are unaffected.
pub fn install() {
    INSTALL.call_once(|| {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: a zeroed sigaction with a valid handler and an empty
            // mask; `on_signal` only does async-signal-safe work.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
                libc::sigemptyset(&mut action.sa_mask);
                action.sa_flags = libc::SA_RESTART;
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    });
}

/// Whether a cancellation signal arrived.
pub fn cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}
