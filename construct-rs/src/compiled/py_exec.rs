//! Python-direct execution trait + dispatch (Phase 16.3).
//!
//! This module defines [`PyCompiledExec`] — a parallel trait to
//! [`CompiledExec`](super::CompiledExec) that writes parse results directly
//! into a [`PySink`] as `PyObject`, bypassing the `Value` intermediate tree
//! on the Python hot path.
//!
//! # Zero round-trip design
//!
//! - **Leaf nodes** call `self.inner.parse()` (returns a transient scalar
//!   `Value`), then immediately convert it to `PyObject` via
//!   [`value_to_py_scalar`] (single `match`, one C API call), and deposit
//!   **both** into the sink via [`PySink::deposit_leaf`]. The `Value` is
//!   used for context expression evaluation; the `PyObject` is the output.
//! - **Composite nodes** (Struct / Sequence / Array / ...) create sub-sinks
//!   via [`PySink::sub_py_field`] / [`PySink::sub_py_item`], recurse, then
//!   integrate via [`PySink::finish_field_py`] / [`PySink::finish_item_py`].
//!   The child's `Value` is returned for context insertion **without** a
//!   `py_to_value` round-trip — it comes from the child's parallel `Value`
//!   tree.
//! - **Wrapper nodes** (Adapter / Validator / Const / Enum / ...) create a
//!   **temporary** [`PyDictSink2`] for the inner node, recurse, then extract
//!   the inner `(Value, PyObject)` pair via [`PySink::into_pair`], apply
//!   their transform/check/mapping on the `Value`, re-convert to `PyObject`,
//!   and deposit into the parent sink.
//!
//! # `python` feature gate
//!
//! This entire module is compiled only under `--features python`. Without
//! it, the pure-Rust [`CompiledExec`](super::CompiledExec) path is used
//! unchanged, and the 1515+ unit tests are unaffected.
//
// NOTE: The `#[cfg(feature = "python")]` gate lives on the `pub mod` line in
// `mod.rs`; an inner `#![cfg(...)]` would duplicate it and trigger
// `clippy::duplicated_attributes`.

use pyo3::prelude::*;

use crate::compiled::py_input::PyInput;
use crate::compiled::py_sink::{
    py_err_to_construct, value_to_py_composite_fallback, value_to_py_scalar, PySink,
};
use crate::compiled::{CompiledExec, CompiledNode};
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::{ByteStream, CombinedStream, Stream};
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// PyCompiledExec trait
// ===========================================================================

/// Python-direct execution trait (cfg-gated under the `python` feature).
///
/// Each [`CompiledNode`] variant implements this trait for the Python parse /
/// build hot path. The Value-path [`CompiledExec`](super::CompiledExec) trait
/// is **unchanged** — both coexist (see design §5.1 "双路径共存架构").
///
/// # Parse direction (`exec_parse_py`)
///
/// Reads from `stream`, writes parse results directly into `sink` as
/// `PyObject`. The `Value` is used only as a transient scalar carrier (for
/// context expression evaluation) — no `Value` output tree accumulates.
///
/// # Build direction (`exec_build_py`)
///
/// Reads field values lazily from `input` (a [`PyInput`]), writes binary
/// data to `stream`. Per-field schema-guided extraction avoids the generic
/// 7× type-check chain.
///
/// **Phase 16.3 status**: `exec_build_py` returns
/// [`ConstructError::Generic`] ("not yet implemented (Phase 16.4)"). Real
/// implementations are filled in by Phase 16.4. The stub is required so
/// that the dispatch macro's exhaustiveness check holds.
pub trait PyCompiledExec {
    /// Parses from `stream`, writing results directly into `sink` as
    /// `PyObject`. Value is used only as a transient scalar carrier (for
    /// context) and is immediately converted to `PyObject` — no Value
    /// output tree accumulates.
    ///
    /// # Errors
    ///
    /// Propagates any [`ConstructError`] from stream reading or sink
    /// operations.
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()>;

    /// Builds binary data, reading field values lazily from `input`.
    ///
    /// **Phase 16.3**: stub. Returns
    /// `Err(ConstructError::Generic { message: "exec_build_py not yet implemented (Phase 16.4)" })`.
    ///
    /// # Errors
    ///
    /// Always returns an "not yet implemented" error in Phase 16.3.
    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()>;
}

// ===========================================================================
// Helper: parse inner into a temporary PyDictSink2, return (Value, PyObject)
// ===========================================================================

/// Executes `exec_parse_py` on `inner` into a fresh temporary
/// [`PyDictSink2`] (Empty mode), then consumes the sink to obtain the
/// `(Value, PyObject)` pair.
///
/// Used by wrapper nodes (Adapter / Validator / Const / Enum / Mapping /
/// RawCopy / Checksum / Computed / ...) that need to inspect the inner
/// parse result before applying their own transform and re-depositing into
/// the parent sink.
///
/// The temporary sink shares the parent's [`ContainerClasses`](crate::compiled::py_sink::ContainerClasses)
/// so `Container` / `ListContainer` wrapping is consistent.
fn exec_parse_inner_py(
    inner: &CompiledNode,
    py: Python<'_>,
    stream: &mut CombinedStream,
    ctx: &mut Context,
    parent_sink: &mut dyn PySink,
) -> Result<(Value, PyObject)> {
    let mut sub = parent_sink.sub_py_item(py)?;
    inner.exec_parse_py(py, stream, ctx, sub.as_mut())?;
    sub.into_pair(py)
}

/// Deposits a (Value, PyObject) pair as a leaf into `sink`.
///
/// `Value` may be either a scalar (the common case for leaf-produced
/// values) or a composite (Container / List, produced by Adapter decode
/// returning a Container, or Enum producing a FlagsContainer). For
/// composites, the `PyObject` is built via [`value_to_py_composite_fallback`]
/// before deposit.
fn deposit_pair_as_leaf(py: Python<'_>, sink: &mut dyn PySink, value: Value) -> Result<()> {
    let obj = value_to_py_scalar(py, &value)
        .or_else(|_| value_to_py_composite_fallback(py, &value))
        .map_err(py_err_to_construct)?;
    sink.deposit_leaf(py, value, obj)
}

// ===========================================================================
// Leaf PyCompiledExec implementations
// ===========================================================================
//
// Leaf compiled nodes embed their declaration-tree construct by value (the
// embed-and-delegate pattern). At runtime, `exec_parse_py` calls
// `self.inner.parse()` (shared decode logic, returns a transient scalar
// `Value`), then immediately converts it to `PyObject` via
// `value_to_py_scalar` (single match, one C API call), and deposits BOTH
// into the sink via `deposit_leaf`. The `Value` is for context expression
// evaluation; the `PyObject` is the output.

/// Macro: generates `PyCompiledExec` for leaf compiled nodes that embed a
/// concrete `Construct` in their `inner` field.
///
/// `exec_parse_py` flow:
/// 1. `self.inner.parse(stream, ctx)` — shared decode, returns scalar Value.
/// 2. `value_to_py_scalar(py, &value)` — one C API call (type known).
/// 3. `sink.deposit_leaf(py, value, obj)` — stores both for ctx and output.
///
/// `exec_build_py` flow:
/// 1. `input.extract_scalar_py(py)` — schema-guided scalar extraction (no
///    7× type-check chain).
/// 2. `self.inner.build(&value, stream, ctx)` — shared encode logic.
macro_rules! impl_leaf_py_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl PyCompiledExec for $ty {
                fn exec_parse_py(
                    &self,
                    py: Python<'_>,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn PySink,
                ) -> Result<()> {
                    // Shared decode: returns scalar Value (cheap, transient).
                    let value = self.inner.parse(stream, ctx)?;
                    // One C API call (type known from Value variant).
                    let obj = value_to_py_scalar(py, &value)
                        .or_else(|_| value_to_py_composite_fallback(py, &value))
                        .map_err(py_err_to_construct)?;
                    // Deposit both: Value for ctx, PyObject for output.
                    sink.deposit_leaf(py, value, obj)
                }

                fn exec_build_py(
                    &self,
                    py: Python<'_>,
                    input: &dyn PyInput,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    let value = input.extract_scalar_py(py)?;
                    self.inner.build(&value, stream, ctx)
                }
            }
        )*
    };
}

impl_leaf_py_exec! {
    // atomic leaves
    crate::compiled::CompiledFormatField,
    crate::compiled::CompiledBytes,
    crate::compiled::CompiledGreedyBytes,
    crate::compiled::CompiledBytesExpr,
    crate::compiled::CompiledBytesInteger,
    crate::compiled::CompiledBitsInteger,
    crate::compiled::CompiledVarInt,
    crate::compiled::CompiledZigZag,
    crate::compiled::CompiledFlag,
    crate::compiled::CompiledCString,
    crate::compiled::CompiledPaddedString,
    // meta leaves
    crate::compiled::CompiledPass,
    crate::compiled::CompiledTerminated,
    crate::compiled::CompiledTell,
    crate::compiled::CompiledSeek,
    crate::compiled::CompiledSeekExpr,
    crate::compiled::CompiledError,
}

// ===========================================================================
// Pure-forward wrapper PyCompiledExec implementations
// ===========================================================================
//
// Pure-forward wrappers (Subconstruct / Renamed / Hex / HexDump / Lazy /
// LazyStruct) delegate every operation directly to their inner compiled
// node with no transform. The Py path simply passes the parent sink
// through to `inner.exec_parse_py`.

/// Macro: generates `PyCompiledExec` for pure-forward wrappers that delegate
/// directly to `self.inner.exec_parse_py` with the same sink (no transform).
macro_rules! impl_forward_wrapper_py_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl PyCompiledExec for $ty {
                fn exec_parse_py(
                    &self,
                    py: Python<'_>,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn PySink,
                ) -> Result<()> {
                    self.inner.exec_parse_py(py, stream, ctx, sink)
                }

                fn exec_build_py(
                    &self,
                    py: Python<'_>,
                    input: &dyn PyInput,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    self.inner.exec_build_py(py, input, stream, ctx)
                }
            }
        )*
    };
}

impl_forward_wrapper_py_exec! {
    crate::compiled::CompiledSubconstruct,
    crate::compiled::CompiledRenamed,
    crate::compiled::CompiledHex,
    crate::compiled::CompiledHexDump,
    crate::compiled::CompiledLazy,
    crate::compiled::CompiledLazyStruct,
}

// ===========================================================================
// Adapter / ExprAdapter PyCompiledExec (decode → re-deposit)
// ===========================================================================
//
// C-IMP-2: Adapter wraps an inner node and applies a decode closure
// (Value → Value) on the inner parse result. The Py path:
// 1. Parse inner into a temporary sub-sink (via exec_parse_inner_py).
// 2. Apply decode on the resulting Value.
// 3. Re-convert the decoded Value to PyObject via value_to_py_scalar
//    (or value_to_py_composite_fallback for Container/List results).
// 4. Deposit the new (Value, PyObject) pair as a leaf into the parent sink.

/// Macro: generates `PyCompiledExec` for adapter-style wrappers that hold
/// separate `decode` and `encode` closures (Adapter, ExprAdapter).
macro_rules! impl_adapter_py_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl PyCompiledExec for $ty {
                fn exec_parse_py(
                    &self,
                    py: Python<'_>,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn PySink,
                ) -> Result<()> {
                    let (raw, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
                    let decoded = (self.decode)(&raw, ctx)?;
                    deposit_pair_as_leaf(py, sink, decoded)
                }

                fn exec_build_py(
                    &self,
                    py: Python<'_>,
                    input: &dyn PyInput,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    let value = input.extract_ctx_value_py(py)?;
                    let encoded = (self.encode)(&value, ctx)?;
                    let scalar_input = crate::compiled::py_input::PyScalarInput::new(encoded);
                    self.inner.exec_build_py(py, &scalar_input, stream, ctx)
                }
            }
        )*
    };
}

impl_adapter_py_exec!(
    crate::compiled::CompiledAdapter,
    crate::compiled::CompiledExprAdapter,
);

// -- SymmetricAdapter (single func for both directions) -------------------

impl PyCompiledExec for crate::compiled::CompiledSymmetricAdapter {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (raw, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        let decoded = (self.func)(&raw, ctx)?;
        deposit_pair_as_leaf(py, sink, decoded)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let value = input.extract_ctx_value_py(py)?;
        let encoded = (self.func)(&value, ctx)?;
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(encoded);
        self.inner.exec_build_py(py, &scalar_input, stream, ctx)
    }
}

// ===========================================================================
// Validator / ExprValidator PyCompiledExec (check → pass-through)
// ===========================================================================

/// Macro: generates `PyCompiledExec` for validator-style wrappers. The inner
/// value passes through unchanged when validation succeeds.
macro_rules! impl_validator_py_exec {
    ($($ty:ty),* $(,)?) => {
        $(
            impl PyCompiledExec for $ty {
                fn exec_parse_py(
                    &self,
                    py: Python<'_>,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                    sink: &mut dyn PySink,
                ) -> Result<()> {
                    let (raw, obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
                    // Validate; on success the raw value passes through.
                    (self.check)(&raw, ctx)?;
                    // Re-use the existing PyObject when possible. For scalar
                    // Values the inner's obj is already correct; for
                    // composites we still need to ensure correct wrapping.
                    sink.deposit_leaf(py, raw, obj)
                }

                fn exec_build_py(
                    &self,
                    py: Python<'_>,
                    input: &dyn PyInput,
                    stream: &mut CombinedStream,
                    ctx: &mut Context,
                ) -> Result<()> {
                    let value = input.extract_ctx_value_py(py)?;
                    (self.check)(&value, ctx)?;
                    self.inner.exec_build_py(py, input, stream, ctx)
                }
            }
        )*
    };
}

impl_validator_py_exec!(
    crate::compiled::CompiledValidator,
    crate::compiled::CompiledExprValidator,
);

// ===========================================================================
// Const (parse inner → check → pass-through)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledConst {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (raw, obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        if raw != self.value {
            return Err(ConstructError::Const {
                path: String::new(),
                expected: format!("{:?}", self.value),
                actual: format!("{:?}", raw),
            });
        }
        sink.deposit_leaf(py, raw, obj)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let value = input.extract_ctx_value_py(py)?;
        if !value.is_none() && value != self.value {
            return Err(ConstructError::Const {
                path: String::new(),
                expected: format!("None or {:?}", self.value),
                actual: format!("{:?}", value),
            });
        }
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(self.value.clone());
        self.inner.exec_build_py(py, &scalar_input, stream, ctx)
    }
}

// ===========================================================================
// Enum (integer ↔ string mapping)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledEnum {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (obj, _inner_pyobj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        let int_val = obj.to_u64().map_err(|e| e.with_path_prefix("Enum"))?;
        let value = match self.decmap.get(&int_val) {
            Some(label) => Value::String(label.clone()),
            None => obj,
        };
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.extract_ctx_value_py(py)?;
        let build_val = match &raw {
            Value::String(label) => match self.mapping.get(label) {
                Some(&v) => Value::UInt(v),
                None => {
                    return Err(ConstructError::Mapping {
                        path: String::new(),
                        key: format!("{:?}", label),
                    }
                    .with_path_prefix("Enum"))
                }
            },
            Value::Int(i) => Value::Int(*i),
            other => {
                let v = other.to_u64().map_err(|e| e.with_path_prefix("Enum"))?;
                Value::UInt(v)
            }
        };
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(build_val);
        self.inner
            .exec_build_py(py, &scalar_input, stream, ctx)
            .map_err(|e| e.with_path_prefix("Enum"))
    }
}

// ===========================================================================
// FlagsEnum (bit-flag expansion → Container)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledFlagsEnum {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (raw, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        let int_val = raw.to_u64().map_err(|e| e.with_path_prefix("FlagsEnum"))?;
        let mut container = indexmap::IndexMap::new();
        for (name, &flag_value) in &self.flags {
            let set = (int_val & flag_value) == flag_value;
            container.insert(name.clone(), Value::Bool(set));
        }
        deposit_pair_as_leaf(py, sink, Value::Container(container))
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.extract_ctx_value_py(py)?;
        let container = raw
            .as_container()
            .map_err(|e| e.with_path_prefix("FlagsEnum"))?;
        let mut flags_val: u64 = 0;
        for (name, value) in container {
            if name.starts_with('_') {
                continue;
            }
            match self.flags.get(name) {
                Some(&flag_bits) => {
                    let is_set = value
                        .as_bool()
                        .map_err(|e| e.with_path_prefix("FlagsEnum"))?;
                    if is_set {
                        flags_val |= flag_bits;
                    }
                }
                None => {
                    return Err(ConstructError::Mapping {
                        path: String::new(),
                        key: format!("{:?}", name),
                    }
                    .with_path_prefix("FlagsEnum"));
                }
            }
        }
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(Value::UInt(flags_val));
        self.inner
            .exec_build_py(py, &scalar_input, stream, ctx)
            .map_err(|e| e.with_path_prefix("FlagsEnum"))
    }
}

// ===========================================================================
// Mapping (arbitrary Value ↔ Value mapping)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledMapping {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (obj, _inner_pyobj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        for (k, v) in &self.decmapping {
            if k == &obj {
                return deposit_pair_as_leaf(py, sink, v.clone());
            }
        }
        Err(ConstructError::Mapping {
            path: String::new(),
            key: format!("{:?}", obj),
        }
        .with_path_prefix("Mapping"))
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.extract_ctx_value_py(py)?;
        for (k, v) in &self.mapping {
            if k == &raw {
                let scalar_input = crate::compiled::py_input::PyScalarInput::new(v.clone());
                return self
                    .inner
                    .exec_build_py(py, &scalar_input, stream, ctx)
                    .map_err(|e| e.with_path_prefix("Mapping"));
            }
        }
        Err(ConstructError::Mapping {
            path: String::new(),
            key: format!("{:?}", raw),
        }
        .with_path_prefix("Mapping"))
    }
}

// ===========================================================================
// Computed / Index / Check / StopIf (context-only, no stream read)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledComputed {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let value = (self.func)(ctx)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        _py: Python<'_>,
        _input: &dyn PyInput,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let _ = (self.func)(ctx)?;
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledIndex {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        // Mirrors REPETITION_INDEX_KEY in mod.rs.
        const REPETITION_INDEX_KEY: &str = "_index";
        let value = ctx
            .get(REPETITION_INDEX_KEY)
            .cloned()
            .unwrap_or(Value::None);
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        _py: Python<'_>,
        _input: &dyn PyInput,
        _stream: &mut CombinedStream,
        _ctx: &mut Context,
    ) -> Result<()> {
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledCheck {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        (self.check)(ctx)?;
        let value = Value::None;
        let obj = py.None();
        sink.deposit_leaf(py, value, obj)
    }

    fn exec_build_py(
        &self,
        _py: Python<'_>,
        _input: &dyn PyInput,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        (self.check)(ctx)
    }
}

impl PyCompiledExec for crate::compiled::CompiledStopIf {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            Err(ConstructError::StopField {
                path: String::new(),
            })
        } else {
            let value = Value::None;
            let obj = py.None();
            sink.deposit_leaf(py, value, obj)
        }
    }

    fn exec_build_py(
        &self,
        _py: Python<'_>,
        _input: &dyn PyInput,
        _stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            Err(ConstructError::StopField {
                path: String::new(),
            })
        } else {
            Ok(())
        }
    }
}

// ===========================================================================
// Default / Rebuild (forward parse; build differs)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledDefault {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        self.inner.exec_parse_py(py, stream, ctx, sink)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.extract_ctx_value_py(py)?;
        let effective = if raw.is_none() {
            self.value.clone()
        } else {
            raw
        };
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(effective);
        self.inner.exec_build_py(py, &scalar_input, stream, ctx)
    }
}

impl PyCompiledExec for crate::compiled::CompiledRebuild {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        self.inner.exec_parse_py(py, stream, ctx, sink)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        _input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let value = (self.func)(ctx)?;
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(value);
        self.inner.exec_build_py(py, &scalar_input, stream, ctx)
    }
}

// ===========================================================================
// NamedTuple / TimestampAdapter (decode inner → re-deposit composite)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledNamedTuple {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (raw, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        // Reuse the Value-path decoder (it returns a Value::List of Containers).
        let named = named_tuple_decode_value(&raw, &self.field_names)?;
        deposit_pair_as_leaf(py, sink, named)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.extract_ctx_value_py(py)?;
        let encoded = named_tuple_encode_value(&raw, &self.field_names)?;
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(encoded);
        self.inner.exec_build_py(py, &scalar_input, stream, ctx)
    }
}

/// Local copy of `CompiledNamedTuple::decode_value` (mirrors mod.rs).
///
/// Defined as a free function here because the original is a private
/// associated function on `CompiledNamedTuple` and cannot be called from
/// this module.
fn named_tuple_decode_value(raw: &Value, field_names: &[String]) -> Result<Value> {
    match raw {
        Value::List(items) => {
            if items.len() != field_names.len() {
                return Err(ConstructError::Array {
                    path: String::new(),
                    expected: field_names.len(),
                    actual: items.len(),
                });
            }
            let named: Vec<Value> = items
                .iter()
                .zip(field_names.iter())
                .map(|(val, name)| {
                    let mut entry = indexmap::IndexMap::new();
                    entry.insert("name".to_string(), Value::String(name.clone()));
                    entry.insert("value".to_string(), val.clone());
                    Value::Container(entry)
                })
                .collect();
            Ok(Value::List(named))
        }
        Value::Container(map) => {
            let named: Vec<Value> = field_names
                .iter()
                .map(|name| {
                    let val = map.get(name).cloned().unwrap_or(Value::None);
                    let mut entry = indexmap::IndexMap::new();
                    entry.insert("name".to_string(), Value::String(name.clone()));
                    entry.insert("value".to_string(), val);
                    Value::Container(entry)
                })
                .collect();
            Ok(Value::List(named))
        }
        other => Err(ConstructError::TypeMismatch {
            path: String::new(),
            expected: "List or Container".to_string(),
            actual: other.type_name().to_string(),
        }),
    }
}

impl PyCompiledExec for crate::compiled::CompiledTimestampAdapter {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (raw, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        let raw_i64 = raw.to_i64()?;
        let (secs, nanos) = timestamp_to_secs_nanos(raw_i64, self.unit);
        let mut map = indexmap::IndexMap::new();
        map.insert("secs".to_string(), Value::Int(secs));
        map.insert("nanos".to_string(), Value::UInt(u64::from(nanos)));
        deposit_pair_as_leaf(py, sink, Value::Container(map))
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let raw = input.extract_ctx_value_py(py)?;
        let container = raw.as_container()?;
        let secs = container
            .get("secs")
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: "secs".to_string(),
            })?
            .to_i64()?;
        let nanos = container
            .get("nanos")
            .ok_or_else(|| ConstructError::FieldMissing {
                path: String::new(),
                field: "nanos".to_string(),
            })?
            .to_u64()? as u32;
        let raw_i64 = secs_nanos_to_timestamp(secs, nanos, self.unit);
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(Value::Int(raw_i64));
        self.inner.exec_build_py(py, &scalar_input, stream, ctx)
    }
}

/// Mirrors `timestamp_to_secs_nanos` in mod.rs.
fn timestamp_to_secs_nanos(
    raw: i64,
    unit: crate::constructs::computed::TimestampUnit,
) -> (i64, u32) {
    use crate::constructs::computed::TimestampUnit;
    match unit {
        TimestampUnit::Seconds => (raw, 0u32),
        TimestampUnit::Milliseconds => (raw / 1000, ((raw % 1000) * 1_000_000) as u32),
        TimestampUnit::Microseconds => (raw / 1_000_000, ((raw % 1_000_000) * 1000) as u32),
        TimestampUnit::Nanoseconds => (raw / 1_000_000_000, (raw % 1_000_000_000) as u32),
    }
}

/// Mirrors `secs_nanos_to_timestamp` in mod.rs.
fn secs_nanos_to_timestamp(
    secs: i64,
    nanos: u32,
    unit: crate::constructs::computed::TimestampUnit,
) -> i64 {
    use crate::constructs::computed::TimestampUnit;
    match unit {
        TimestampUnit::Seconds => secs,
        TimestampUnit::Milliseconds => secs * 1000 + (i64::from(nanos) / 1_000_000),
        TimestampUnit::Microseconds => secs * 1_000_000 + (i64::from(nanos) / 1000),
        TimestampUnit::Nanoseconds => secs * 1_000_000_000 + i64::from(nanos),
    }
}

/// Local copy of `CompiledNamedTuple::encode_value` (mirrors mod.rs).
fn named_tuple_encode_value(data: &Value, field_names: &[String]) -> Result<Value> {
    match data {
        Value::List(items) => {
            let plain: Vec<Value> = items
                .iter()
                .zip(field_names.iter())
                .map(|(item, _name)| match item {
                    Value::Container(entry) => entry.get("value").cloned().unwrap_or(Value::None),
                    other => other.clone(),
                })
                .collect();
            Ok(Value::List(plain))
        }
        Value::Container(map) => Ok(Value::Container(map.clone())),
        other => Err(ConstructError::TypeMismatch {
            path: String::new(),
            expected: "List or Container".to_string(),
            actual: other.type_name().to_string(),
        }),
    }
}

// ===========================================================================
// Padded / Aligned / FixedSized (parse inner → consume padding → deposit)
// ===========================================================================
//
// These wrappers parse the inner node, then consume / validate padding
// bytes from the main stream. The inner result is deposited unchanged.

/// Minimum valid modulus for Aligned (mirrors mod.rs).
const ALIGNED_MIN_MODULUS: usize = 2;

impl PyCompiledExec for crate::compiled::CompiledPadded {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let pos_before = stream.tell()?;
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        let pos_after = stream.tell()?;
        let consumed = (pos_after - pos_before) as usize;
        if consumed > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "subcon parsed {} bytes but was allowed only {}",
                    consumed, self.length
                ),
            });
        }
        let pad = self.length - consumed;
        if pad > 0 {
            let padding_bytes = stream.read_bytes(pad)?;
            if self.strict {
                for &b in &padding_bytes {
                    if b != self.pattern {
                        return Err(ConstructError::Padding {
                            path: String::new(),
                            message: format!(
                                "expected padding byte 0x{:02X} but got 0x{:02X}",
                                self.pattern, b
                            ),
                        });
                    }
                }
            }
        }
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let pos_before = stream.tell()?;
        self.inner.exec_build_py(py, input, stream, ctx)?;
        let pos_after = stream.tell()?;
        let written = (pos_after - pos_before) as usize;
        if written > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "subcon build {} bytes but was allowed only {}",
                    written, self.length
                ),
            });
        }
        let pad = self.length - written;
        if pad > 0 {
            let padding = vec![self.pattern; pad];
            stream.write_bytes(&padding)?;
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledAligned {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        if self.modulus < ALIGNED_MIN_MODULUS {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "expected modulus {} or greater, got {}",
                    ALIGNED_MIN_MODULUS, self.modulus
                ),
            });
        }
        let pos_before = stream.tell()?;
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        let pos_after = stream.tell()?;
        let consumed = (pos_after - pos_before) as usize;
        let pad = (self.modulus - (consumed % self.modulus)) % self.modulus;
        if pad > 0 {
            let _ = stream.read_bytes(pad)?;
        }
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if self.modulus < ALIGNED_MIN_MODULUS {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "expected modulus {} or greater, got {}",
                    ALIGNED_MIN_MODULUS, self.modulus
                ),
            });
        }
        let pos_before = stream.tell()?;
        self.inner.exec_build_py(py, input, stream, ctx)?;
        let pos_after = stream.tell()?;
        let written = (pos_after - pos_before) as usize;
        let pad = (self.modulus - (written % self.modulus)) % self.modulus;
        if pad > 0 {
            let padding = vec![self.pattern; pad];
            stream.write_bytes(&padding)?;
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledFixedSized {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        // Read fixed-size chunk, parse inner from sub-stream.
        let data = stream.read_bytes(self.length)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&data));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        if built.len() > self.length {
            return Err(ConstructError::Padding {
                path: String::new(),
                message: format!(
                    "subcon build {} bytes but was allowed only {}",
                    built.len(),
                    self.length
                ),
            });
        }
        stream.write_bytes(&built)?;
        let pad = self.length - built.len();
        if pad > 0 {
            let padding = vec![0u8; pad];
            stream.write_bytes(&padding)?;
        }
        Ok(())
    }
}

// ===========================================================================
// Composite PyCompiledExec implementations
// ===========================================================================
//
// Composite compiled nodes (Struct / Sequence / Union / Select / FocusedSeq
// / Array / ArrayExpr / GreedyRange / RepeatUntil) hold a `Vec` of compiled
// children. The Py path mirrors the Value path: for each child, create a
// sub-sink via `sub_py_field` / `sub_py_item`, recurse `exec_parse_py` into
// it, then integrate via `finish_field_py` / `finish_item_py`. The auto-
// transition Empty → Struct / Array happens on the first integration call.

/// Context key for the current loop index (mirrors mod.rs).
const REPETITION_INDEX_KEY: &str = "_index";

impl PyCompiledExec for crate::compiled::CompiledStruct {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        for field in &self.fields {
            match &field.name {
                Some(name) => {
                    let mut sub = sink.sub_py_field(py, name)?;
                    match field
                        .subcon
                        .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        Ok(()) => {}
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    }
                    let value = sink.finish_field_py(py, name, sub)?;
                    child_ctx.insert(name.clone(), value);
                }
                None => {
                    let mut sub = sink.sub_py_item(py)?;
                    match field
                        .subcon
                        .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        Ok(()) => sink.finish_item_py(py, sub)?,
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix("(anonymous)")),
                    }
                }
            }
        }
        Ok(())
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        for field in &self.fields {
            let field_input: Box<dyn PyInput> = match &field.name {
                Some(name) => {
                    if field.flagbuildnone && !input.has_field_py(py, name) {
                        Box::new(crate::compiled::py_input::PyScalarInput::new(Value::None))
                    } else {
                        input.sub_field_py(py, name)?
                    }
                }
                None => Box::new(crate::compiled::py_input::PyScalarInput::new(Value::None)),
            };
            let ctx_value = field_input.extract_ctx_value_py(py)?;
            if let Some(name) = &field.name {
                child_ctx.insert(name.clone(), ctx_value);
            }
            match field
                .subcon
                .exec_build_py(py, &*field_input, stream, &mut child_ctx)
            {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => {
                    return Err(if let Some(name) = &field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            }
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledSequence {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        for entry in &self.entries {
            match &entry.name {
                Some(name) => {
                    let mut sub = sink.sub_py_item(py)?;
                    match entry
                        .subcon
                        .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        Ok(()) => {}
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    }
                    let value = sink.finish_named_item_py(py, name, sub)?;
                    child_ctx.insert(name.clone(), value);
                }
                None => {
                    let mut sub = sink.sub_py_item(py)?;
                    match entry
                        .subcon
                        .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        Ok(()) => sink.finish_item_py(py, sub)?,
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix("(anonymous)")),
                    }
                }
            }
        }
        Ok(())
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        for (i, entry) in self.entries.iter().enumerate() {
            let entry_input = input.sub_index_py(py, i)?;
            let build_value = entry_input.extract_ctx_value_py(py)?;
            if let Some(name) = &entry.name {
                child_ctx.insert(name.clone(), build_value);
            }
            match entry
                .subcon
                .exec_build_py(py, &*entry_input, stream, &mut child_ctx)
            {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => {
                    return Err(if let Some(name) = &entry.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix(&format!("[{i}]"))
                    });
                }
            }
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledArray {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        for i in 0..self.count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let mut sub = sink.sub_py_item(py)?;
            match self.subcon.exec_parse_py(py, stream, ctx, sub.as_mut()) {
                Ok(()) => {
                    if !self.discard {
                        sink.finish_item_py(py, sub)?;
                    }
                }
                Err(e) => return Err(e.with_path_prefix(&format!("[{i}]"))),
            }
        }
        Ok(())
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Fast path: homogeneous int array batch extraction.
        if let Some(ints) = input.extract_batch_int_py(py)? {
            if ints.len() != self.count {
                return Err(ConstructError::Array {
                    path: String::new(),
                    expected: self.count,
                    actual: ints.len(),
                });
            }
            for (i, &val) in ints.iter().enumerate() {
                ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
                let elem_input = crate::compiled::py_input::PyScalarInput::new(Value::Int(val));
                self.subcon
                    .exec_build_py(py, &elem_input, stream, ctx)
                    .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            }
            return Ok(());
        }
        // Fallback: per-index extraction.
        let len = input.len_py(py);
        if len != self.count {
            return Err(ConstructError::Array {
                path: String::new(),
                expected: self.count,
                actual: len,
            });
        }
        for i in 0..self.count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = input.sub_index_py(py, i)?;
            self.subcon
                .exec_build_py(py, &*elem_input, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledArrayExpr {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let count_val = self
            .compiled_count
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("ArrayExpr"))?;
        let count = count_val.to_u64().map_err(|e| {
            ConstructError::Expr {
                path: String::new(),
                message: format!("ArrayExpr count must be a non-negative integer: {e}"),
            }
            .with_path_prefix("ArrayExpr")
        })? as usize;

        for i in 0..count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let mut sub = sink.sub_py_item(py)?;
            match self.subcon.exec_parse_py(py, stream, ctx, sub.as_mut()) {
                Ok(()) => sink.finish_item_py(py, sub)?,
                Err(e) => return Err(e.with_path_prefix(&format!("[{i}]"))),
            }
        }
        Ok(())
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let expected_val = self
            .compiled_count
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("ArrayExpr"))?;
        let expected = expected_val.to_u64().map_err(|e| {
            ConstructError::Expr {
                path: String::new(),
                message: format!("ArrayExpr count must be a non-negative integer: {e}"),
            }
            .with_path_prefix("ArrayExpr")
        })? as usize;
        if let Some(ints) = input.extract_batch_int_py(py)? {
            if ints.len() != expected {
                return Err(ConstructError::Array {
                    path: String::new(),
                    expected,
                    actual: ints.len(),
                });
            }
            for (i, &val) in ints.iter().enumerate() {
                ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
                let elem_input = crate::compiled::py_input::PyScalarInput::new(Value::Int(val));
                self.subcon
                    .exec_build_py(py, &elem_input, stream, ctx)
                    .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            }
            return Ok(());
        }
        let len = input.len_py(py);
        if len != expected {
            return Err(ConstructError::Array {
                path: String::new(),
                expected,
                actual: len,
            });
        }
        for i in 0..expected {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = input.sub_index_py(py, i)?;
            self.subcon
                .exec_build_py(py, &*elem_input, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledGreedyRange {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let mut i: u64 = 0;
        loop {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i));
            let fallback = stream.tell()?;
            let mut sub = sink.sub_py_item(py)?;
            match self.subcon.exec_parse_py(py, stream, ctx, sub.as_mut()) {
                Ok(()) => {
                    if !self.discard {
                        sink.finish_item_py(py, sub)?;
                    }
                    i = i.saturating_add(1);
                }
                Err(ConstructError::StopField { .. }) => break,
                Err(_) => {
                    // Any other error: seek back and stop.
                    stream.seek(fallback)?;
                    break;
                }
            }
        }
        Ok(())
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let len = input.len_py(py);
        for i in 0..len {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = input.sub_index_py(py, i)?;
            self.subcon
                .exec_build_py(py, &*elem_input, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledRepeatUntil {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let mut i: u64 = 0;
        let mut local_list: Vec<Value> = Vec::new();
        loop {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i));
            let mut sub = sink.sub_py_item(py)?;
            self.subcon
                .exec_parse_py(py, stream, ctx, sub.as_mut())
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            let (element, _obj) = sub.into_pair(py)?;

            if !self.discard {
                local_list.push(element.clone());
                let obj = value_to_py_scalar(py, &element)
                    .or_else(|_| value_to_py_composite_fallback(py, &element))
                    .map_err(py_err_to_construct)?;
                let mut item_sink = sink.sub_py_item(py)?;
                item_sink.deposit_leaf(py, element.clone(), obj.clone_ref(py))?;
                sink.finish_item_py(py, item_sink)?;
            }

            if (self.predicate)(&element, &local_list, ctx) {
                return Ok(());
            }
            i = i.saturating_add(1);
        }
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let len = input.len_py(py);
        let mut built: Vec<Value> = Vec::new();
        for i in 0..len {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = input.sub_index_py(py, i)?;
            let element = elem_input.extract_ctx_value_py(py)?;
            self.subcon
                .exec_build_py(py, &*elem_input, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            if !self.discard {
                built.push(element.clone());
            }
            if (self.predicate)(&element, &built, ctx) {
                return Ok(());
            }
        }
        Err(ConstructError::Array {
            path: String::new(),
            expected: 0,
            actual: 0,
        })
    }
}

impl PyCompiledExec for crate::compiled::CompiledSelect {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        for sc in &self.subcons {
            let mut sub = sink.sub_py_item(py)?;
            match sc.exec_parse_py(py, stream, ctx, sub.as_mut()) {
                Ok(()) => {
                    let (value, obj) = sub.into_pair(py)?;
                    sink.deposit_leaf(py, value, obj)?;
                    return Ok(());
                }
                Err(ConstructError::StopField { .. }) => {
                    return Err(ConstructError::StopField {
                        path: String::new(),
                    });
                }
                Err(_) => {
                    stream.seek(fallback)?;
                }
            }
        }
        Err(ConstructError::Select {
            path: String::new(),
            message: format!(
                "no subconstruct matched after trying {} candidates",
                self.subcons.len()
            ),
        })
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        for sc in &self.subcons {
            let mut build_stream = CombinedStream::ByteStream(ByteStream::new_write());
            match sc.exec_build_py(py, input, &mut build_stream, ctx) {
                Ok(()) => {
                    let bytes = build_stream.into_bytes();
                    stream.write_bytes(&bytes)?;
                    return Ok(());
                }
                Err(ConstructError::StopField { .. }) => {
                    return Err(ConstructError::StopField {
                        path: String::new(),
                    });
                }
                Err(_) => {
                    // Try next subconstruct.
                }
            }
        }
        Err(ConstructError::Select {
            path: String::new(),
            message: format!(
                "no subconstruct matched for building after trying {} candidates",
                self.subcons.len()
            ),
        })
    }
}

impl PyCompiledExec for crate::compiled::CompiledUnion {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        let fallback = stream.tell()?;
        let mut forward_by_index: Vec<u64> = Vec::with_capacity(self.fields.len());
        let mut forward_by_name: indexmap::IndexMap<String, u64> =
            indexmap::IndexMap::with_capacity(self.fields.len());

        for (i, field) in self.fields.iter().enumerate() {
            match &field.name {
                Some(name) => {
                    let mut sub = sink.sub_py_field(py, name)?;
                    match field
                        .subcon
                        .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        Ok(()) => {}
                        Err(e) => return Err(e.with_path_prefix(name)),
                    }
                    let value = sink.finish_field_py(py, name, sub)?;
                    let forward_pos = stream.tell()?;
                    forward_by_index.push(forward_pos);
                    forward_by_name.insert(name.clone(), forward_pos);
                    child_ctx.insert(name.clone(), value);
                }
                None => {
                    let mut sub = sink.sub_py_item(py)?;
                    if let Err(e) =
                        field
                            .subcon
                            .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        return Err(e.with_path_prefix(&format!("[{i}]")));
                    }
                    let forward_pos = stream.tell()?;
                    forward_by_index.push(forward_pos);
                }
            }
            stream.seek(fallback)?;
        }

        if let Some(ref target) = self.parsefrom {
            let pos = match target {
                crate::constructs::union::UnionTarget::Index(idx) => forward_by_index
                    .get(*idx)
                    .copied()
                    .ok_or_else(|| ConstructError::Index {
                        path: String::new(),
                        index: *idx,
                        length: forward_by_index.len(),
                    }),
                crate::constructs::union::UnionTarget::Name(name) => forward_by_name
                    .get(name)
                    .copied()
                    .ok_or_else(|| ConstructError::FieldMissing {
                        path: String::new(),
                        field: name.clone(),
                    }),
            }?;
            stream.seek(pos)?;
        }
        Ok(())
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        for field in &self.fields {
            let name_ref = field.name.as_deref();
            let should_build = match name_ref {
                Some(n) => input.has_field_py(py, n),
                None => false,
            };
            if !should_build {
                continue;
            }
            let field_input: Box<dyn PyInput> = match name_ref {
                Some(n) => input.sub_field_py(py, n)?,
                None => Box::new(crate::compiled::py_input::PyScalarInput::new(Value::None)),
            };
            let ctx_value = field_input.extract_ctx_value_py(py)?;
            if let Some(name) = &field.name {
                child_ctx.insert(name.clone(), ctx_value);
            }
            return field
                .subcon
                .exec_build_py(py, &*field_input, stream, &mut child_ctx)
                .map_err(|e| {
                    if let Some(name) = &field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    }
                });
        }
        Err(ConstructError::Union {
            path: String::new(),
            message: "cannot build, none of the subcons were found in the dictionary".to_string(),
        })
    }
}

impl PyCompiledExec for crate::compiled::CompiledFocusedSeq {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let mut child_ctx = ctx.subcontext();
        let mut focused_value: Option<Value> = None;

        for field in &self.fields {
            match &field.name {
                Some(name) => {
                    // FocusedSeq extracts the Value via a temporary sub-sink
                    // WITHOUT integrating it into the parent sink (mirrors
                    // exec_parse_field_value in mod.rs).
                    let mut sub = sink.sub_py_field(py, name)?;
                    match field
                        .subcon
                        .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        Ok(()) => {}
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix(name)),
                    }
                    let (value, _obj) = sub.into_pair(py)?;
                    child_ctx.insert(name.clone(), value.clone());

                    if name == &self.parsebuildfrom {
                        focused_value = Some(value);
                    }
                }
                None => {
                    let mut sub = sink.sub_py_item(py)?;
                    match field
                        .subcon
                        .exec_parse_py(py, stream, &mut child_ctx, sub.as_mut())
                    {
                        Ok(()) => {}
                        Err(ConstructError::StopField { .. }) => break,
                        Err(e) => return Err(e.with_path_prefix("(anonymous)")),
                    }
                }
            }
        }

        match focused_value {
            Some(v) => deposit_pair_as_leaf(py, sink, v),
            None => Err(ConstructError::FieldMissing {
                path: String::new(),
                field: self.parsebuildfrom.clone(),
            }),
        }
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let input_val = input.extract_ctx_value_py(py)?;
        let mut child_ctx = ctx.subcontext();
        child_ctx.insert(self.parsebuildfrom.clone(), input_val.clone());
        for field in &self.fields {
            let name_ref = field.name.as_deref();
            let build_value = if name_ref == Some(self.parsebuildfrom.as_str()) {
                input_val.clone()
            } else {
                Value::None
            };
            if let Some(name) = &field.name {
                child_ctx.insert(name.clone(), build_value.clone());
            }
            let field_input = crate::compiled::py_input::PyScalarInput::new(build_value);
            match field
                .subcon
                .exec_build_py(py, &field_input, stream, &mut child_ctx)
            {
                Ok(()) => {}
                Err(ConstructError::StopField { .. }) => return Ok(()),
                Err(e) => {
                    return Err(if let Some(name) = &field.name {
                        e.with_path_prefix(name)
                    } else {
                        e.with_path_prefix("(anonymous)")
                    });
                }
            }
        }
        Ok(())
    }
}

// ===========================================================================
// Lazy composite wrappers (LazyArray / Rebuffered)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledLazyArray {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        for i in 0..self.count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let mut sub = sink.sub_py_item(py)?;
            match self.subcon.exec_parse_py(py, stream, ctx, sub.as_mut()) {
                Ok(()) => {
                    // Integrate the child sub-sink as an Array item. This
                    // auto-transitions the parent sink `Empty → Array` on the
                    // first element and appends on subsequent ones — the same
                    // protocol used by `CompiledArray` / `CompiledGreedyRange`.
                    //
                    // NOTE: do NOT use `deposit_leaf` here. `deposit_leaf` only
                    // succeeds while the sink is `Empty`; calling it inside a
                    // loop corrupts the output for count >= 2 (second call
                    // errors) and silently demotes count == 1 to a bare scalar
                    // instead of a single-element list.
                    sink.finish_item_py(py, sub)?;
                }
                Err(e) => return Err(e.with_path_prefix(&format!("[{i}]"))),
            }
        }
        Ok(())
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if let Some(ints) = input.extract_batch_int_py(py)? {
            if ints.len() != self.count {
                return Err(ConstructError::Array {
                    path: String::new(),
                    expected: self.count,
                    actual: ints.len(),
                });
            }
            for (i, &val) in ints.iter().enumerate() {
                ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
                let elem_input = crate::compiled::py_input::PyScalarInput::new(Value::Int(val));
                self.subcon
                    .exec_build_py(py, &elem_input, stream, ctx)
                    .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
            }
            return Ok(());
        }
        let len = input.len_py(py);
        if len != self.count {
            return Err(ConstructError::Array {
                path: String::new(),
                expected: self.count,
                actual: len,
            });
        }
        for i in 0..self.count {
            ctx.insert(REPETITION_INDEX_KEY, Value::UInt(i as u64));
            let elem_input = input.sub_index_py(py, i)?;
            self.subcon
                .exec_build_py(py, &*elem_input, stream, ctx)
                .map_err(|e| e.with_path_prefix(&format!("[{i}]")))?;
        }
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledRebuffered {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let start = stream.tell()?;
        let data = stream.read_remaining()?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&data));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        let consumed = sub_stream.tell()?;
        stream.seek(start + consumed)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        self.inner.exec_build_py(py, input, stream, ctx)
    }
}

// ===========================================================================
// Stream ops / tunneling PyCompiledExec
// ===========================================================================
//
// Stream ops read bytes from the main stream, transform them (swap bits /
// bytes, decompress, etc.), create a sub-stream from the transformed bytes,
// and parse the inner node from the sub-stream. The Py path passes the
// parent sink through to `inner.exec_parse_py` (with the sub-stream).

impl PyCompiledExec for crate::compiled::CompiledBitwise {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let data = stream.read_remaining()?;
        let bits = crate::binary::bytes2bits(&data);
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&bits));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let bits = sub_stream.into_bytes();
        let bytes = crate::binary::bits2bytes(&bits)?;
        stream.write_bytes(&bytes)
    }
}

impl PyCompiledExec for crate::compiled::CompiledBytewise {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let bits = stream.read_remaining()?;
        let bytes = crate::binary::bits2bytes(&bits)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&bytes));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let bytes = sub_stream.into_bytes();
        let bits = crate::binary::bytes2bits(&bytes);
        stream.write_bytes(&bits)
    }
}

impl PyCompiledExec for crate::compiled::CompiledByteSwapped {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let size = self.inner.exec_sizeof(ctx)?;
        let data = stream.read_bytes(size)?;
        let swapped = crate::binary::swapbytes(&data);
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&swapped));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let swapped = crate::binary::swapbytes(&built);
        stream.write_bytes(&swapped)
    }
}

impl PyCompiledExec for crate::compiled::CompiledBitsSwapped {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let data = if self.fixed_size {
            let size = self.inner.exec_sizeof(ctx)?;
            stream.read_bytes(size)?
        } else {
            stream.read_remaining()?
        };
        let swapped = swap_bits_in_bytes(&data);
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&swapped));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let swapped = swap_bits_in_bytes(&built);
        stream.write_bytes(&swapped)
    }
}

/// Reverses the bit order within each byte of `data` (mirrors mod.rs).
fn swap_bits_in_bytes(data: &[u8]) -> Vec<u8> {
    data.iter()
        .map(|&b| {
            let mut result = 0u8;
            for i in 0..8 {
                if b & (1 << i) != 0 {
                    result |= 1 << (7 - i);
                }
            }
            result
        })
        .collect()
}

impl PyCompiledExec for crate::compiled::CompiledTransformed {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let data = match self.decode_amount {
            Some(amount) => stream.read_bytes(amount)?,
            None => stream.read_remaining()?,
        };
        let decoded = (self.decode)(&data)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&decoded));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let encoded = (self.encode)(&built)?;
        if let Some(expected) = self.encode_amount {
            if encoded.len() != expected {
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: format!(
                        "Transformed: encode produced {} bytes but expected {}",
                        encoded.len(),
                        expected
                    ),
                });
            }
        }
        stream.write_bytes(&encoded)
    }
}

impl PyCompiledExec for crate::compiled::CompiledRestreamed {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        if self.decoder_unit == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "Restreamed decoder_unit must be positive".to_string(),
            });
        }
        let raw = stream.read_remaining()?;
        let mut decoded = Vec::new();
        for chunk in raw.chunks(self.decoder_unit) {
            let decoded_chunk = (self.decoder)(chunk)?;
            decoded.extend_from_slice(&decoded_chunk);
        }
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&decoded));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if self.encoder_unit == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "Restreamed encoder_unit must be positive".to_string(),
            });
        }
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        for chunk in built.chunks(self.encoder_unit) {
            let encoded_chunk = (self.encoder)(chunk)?;
            stream.write_bytes(&encoded_chunk)?;
        }
        Ok(())
    }
}

#[cfg(feature = "compression")]
impl PyCompiledExec for crate::compiled::CompiledCompressed {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        use crate::constructs::stream_ops::CompressionAlgorithm;
        let data = stream.read_remaining()?;
        let decoded = match self.algorithm {
            CompressionAlgorithm::Zlib => {
                use std::io::Read;
                let mut decoder = flate2::read::ZlibDecoder::new(&data[..]);
                let mut out = Vec::new();
                decoder
                    .read_to_end(&mut out)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("zlib decompression failed: {e}"),
                    })?;
                out
            }
            CompressionAlgorithm::Deflate => {
                use std::io::Read;
                let mut decoder = flate2::read::DeflateDecoder::new(&data[..]);
                let mut out = Vec::new();
                decoder
                    .read_to_end(&mut out)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("deflate decompression failed: {e}"),
                    })?;
                out
            }
        };
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&decoded));
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        use crate::constructs::stream_ops::CompressionAlgorithm;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.inner.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let encoded = match self.algorithm {
            CompressionAlgorithm::Zlib => {
                use std::io::Write;
                let mut encoder =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                encoder
                    .write_all(&built)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("zlib compression failed: {e}"),
                    })?;
                encoder.finish().map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("zlib compression finalize failed: {e}"),
                })?
            }
            CompressionAlgorithm::Deflate => {
                use std::io::Write;
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder
                    .write_all(&built)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("deflate compression failed: {e}"),
                    })?;
                encoder.finish().map_err(|e| ConstructError::Generic {
                    path: String::new(),
                    message: format!("deflate compression finalize failed: {e}"),
                })?
            }
        };
        stream.write_bytes(&encoded)
    }
}

// -- Pointer / PointerExpr / Peek / Prefixed (seek + sub-stream) ---------

impl PyCompiledExec for crate::compiled::CompiledPointer {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        seek_to_offset(stream, self.offset)?;
        let result = self.inner.exec_parse_py(py, stream, ctx, sink);
        let _ = stream.seek(fallback);
        result
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        seek_to_offset(stream, self.offset)?;
        let result = self.inner.exec_build_py(py, input, stream, ctx);
        let _ = stream.seek(fallback);
        result
    }
}

impl PyCompiledExec for crate::compiled::CompiledPointerExpr {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let offset_val = self
            .compiled_offset
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("PointerExpr"))?;
        let offset = offset_val.to_i64().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("PointerExpr offset must be an integer: {e}"),
        })?;
        let fallback = stream.tell()?;
        seek_to_offset(stream, offset)?;
        let result = self.inner.exec_parse_py(py, stream, ctx, sink);
        let _ = stream.seek(fallback);
        result
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let offset_val = self
            .compiled_offset
            .eval(ctx, None)
            .map_err(|e| e.with_path_prefix("PointerExpr"))?;
        let offset = offset_val.to_i64().map_err(|e| ConstructError::Expr {
            path: String::new(),
            message: format!("PointerExpr offset must be an integer: {e}"),
        })?;
        let fallback = stream.tell()?;
        seek_to_offset(stream, offset)?;
        let result = self.inner.exec_build_py(py, input, stream, ctx);
        let _ = stream.seek(fallback);
        result
    }
}

/// Seeks the stream to the given offset (mirrors mod.rs).
fn seek_to_offset(stream: &mut CombinedStream, offset: i64) -> Result<()> {
    use std::io::SeekFrom;
    if offset >= 0 {
        stream.seek(offset as u64)?;
        Ok(())
    } else {
        stream.seek_from(SeekFrom::End(offset))?;
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledPeek {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let fallback = stream.tell()?;
        let result = self.inner.exec_parse_py(py, stream, ctx, sink);
        let _ = stream.seek(fallback);
        match result {
            Ok(()) => Ok(()),
            Err(e @ ConstructError::Check { .. }) | Err(e @ ConstructError::StopField { .. }) => {
                Err(e)
            }
            Err(_) => {
                let value = Value::None;
                let obj = py.None();
                sink.deposit_leaf(py, value, obj)
            }
        }
    }

    fn exec_build_py(
        &self,
        _py: Python<'_>,
        _input: &dyn PyInput,
        _stream: &mut CombinedStream,
        _ctx: &mut Context,
    ) -> Result<()> {
        // Peek does not write anything during build (mirrors Python original).
        Ok(())
    }
}

impl PyCompiledExec for crate::compiled::CompiledPrefixed {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        // Parse length field into a temp sink, then read payload, parse subcon.
        let (length_val, _obj) = exec_parse_inner_py(&self.length_field, py, stream, ctx, sink)?;
        let mut length = length_val.to_u64()? as usize;
        if self.include_length {
            let lf_size = self.length_field.exec_sizeof(ctx)?;
            length = length.saturating_sub(lf_size);
        }
        let data = stream.read_bytes(length)?;
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_read(&data));
        let (value, _obj) = exec_parse_inner_py(&self.subcon, py, &mut sub_stream, ctx, sink)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Build subcon into a temporary stream to measure length.
        let mut sub_stream = CombinedStream::ByteStream(ByteStream::new_write());
        self.subcon.exec_build_py(py, input, &mut sub_stream, ctx)?;
        let built_data = sub_stream.into_bytes();
        let mut length = built_data.len() as u64;
        if self.include_length {
            let lf_size = self.length_field.exec_sizeof(ctx)?;
            length += lf_size as u64;
        }
        // Build the length field with the computed length value.
        let lf_input = crate::compiled::py_input::PyScalarInput::new(Value::UInt(length));
        self.length_field
            .exec_build_py(py, &lf_input, stream, ctx)?;
        // Append the built subcon data.
        stream.write_bytes(&built_data)
    }
}

impl PyCompiledExec for crate::compiled::CompiledRawCopy {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let offset1 = stream.tell()?;
        let (value, _obj) = exec_parse_inner_py(&self.inner, py, stream, ctx, sink)?;
        let offset2 = stream.tell()?;
        let length = offset2.saturating_sub(offset1) as usize;
        stream.seek(offset1)?;
        let data = stream.read_bytes(length)?;
        let mut map = indexmap::IndexMap::new();
        map.insert("data".to_string(), Value::Bytes(data));
        map.insert("value".to_string(), value);
        map.insert("offset1".to_string(), Value::UInt(offset1));
        map.insert("offset2".to_string(), Value::UInt(offset2));
        map.insert("length".to_string(), Value::UInt(length as u64));
        deposit_pair_as_leaf(py, sink, Value::Container(map))
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // RawCopy build accepts either {"data": bytes} or {"value": ...}.
        let raw = input.extract_ctx_value_py(py)?;
        match &raw {
            Value::Container(map) => {
                if let Some(Value::Bytes(data)) = map.get("data") {
                    return stream.write_bytes(data);
                }
                if let Some(value) = map.get("value") {
                    let scalar_input = crate::compiled::py_input::PyScalarInput::new(value.clone());
                    return self.inner.exec_build_py(py, &scalar_input, stream, ctx);
                }
                Err(ConstructError::Generic {
                    path: String::new(),
                    message: "RawCopy build requires 'data' or 'value' key".to_string(),
                })
            }
            // If the input is itself bytes (no container wrapper), write directly.
            Value::Bytes(data) => stream.write_bytes(data),
            _ => {
                // Fall back: treat the input as a value for the inner subcon.
                let scalar_input = crate::compiled::py_input::PyScalarInput::new(raw);
                self.inner.exec_build_py(py, &scalar_input, stream, ctx)
            }
        }
    }
}

impl PyCompiledExec for crate::compiled::CompiledChecksum {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let (parsed, _obj) = exec_parse_inner_py(&self.checksum_field, py, stream, ctx, sink)?;
        let raw_bytes = (self.bytes_func)(ctx)?;
        let expected = (self.hash_func)(&raw_bytes);
        let parsed_bytes = parsed.as_bytes().unwrap_or(&[]).to_vec();
        if parsed_bytes != expected {
            return Err(ConstructError::Check {
                path: String::new(),
                message: format!(
                    "wrong checksum: read {:02x?}, computed {:02x?}",
                    parsed_bytes, expected
                ),
            });
        }
        deposit_pair_as_leaf(py, sink, parsed)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        _input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Compute the checksum from the context-extracted bytes, then build the
        // checksum field with that value.
        let raw_bytes = (self.bytes_func)(ctx)?;
        let checksum = (self.hash_func)(&raw_bytes);
        let scalar_input = crate::compiled::py_input::PyScalarInput::new(Value::Bytes(checksum));
        self.checksum_field
            .exec_build_py(py, &scalar_input, stream, ctx)
    }
}

// ===========================================================================
// Control flow PyCompiledExec
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledIfThenElse {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            self.then_constr.exec_parse_py(py, stream, ctx, sink)
        } else {
            self.else_constr.exec_parse_py(py, stream, ctx, sink)
        }
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        if (self.cond)(ctx) {
            self.then_constr.exec_build_py(py, input, stream, ctx)
        } else {
            self.else_constr.exec_build_py(py, input, stream, ctx)
        }
    }
}

impl PyCompiledExec for crate::compiled::CompiledSwitch {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let key = (self.keyfunc)(ctx)?;
        let node = self
            .cases
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, n)| n.as_ref())
            .or(self.default.as_deref());
        match node {
            Some(n) => n.exec_parse_py(py, stream, ctx, sink),
            None => {
                // No default — deposit None (matches Value-path behavior).
                let value = Value::None;
                let obj = py.None();
                sink.deposit_leaf(py, value, obj)
            }
        }
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        let key = (self.keyfunc)(ctx)?;
        let node = self
            .cases
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, n)| n.as_ref())
            .or(self.default.as_deref());
        match node {
            Some(n) => n.exec_build_py(py, input, stream, ctx),
            None => {
                // No matching case and no default — build nothing (matches
                // Python's Switch which returns Pass for missing keys).
                Ok(())
            }
        }
    }
}

// ===========================================================================
// Escape-hatch PyCompiledExec (Dynamic / LazyBound / External)
// ===========================================================================

impl PyCompiledExec for crate::compiled::CompiledDynamic {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        // Delegate to the wrapped construct's old-path parse, producing a
        // Value tree. Then convert to PyObject via the composite fallback
        // (Dynamic may produce composite Value trees). This is the
        // escape-hatch path — not the hot path.
        let value = self.inner.parse(stream, ctx)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Escape hatch: extract the input as a Value and delegate to the
        // legacy Value-path build.
        let value = input.extract_ctx_value_py(py)?;
        self.inner.build(&value, stream, ctx)
    }
}

impl PyCompiledExec for crate::compiled::CompiledLazyBound {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        // Same as Dynamic: delegate to the wrapped construct.
        let value = self.inner.parse(stream, ctx)?;
        deposit_pair_as_leaf(py, sink, value)
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Escape hatch: extract the input as a Value and delegate to the
        // legacy Value-path build.
        let value = input.extract_ctx_value_py(py)?;
        self.inner.build(&value, stream, ctx)
    }
}

impl PyCompiledExec for crate::compiled::CompiledExternal {
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        let produced = self.inner.ext_parse(stream, ctx)?;
        match produced {
            crate::compiled::ProducedOutput::Value(v) => deposit_pair_as_leaf(py, sink, v),
            // Phase 16.3 limitation: ProducedOutput::Object cannot be
            // downcast from construct-rs (the box is opaque). Return a
            // descriptive error — full Object-path support is added in
            // Phase 16.5 if needed.
            crate::compiled::ProducedOutput::Object(_) => Err(ConstructError::Generic {
                path: String::new(),
                message:
                    "CompiledExternal::ProducedOutput::Object path not supported in Phase 16.3"
                        .to_string(),
            }),
        }
    }

    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        // Escape hatch: extract Value from PyInput, wrap in OwnedValueInput,
        // and delegate to the Value-path ext_build.
        let value = input.extract_ctx_value_py(py)?;
        let owned = crate::compiled::input::OwnedValueInput::new(value);
        self.inner.ext_build(&owned, stream, ctx)
    }
}

// ===========================================================================
// Blanket impl on `CompiledNode` so `Box<CompiledNode>` / `&CompiledNode`
// auto-deref into dispatch. Recursion is avoided: dispatch matches on the
// concrete inner variant struct, not on `CompiledNode` itself.
// ===========================================================================

impl PyCompiledExec for CompiledNode {
    #[inline]
    fn exec_parse_py(
        &self,
        py: Python<'_>,
        stream: &mut CombinedStream,
        ctx: &mut Context,
        sink: &mut dyn PySink,
    ) -> Result<()> {
        exec_parse_py_dispatch(self, py, stream, ctx, sink)
    }

    #[inline]
    fn exec_build_py(
        &self,
        py: Python<'_>,
        input: &dyn PyInput,
        stream: &mut CombinedStream,
        ctx: &mut Context,
    ) -> Result<()> {
        exec_build_py_dispatch(self, py, input, stream, ctx)
    }
}

// ===========================================================================
// Dispatch functions (71-arm match)
// ===========================================================================
//
// These dispatch functions ensure compile-time exhaustiveness: if a new
// variant is added to `CompiledNode` without a `PyCompiledExec`
// implementation, the match fails to compile. This is the safety net that
// prevents silent regressions when the enum grows.

/// Dispatches [`PyCompiledExec::exec_parse_py`] over all `CompiledNode`
/// variants.
///
/// Generated as an explicit 71-arm match (rather than via
/// `#[enum_dispatch]`) because `CompiledNode` already has
/// `#[enum_dispatch(CompiledExec)]` applied, and applying a second
/// `enum_dispatch` on the same enum for a different trait is not
/// officially supported. An explicit macro match provides the same
/// compile-time exhaustiveness guarantee.
///
/// # Errors
///
/// Propagates any [`ConstructError`] from the dispatched implementation.
pub fn exec_parse_py_dispatch(
    node: &CompiledNode,
    py: Python<'_>,
    stream: &mut CombinedStream,
    ctx: &mut Context,
    sink: &mut dyn PySink,
) -> Result<()> {
    match node {
        CompiledNode::Subconstruct(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Renamed(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::FormatField(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Bytes(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::GreedyBytes(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::BytesExpr(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::BytesInteger(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::BitsInteger(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::VarInt(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::ZigZag(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Flag(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::CString(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::PaddedString(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Const(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Mapping(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Adapter(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::SymmetricAdapter(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::ExprAdapter(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Validator(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::ExprValidator(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Pass(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Terminated(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Tell(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Seek(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::SeekExpr(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Error(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Struct(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Sequence(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Union(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Select(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::FocusedSeq(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Enum(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::FlagsEnum(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Computed(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Rebuild(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Default(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Index(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Padded(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Aligned(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::FixedSized(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::NamedTuple(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::TimestampAdapter(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Array(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::ArrayExpr(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::GreedyRange(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::RepeatUntil(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Lazy(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::LazyStruct(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::LazyArray(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Rebuffered(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Bitwise(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Bytewise(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Pointer(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::PointerExpr(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Peek(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::RawCopy(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Prefixed(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Transformed(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Restreamed(n) => n.exec_parse_py(py, stream, ctx, sink),
        #[cfg(feature = "compression")]
        CompiledNode::Compressed(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Checksum(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::ByteSwapped(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::BitsSwapped(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::LazyBound(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::IfThenElse(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Switch(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Check(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::StopIf(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Hex(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::HexDump(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::Dynamic(n) => n.exec_parse_py(py, stream, ctx, sink),
        CompiledNode::External(n) => n.exec_parse_py(py, stream, ctx, sink),
    }
}

/// Dispatches [`PyCompiledExec::exec_build_py`] over all `CompiledNode`
/// variants.
///
/// This is the build-direction entry point for the Python-direct path
/// (Phase 16.4). It reads field values lazily from a [`PyInput`] (typically
/// [`PyDictInput`](crate::compiled::py_input::PyDictInput)) and writes
/// binary output directly to the stream — no intermediate `Value` tree is
/// built.
///
/// # Errors
///
/// Propagates any [`ConstructError`] from the build traversal (field
/// missing, type mismatch, stream write failure, …).
pub fn exec_build_py_dispatch(
    node: &CompiledNode,
    py: Python<'_>,
    input: &dyn PyInput,
    stream: &mut CombinedStream,
    ctx: &mut Context,
) -> Result<()> {
    match node {
        CompiledNode::Subconstruct(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Renamed(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::FormatField(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Bytes(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::GreedyBytes(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::BytesExpr(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::BytesInteger(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::BitsInteger(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::VarInt(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::ZigZag(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Flag(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::CString(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::PaddedString(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Const(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Mapping(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Adapter(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::SymmetricAdapter(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::ExprAdapter(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Validator(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::ExprValidator(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Pass(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Terminated(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Tell(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Seek(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::SeekExpr(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Error(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Struct(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Sequence(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Union(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Select(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::FocusedSeq(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Enum(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::FlagsEnum(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Computed(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Rebuild(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Default(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Index(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Padded(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Aligned(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::FixedSized(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::NamedTuple(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::TimestampAdapter(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Array(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::ArrayExpr(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::GreedyRange(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::RepeatUntil(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Lazy(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::LazyStruct(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::LazyArray(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Rebuffered(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Bitwise(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Bytewise(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Pointer(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::PointerExpr(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Peek(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::RawCopy(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Prefixed(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Transformed(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Restreamed(n) => n.exec_build_py(py, input, stream, ctx),
        #[cfg(feature = "compression")]
        CompiledNode::Compressed(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Checksum(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::ByteSwapped(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::BitsSwapped(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::LazyBound(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::IfThenElse(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Switch(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Check(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::StopIf(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Hex(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::HexDump(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::Dynamic(n) => n.exec_build_py(py, input, stream, ctx),
        CompiledNode::External(n) => n.exec_build_py(py, input, stream, ctx),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combined::CombinedConstruct;
    use crate::compiled::py_sink::{ensure_test_python, PyDictSink2};
    use crate::compiler::SchemaCompiler;
    use crate::constructs::format_field::{INT16UB, INT8UB};
    use crate::constructs::lazy::LazyArray;
    use crate::constructs::repetition::Array;
    use crate::constructs::sequence::Sequence;
    use crate::constructs::struct_::Struct;
    use crate::core::stream::ByteStream;
    use pyo3::Python;
    use std::collections::HashMap;

    /// Helper: compile a schema, parse data via the Py dispatch path
    /// into a root Struct sink, and return the root PyObject.
    fn parse_via_py(cc: &CombinedConstruct, data: &[u8]) -> PyObject {
        ensure_test_python();
        Python::with_gil(|py| {
            let compiled = SchemaCompiler::new()
                .compile(cc)
                .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
            let node = compiled.tree();
            let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
            let mut ctx = Context::new();
            let mut sink = PyDictSink2::new_root_struct(py);
            exec_parse_py_dispatch(node, py, &mut stream, &mut ctx, &mut sink)
                .unwrap_or_else(|e| panic!("exec_parse_py failed: {e:?}"));
            Box::new(sink).into_root_py(py).unwrap()
        })
    }

    // ---- 1. Simple struct: two INT8UB fields --------------------------------

    #[test]
    fn py_parse_simple_struct() {
        let cc: CombinedConstruct = Struct::new()
            .field("x", Box::new(INT8UB.into()))
            .field("y", Box::new(INT8UB.into()))
            .into();
        let obj = parse_via_py(&cc, &[0x0A, 0x14]);
        Python::with_gil(|py| {
            let map: HashMap<String, i64> = obj.bind(py).extract().unwrap();
            assert_eq!(map["x"], 10);
            assert_eq!(map["y"], 20);
        });
    }

    // ---- 2. Nested struct ---------------------------------------------------

    #[test]
    fn py_parse_nested_struct() {
        let inner: CombinedConstruct = Struct::new().field("a", Box::new(INT8UB.into())).into();
        let outer: CombinedConstruct = Struct::new()
            .field("len", Box::new(INT8UB.into()))
            .field("data", Box::new(inner))
            .into();
        let obj = parse_via_py(&outer, &[0x01, 0xFF]);
        Python::with_gil(|py| {
            let map: HashMap<String, pyo3::PyObject> = obj.bind(py).extract().unwrap();
            assert_eq!(map["len"].bind(py).extract::<i64>().unwrap(), 1);
            let inner_map: HashMap<String, i64> = map["data"].bind(py).extract().unwrap();
            assert_eq!(inner_map["a"], 255);
        });
    }

    // ---- 3. Array of INT8UB (3 elements) ------------------------------------

    #[test]
    fn py_parse_array_via_struct() {
        let arr: CombinedConstruct = Array::new(3, Box::new(INT8UB.into())).into();
        let cc: CombinedConstruct = Struct::new().field("items", Box::new(arr)).into();
        let obj = parse_via_py(&cc, &[0x01, 0x02, 0x03]);
        Python::with_gil(|py| {
            let map: HashMap<String, pyo3::PyObject> = obj.bind(py).extract().unwrap();
            let list: Vec<i64> = map["items"].bind(py).extract().unwrap();
            assert_eq!(list, vec![1, 2, 3]);
        });
    }

    // ---- 3b. LazyArray boundary cases (B1 regression guard) -----------------
    //
    // `CompiledLazyArray::exec_parse_py` previously called `deposit_leaf`
    // inside its element loop. `deposit_leaf` only succeeds while the parent
    // sink is `Empty`, so:
    //   - count == 1 was silently demoted to a bare scalar (not a list);
    //   - count >= 2 errored on the second iteration (Leaf-mode rejection).
    // The fix uses `finish_item_py`, matching `CompiledArray`. These tests
    // pin the corrected behaviour for count == 0 / 1 / 3.

    /// Parses a top-level Array-family construct via the Py dispatch path into
    /// a root Array sink (mirrors `py_parse_sequence`'s setup). Used for
    /// LazyArray count == 0, which has no field wrapper.
    fn parse_via_py_root_array(cc: &CombinedConstruct, data: &[u8]) -> PyObject {
        ensure_test_python();
        Python::with_gil(|py| {
            let compiled = SchemaCompiler::new()
                .compile(cc)
                .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
            let node = compiled.tree();
            let mut stream = CombinedStream::ByteStream(ByteStream::new_read(data));
            let mut ctx = Context::new();
            let mut sink = PyDictSink2::new_root_array(py);
            exec_parse_py_dispatch(node, py, &mut stream, &mut ctx, &mut sink)
                .unwrap_or_else(|e| panic!("exec_parse_py failed: {e:?}"));
            Box::new(sink).into_root_py(py).unwrap()
        })
    }

    #[test]
    fn lazy_array_parse_count_0_returns_empty_list() {
        // count == 0: the loop body never runs, so the root Array sink stays
        // in Array mode and yields an empty list (no deposit_leaf misuse can
        // occur because the loop is never entered).
        let cc: CombinedConstruct = LazyArray::new(0, Box::new(INT8UB.into())).into();
        let obj = parse_via_py_root_array(&cc, &[]);
        Python::with_gil(|py| {
            let list: Vec<i64> = obj.bind(py).extract().unwrap();
            assert!(
                list.is_empty(),
                "count==0 must yield empty list, got {list:?}"
            );
        });
    }

    #[test]
    fn lazy_array_parse_count_1_returns_single_element_list() {
        // count == 1: must yield a single-element LIST, not a bare scalar.
        // Pre-fix, `deposit_leaf` demoted this to a scalar int — extracting
        // as `Vec<i64>` would then fail. Post-fix it is a proper list.
        let arr: CombinedConstruct = LazyArray::new(1, Box::new(INT8UB.into())).into();
        let cc: CombinedConstruct = Struct::new().field("items", Box::new(arr)).into();
        let obj = parse_via_py(&cc, &[0x07]);
        Python::with_gil(|py| {
            let map: HashMap<String, pyo3::PyObject> = obj.bind(py).extract().unwrap();
            let list: Vec<i64> = map["items"].bind(py).extract().unwrap();
            assert_eq!(list, vec![7]);
        });
    }

    #[test]
    fn lazy_array_parse_count_3_returns_three_element_list() {
        // count == 3: pre-fix, the second `deposit_leaf` call errored
        // because the sink had already transitioned to Leaf mode on the
        // first iteration. Post-fix, all three items append into the list.
        let arr: CombinedConstruct = LazyArray::new(3, Box::new(INT8UB.into())).into();
        let cc: CombinedConstruct = Struct::new().field("items", Box::new(arr)).into();
        let obj = parse_via_py(&cc, &[0x01, 0x02, 0x03]);
        Python::with_gil(|py| {
            let map: HashMap<String, pyo3::PyObject> = obj.bind(py).extract().unwrap();
            let list: Vec<i64> = map["items"].bind(py).extract().unwrap();
            assert_eq!(list, vec![1, 2, 3]);
        });
    }

    // ---- 4. Sequence (flat list output) -------------------------------------

    #[test]
    fn py_parse_sequence() {
        let cc: CombinedConstruct = Sequence::new()
            .push(Box::new(INT8UB.into()))
            .push(Box::new(INT16UB.into()))
            .into();
        ensure_test_python();
        Python::with_gil(|py| {
            let compiled = SchemaCompiler::new()
                .compile(&cc)
                .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
            let node = compiled.tree();
            let mut stream = CombinedStream::ByteStream(ByteStream::new_read(&[0x0A, 0x00, 0x14]));
            let mut ctx = Context::new();
            let mut sink = PyDictSink2::new_root_array(py);
            exec_parse_py_dispatch(node, py, &mut stream, &mut ctx, &mut sink)
                .unwrap_or_else(|e| panic!("exec_parse_py failed: {e:?}"));
            let obj = Box::new(sink).into_root_py(py).unwrap();
            let list: Vec<i64> = obj.bind(py).extract().unwrap();
            assert_eq!(list, vec![10, 20]);
        });
    }

    // ---- 5. exec_build_py produces correct bytes ---------------------------

    #[test]
    fn py_build_produces_correct_bytes() {
        let cc: CombinedConstruct = INT8UB.into();
        ensure_test_python();
        Python::with_gil(|py| {
            let compiled = SchemaCompiler::new()
                .compile(&cc)
                .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
            let node = compiled.tree();
            let mut stream = CombinedStream::ByteStream(ByteStream::new_write());
            let mut ctx = Context::new();
            let input = crate::compiled::py_input::PyScalarInput::new(Value::UInt(42));
            exec_build_py_dispatch(node, py, &input, &mut stream, &mut ctx).unwrap();
            let result = stream.into_bytes();
            assert_eq!(result, vec![42]);
        });
    }
}
