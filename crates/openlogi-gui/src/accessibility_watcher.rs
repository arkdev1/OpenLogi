//! macOS Accessibility-permission watcher.
//!
//! Polls [`openlogi_hook::Hook::has_accessibility`] on a dedicated OS thread
//! and forwards the trust state over an mpsc whenever it changes (plus an
//! initial value). The GUI uses it to (a) show/hide the permission gate and
//! (b) install the OS mouse hook the moment the user grants access, without
//! requiring a restart.
//!
//! Non-macOS platforms have no Accessibility concept — `has_accessibility`
//! returns `true` there — so the watcher emits a single `true` and exits.

use std::thread;
use std::time::Duration;

use openlogi_hook::Hook;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Start the watcher and return a receiver of trust-state transitions. The
/// initial value is pushed immediately so the consumer doesn't need a
/// separate query.
///
/// Dropping the receiver shuts the watcher down: the next `send` fails and
/// the loop exits.
pub fn spawn(period: Duration) -> mpsc::UnboundedReceiver<bool> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Non-macOS: permission is always "granted"; emit once and stop. The
    // initial send can't fail (receiver is still held by the caller).
    if !cfg!(target_os = "macos") {
        let _ = tx.send(true);
        let _ = period;
        return rx;
    }

    let spawn_result = thread::Builder::new()
        .name("openlogi-accessibility-watcher".into())
        .spawn(move || {
            let mut last: Option<bool> = None;
            loop {
                let granted = Hook::has_accessibility();
                if last != Some(granted) {
                    debug!(granted, "accessibility trust changed");
                    if tx.send(granted).is_err() {
                        debug!("accessibility watcher receiver dropped — exiting");
                        return;
                    }
                    last = Some(granted);
                    // Once granted it can't be revoked without a relaunch
                    // (macOS restarts the process on revocation), so stop
                    // polling to keep the thread idle-free.
                    if granted {
                        return;
                    }
                }
                thread::sleep(period);
            }
        });
    if let Err(e) = spawn_result {
        warn!(error = %e, "could not spawn accessibility watcher — gate won't auto-dismiss");
    }
    rx
}
