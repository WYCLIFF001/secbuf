// src/error.rs
//! Error types for buffer operations with advanced conversion support

use std::fmt;

/// Errors that can occur during buffer operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferError {
    /// Position exceeds buffer length
    PositionOutOfBounds,
    /// Increment would exceed buffer size
    IncrementTooLarge,
    /// Buffer size exceeds maximum allowed
    SizeTooBig,
    /// Attempted to read/write beyond buffer bounds
    BufferOverflow,
    /// Invalid buffer state
    InvalidState(String),
    /// Circular buffer is full
    BufferFull,
    /// Circular buffer is empty
    BufferEmpty,
    /// Write would exceed available space
    InsufficientSpace,
    /// Invalid string encoding
    InvalidString,
    /// Invalid data format
    InvalidData(String),
    /// I/O error (for compatibility)
    Io(String),
}

impl fmt::Display for BufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PositionOutOfBounds => write!(f, "Position out of bounds"),
            Self::IncrementTooLarge => write!(f, "Increment too large"),
            Self::SizeTooBig => write!(f, "Buffer size too big"),
            Self::BufferOverflow => write!(f, "Buffer overflow"),
            Self::InvalidState(msg) => write!(f, "Invalid state: {}", msg),
            Self::BufferFull => write!(f, "Circular buffer is full"),
            Self::BufferEmpty => write!(f, "Circular buffer is empty"),
            Self::InsufficientSpace => write!(f, "Insufficient space in buffer"),
            Self::InvalidString => write!(f, "Invalid string encoding"),
            Self::InvalidData(msg) => write!(f, "Invalid data: {}", msg),
            Self::Io(msg) => write!(f, "I/O error: {}", msg),
        }
    }
}

impl std::error::Error for BufferError {}

// ============================================================================
// ADVANCED ERROR CONVERSION - Makes buffer library compatible with any error type
// ============================================================================

/// Convert BufferError to std::io::Error
impl From<BufferError> for std::io::Error {
    fn from(err: BufferError) -> Self {
        use std::io::ErrorKind;
        match err {
            BufferError::BufferOverflow | BufferError::InsufficientSpace => {
                std::io::Error::new(ErrorKind::WriteZero, err)
            }
            BufferError::BufferEmpty => {
                std::io::Error::new(ErrorKind::UnexpectedEof, err)
            }
            BufferError::Io(msg) => {
                std::io::Error::new(ErrorKind::Other, msg)
            }
            _ => std::io::Error::new(ErrorKind::Other, err),
        }
    }
}

/// Convert std::io::Error to BufferError
impl From<std::io::Error> for BufferError {
    fn from(err: std::io::Error) -> Self {
        BufferError::Io(err.to_string())
    }
}

/// Convert BufferError to anyhow::Error (for SSH handler compatibility)
#[cfg(feature = "anyhow")]
impl From<BufferError> for anyhow::Error {
    fn from(err: BufferError) -> Self {
        anyhow::anyhow!("{}", err)
    }
}

/// Allow using ? with anyhow::Error
#[cfg(feature = "anyhow")]
impl From<anyhow::Error> for BufferError {
    fn from(err: anyhow::Error) -> Self {
        BufferError::InvalidState(err.to_string())
    }
}

// ============================================================================
// RESULT TYPE ALIASES
// ============================================================================

/// Result type alias for buffer operations
///
/// Note: When using with other Result types (like anyhow::Result),
/// either qualify the type (`buffer::Result<T>`) or use the conversion traits.
pub type Result<T> = std::result::Result<T, BufferError>;

// ============================================================================
// EXTENSION TRAIT FOR EASY CONVERSION
// ============================================================================

/// Extension trait for converting Results between different error types
pub trait ResultExt<T> {
    /// Convert to anyhow::Result
    #[cfg(feature = "anyhow")]
    fn into_anyhow(self) -> anyhow::Result<T>;

    /// Convert to io::Result
    fn into_io(self) -> std::io::Result<T>;
}

impl<T> ResultExt<T> for Result<T> {
    #[cfg(feature = "anyhow")]
    fn into_anyhow(self) -> anyhow::Result<T> {
        self.map_err(|e| e.into())
    }

    fn into_io(self) -> std::io::Result<T> {
        self.map_err(|e| e.into())
    }
}

// ============================================================================
// HELPER MACROS FOR ERROR HANDLING
// ============================================================================

/// Convenience macro for converting buffer operations to any Result type.
///
/// Requires an explicit target error type as the second argument so the
/// conversion is unambiguous â€” necessary because error types like
/// `anyhow::Error` have multiple overlapping `From` impls.
///
/// # Example
/// ```ignore
/// use secbuf::prelude::*;
/// use secbuf::buffer_op;
///
/// fn handler_function() -> anyhow::Result<()> {
///     let mut buf = Buffer::new(1024);
///     buffer_op!(buf.put_u32(42), anyhow::Error)?;
///     Ok(())
/// }
/// ```
#[macro_export]
macro_rules! buffer_op {
    // Two-arg form: explicit target type (use this with anyhow, Box<dyn Error>, etc.)
    ($expr:expr, $target:ty) => {
        $expr.map_err(|e: $crate::BufferError| -> $target { e.into() })
    };
    // One-arg form: defaults to std::io::Error (unambiguous, no overlapping impls)
    ($expr:expr) => {
        $expr.map_err(|e: $crate::BufferError| -> std::io::Error { e.into() })
    };
}

/// Try a buffer operation with automatic error conversion
#[macro_export]
macro_rules! buffer_try {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(e) => return Err(e.into()),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_conversion_io() {
        let buf_err = BufferError::BufferOverflow;
        let io_err: std::io::Error = buf_err.into();
        assert_eq!(io_err.kind(), std::io::ErrorKind::WriteZero);
    }

    #[test]
    fn test_result_ext() {
        let result: Result<u32> = Ok(42);
        let io_result = result.into_io();
        assert_eq!(io_result.unwrap(), 42);
    }

    #[cfg(feature = "anyhow")]
    #[test]
    fn test_anyhow_conversion() {
        let buf_err = BufferError::InvalidString;
        let anyhow_err: anyhow::Error = buf_err.into();
        assert!(anyhow_err.to_string().contains("Invalid string"));
    }
}