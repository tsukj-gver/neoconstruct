//! Core module — error types, context container, and stream abstraction.
//!
//! This module groups together the fundamental building blocks:
//! - [`error`] — the unified error type for all construct operations
//! - [`context`] — the context container used during parse/build
//! - [`stream`] — the stream abstraction for binary I/O

pub mod context;
pub mod error;
pub mod stream;
