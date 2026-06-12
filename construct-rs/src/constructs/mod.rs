//! Construct implementations — atomic, composite, adapter, and control-flow.
//!
//! This module contains all concrete construct implementations organized into
//! submodules by functionality. Each submodule provides one or more construct
//! types that implement the [`Construct`](crate::core::Construct) trait.
//!
//! # Module layout
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`bytes`] | [`Bytes`], [`GreedyBytes`] |
//! | [`format_field`] | [`FormatField`], [`Endianness`], [`FormatKind`] |
//! | [`bytes_integer`] | [`BytesInteger`], [`BitsInteger`] |
//! | [`varint`] | [`VarInt`], [`ZigZag`] |
//! | [`flag`] | [`Flag`] |
//! | [`const_`] | [`Const`] |
//! | [`meta`] | [`Pass`], [`Terminated`], [`Tell`], [`Seek`], [`SeekWhence`], [`Error`] |
//! | [`sequence`] | [`SeqEntry`], [`Sequence`] |
//! | [`struct_`] | [`Struct`], [`StructField`] |
//! | [`union`] | [`Union`], [`UnionTarget`] |
//! | [`select`] | [`Select`] |
//! | [`focused_seq`] | [`FocusedSeq`] |
//! | [`repetition`] | [`Array`], [`GreedyRange`], [`RepeatUntil`] |

pub mod bytes;
pub mod bytes_integer;
pub mod const_;
pub mod flag;
pub mod focused_seq;
pub mod format_field;
pub mod meta;
pub mod repetition;
pub mod select;
pub mod sequence;
pub mod struct_;
pub mod union;
pub mod varint;

// Re-export primary types for convenience.
pub use bytes::{Bytes, GreedyBytes};
pub use bytes_integer::{BitsInteger, BytesInteger};
pub use const_::Const;
pub use flag::Flag;
pub use focused_seq::FocusedSeq;
pub use format_field::{Endianness, FormatField, FormatKind};
pub use meta::{Error, Pass, Seek, SeekWhence, Tell, Terminated};
pub use repetition::{Array, GreedyRange, RepeatUntil};
pub use select::Select;
pub use sequence::{SeqEntry, Sequence};
pub use struct_::{Struct, StructField};
pub use union::{Union, UnionTarget};
pub use varint::{VarInt, ZigZag};
