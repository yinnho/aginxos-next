//! Kernel-specific error types.

use thiserror::Error;
use carrier_types::error::CarrierError;

/// Kernel error type wrapping CarrierError with kernel-specific context.
#[derive(Error, Debug)]
pub enum KernelError {
    /// A wrapped CarrierError.
    #[error(transparent)]
    Carrier(#[from] CarrierError),

    /// The kernel failed to boot.
    #[error("Boot failed: {0}")]
    BootFailed(String),
}

/// Alias for kernel results.
pub type KernelResult<T> = Result<T, KernelError>;

/// Convert a `KernelError` into a `CarrierError`.
///
/// Used at the `KernelHandle` trait boundary so kernel-internal `KernelResult`s
/// can propagate as the runtime-wide `CarrierResult`. `Carrier(ce)` unwraps to
/// the inner error (preserving its specific variant); `BootFailed` maps to
/// `Internal` (it carries only a string).
impl From<KernelError> for CarrierError {
    fn from(e: KernelError) -> Self {
        match e {
            KernelError::Carrier(ce) => ce,
            KernelError::BootFailed(s) => CarrierError::Internal(s),
        }
    }
}
