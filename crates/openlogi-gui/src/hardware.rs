//! Hardware-side actions invoked from both the GPUI thread (slider release)
//! and the OS-event hook thread (bound button press).
//!
//! Today the only operation is "push a new DPI value via HID++". Each call
//! spawns a one-shot tokio runtime on a dedicated OS thread — that's cheap
//! at the cadence these fire at (≤ once per slider release / button press)
//! and avoids holding a long-lived async runtime alongside GPUI's executor.

use tracing::{debug, warn};

use crate::components::dpi_panel::DpiTarget;

/// Spawn an OS thread that writes `dpi` to the device at `target` via
/// `openlogi_hid::set_dpi`. Returns immediately; failures are logged.
///
/// `target == None` is a no-op (dev environment without a real device).
pub fn write_dpi_in_background(target: Option<DpiTarget>, dpi: u32) {
    let Some(target) = target else {
        debug!(dpi, "no target device — DPI write skipped");
        return;
    };
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                warn!(error = %e, "tokio runtime init failed; DPI write skipped");
                return;
            }
        };
        // DPI values are clamped to <= 6400 by every caller, so the cast is
        // lossless. The saturating fallback exists only for type-system
        // exhaustiveness.
        let dpi_u16 = u16::try_from(dpi).unwrap_or(u16::MAX);
        let result = rt.block_on(openlogi_hid::set_dpi(
            Some(&target.receiver_uid),
            target.slot,
            dpi_u16,
        ));
        match result {
            Ok(()) => debug!(slot = target.slot, dpi = dpi_u16, "DPI written to device"),
            Err(e) => warn!(error = ?e, "DPI write failed"),
        }
    });
}
