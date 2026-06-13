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
//! | [`adapters`] | [`Adapter`], [`SymmetricAdapter`], [`ExprAdapter`], [`Validator`], [`ExprValidator`] |
//! | [`bytes`] | [`Bytes`], [`GreedyBytes`] |
//! | [`format_field`] | [`FormatField`], [`Endianness`], [`FormatKind`] |
//! | [`bytes_integer`] | [`BytesInteger`], [`BitsInteger`] |
//! | [`varint`] | [`VarInt`], [`ZigZag`] |
//! | [`flag`] | [`Flag`] |
//! | [`const_`] | [`Const`] |
//! | [`enum_`] | [`Enum`], [`FlagsEnum`], [`Mapping`] |
//! | [`meta`] | [`Pass`], [`Terminated`], [`Tell`], [`Seek`], [`SeekWhence`], [`Error`] |
//! | [`sequence`] | [`SeqEntry`], [`Sequence`] |
//! | [`struct_`] | [`Struct`], [`StructField`] |
//! | [`union`] | [`Union`], [`UnionTarget`] |
//! | [`select`] | [`Select`] |
//! | [`focused_seq`] | [`FocusedSeq`] |
//! | [`computed`] | [`Computed`], [`Rebuild`], [`Default`], [`Index`], [`Padded`], [`Aligned`], [`FixedSized`], [`NamedTuple`], [`TimestampAdapter`] |
//! | [`repetition`] | [`Array`], [`GreedyRange`], [`RepeatUntil`] |
//! | [`lazy`] | [`Lazy`], [`LazyStruct`], [`LazyArray`], [`Rebuffered`] |
//!
//! [`Adapter`]: adapters::Adapter
//! [`SymmetricAdapter`]: adapters::SymmetricAdapter
//! [`ExprAdapter`]: adapters::ExprAdapter
//! [`Validator`]: adapters::Validator
//! [`ExprValidator`]: adapters::ExprValidator
//! [`Lazy`]: lazy::Lazy
//! [`LazyStruct`]: lazy::LazyStruct
//! [`LazyArray`]: lazy::LazyArray
//! [`Rebuffered`]: lazy::Rebuffered

pub mod adapters;
pub mod bytes;
pub mod bytes_integer;
pub mod computed;
pub mod const_;
pub mod control_flow;
pub mod enum_;
pub mod flag;
pub mod focused_seq;
pub mod format_field;
pub mod hex;
pub mod lazy;
pub mod meta;
pub mod repetition;
pub mod select;
pub mod sequence;
pub mod stream_ops;
pub mod struct_;
pub mod union;
pub mod varint;

// Re-export primary types for convenience.
pub use adapters::{Adapter, ExprAdapter, ExprValidator, SymmetricAdapter, Validator};
pub use bytes::{Bytes, GreedyBytes};
pub use bytes_integer::{BitsInteger, BytesInteger};
pub use computed::{
    Aligned, ComputeFunc, Computed, Default, FixedSized, Index, NamedTuple, Padded, Rebuild,
    TimestampAdapter, TimestampEpoch, TimestampUnit,
};
pub use const_::Const;
pub use enum_::{Enum, FlagsEnum, Mapping};
pub use flag::Flag;
pub use focused_seq::FocusedSeq;
pub use format_field::{Endianness, FormatField, FormatKind};
pub use lazy::{Lazy, LazyArray, LazyStruct, Rebuffered};
pub use meta::{Error, Pass, Seek, SeekWhence, Tell, Terminated};
pub use repetition::{Array, GreedyRange, RepeatUntil};
pub use select::Select;
pub use sequence::{SeqEntry, Sequence};
pub use stream_ops::{
    BitsSwapped, Bitwise, ByteSwapped, Bytewise, Checksum, LazyBound, Peek, Pointer, Prefixed,
    RawCopy, Restreamed, Transformed,
};
#[cfg(feature = "compression")]
pub use stream_ops::{Compressed, CompressionAlgorithm};
pub use struct_::{Struct, StructField};
pub use union::{Union, UnionTarget};
pub use varint::{VarInt, ZigZag};
