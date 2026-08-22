//! Platform-specific safe filesystem primitives.

#[cfg(not(unix))]
mod portable;
#[cfg(unix)]
mod unix;

#[cfg(not(unix))]
pub(crate) use portable::*;
#[cfg(unix)]
pub(crate) use unix::*;
