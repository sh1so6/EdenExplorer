pub mod drives;
pub mod fs;
pub mod indexer;
pub mod launch;
pub mod portable;
#[cfg(feature = "ui-probe")]
pub mod probe;
pub mod session;
pub mod utils;

/// `true` when running as the dev-only UI probe (`ui-probe` feature and
/// `EDEN_PROBE_SCRIPT` set). Always `false` in normal builds.
#[inline]
pub fn probe_active() -> bool {
    #[cfg(feature = "ui-probe")]
    {
        probe::is_active()
    }
    #[cfg(not(feature = "ui-probe"))]
    {
        false
    }
}
