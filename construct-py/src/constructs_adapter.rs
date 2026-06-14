//! PyO3 wrappers for adapter, control-flow, enum, and hex constructs.
//!
//! This module provides `#[pyclass]` wrappers and `#[pyfunction]` factories
//! for the Rust-backed adapter and control-flow constructs, plus functional
//! factory functions (OneOf, NoneOf, Filter, Slicing, Indexing).
//!
//! # Clone strategy (PM decision §11.1 — 方案 A)
//!
//! Each wrapper that holds closures stores the original Python callable
//! objects (`PyObject`) alongside the inner Rust construct. When
//! [`extract_subcon`] needs an owned `Box<dyn Construct>`, the wrapper's
//! `make_owned` method re-bridges the closures from the stored PyObjects.

use construct::core::Construct;
use construct::value::Value;
use indexmap::IndexMap;

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use crate::construct_macros::PyConstructWrapper;
use crate::conversions::py_to_value;
use crate::expr_bridge::{
    py_param_to_cond_func, py_to_adapter_check_func, py_to_check_func, py_to_decode_func,
    py_to_encode_func, py_to_key_func, py_to_symmetric_func,
};
use crate::py_adapter::extract_subcon;

// ===========================================================================
// Helper: slice list operations (for Slicing / Indexing)
// ===========================================================================

/// Applies Python slice semantics `[start:stop:step]` to a list slice.
fn slice_list(
    list: &[Value],
    start: Option<usize>,
    stop: Option<usize>,
    step: usize,
) -> Vec<Value> {
    let len = list.len();
    let s = start.unwrap_or(0).min(len);
    let e = stop.unwrap_or(len).min(len);
    let st = step.max(1);
    let mut result = Vec::new();
    let mut i = s;
    while i < e {
        result.push(list[i].clone());
        i += st;
    }
    result
}

/// Places `input` elements into `output` at `[start:stop:step]` positions.
fn place_into_slice(
    output: &mut [Value],
    input: &[Value],
    start: Option<usize>,
    stop: Option<usize>,
    step: usize,
) {
    let st = step.max(1);
    let s = start.unwrap_or(0);
    let e = stop.unwrap_or(output.len()).min(output.len());
    let mut input_idx = 0;
    let mut i = s;
    while i < e && input_idx < input.len() {
        output[i] = input[input_idx].clone();
        i += st;
        input_idx += 1;
    }
}

// ===========================================================================
// Control flow: IfThenElse / If / Optional
// ===========================================================================

/// PyO3 wrapper: conditional construct (binary branch).
///
/// Corresponds to Python `IfThenElse(condfunc, then_subcon, else_subcon)`.
#[pyclass(name = "IfThenElse", unsendable)]
pub struct PyIfThenElse {
    /// The Rust IfThenElse construct.
    pub(crate) inner: construct::constructs::control_flow::IfThenElse,
    /// Original condition callable (for clone/re-bridge).
    pub(crate) cond_obj: PyObject,
    /// Original then-subcon Python object.
    pub(crate) then_obj: PyObject,
    /// Original else-subcon Python object.
    pub(crate) else_obj: PyObject,
}

impl PyConstructWrapper for PyIfThenElse {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyIfThenElse {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let cond = py_param_to_cond_func(py, self.cond_obj.bind(py))?;
            let then_sc = extract_subcon(self.then_obj.bind(py))?;
            let else_sc = extract_subcon(self.else_obj.bind(py))?;
            let con: Box<dyn Construct> = Box::new(
                construct::constructs::control_flow::IfThenElse::new(cond, then_sc, else_sc),
            );
            Ok(con)
        })
    }
}

/// Factory: `IfThenElse(condfunc, then_subcon, else_subcon)`.
#[pyfunction]
#[pyo3(name = "IfThenElse")]
pub fn py_if_then_else(
    py: Python<'_>,
    condfunc: &Bound<PyAny>,
    then_subcon: &Bound<PyAny>,
    else_subcon: &Bound<PyAny>,
) -> PyResult<PyIfThenElse> {
    let cond = py_param_to_cond_func(py, condfunc)?;
    let then_sc = extract_subcon(then_subcon)?;
    let else_sc = extract_subcon(else_subcon)?;
    Ok(PyIfThenElse {
        inner: construct::constructs::control_flow::IfThenElse::new(cond, then_sc, else_sc),
        cond_obj: condfunc.clone().unbind(),
        then_obj: then_subcon.clone().unbind(),
        else_obj: else_subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyIfThenElse);
crate::impl_construct_operators!(PyIfThenElse);

/// Factory: `If(condfunc, subcon)` — shorthand for IfThenElse(condfunc, subcon, Pass).
#[pyfunction]
#[pyo3(name = "If")]
pub fn py_if(
    py: Python<'_>,
    condfunc: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyIfThenElse> {
    let cond = py_param_to_cond_func(py, condfunc)?;
    let sc = extract_subcon(subcon)?;
    let pass = Box::new(construct::constructs::meta::Pass::new());
    Ok(PyIfThenElse {
        inner: construct::constructs::control_flow::IfThenElse::new(cond, sc, pass),
        cond_obj: condfunc.clone().unbind(),
        then_obj: subcon.clone().unbind(),
        else_obj: py
            .get_type_bound::<crate::constructs_atomic::PyPass>()
            .into_any()
            .unbind(),
    })
}

/// Factory: `Optional(subcon)` — conditional based on whether the value is None.
///
/// When the current value (`_` in context) is not None, parses/builds `subcon`;
/// otherwise returns None and produces empty bytes (Pass).
#[pyfunction]
#[pyo3(name = "Optional")]
pub fn py_optional(py: Python<'_>, subcon: &Bound<PyAny>) -> PyResult<PyIfThenElse> {
    use construct::core::context::Context;
    let sc = extract_subcon(subcon)?;
    let pass = Box::new(construct::constructs::meta::Pass::new());
    let cond: Box<dyn Fn(&Context) -> bool> = Box::new(|ctx: &Context| {
        // During build, the value being built is stored as "_" in context.
        // During parse, "_" is the parsed value from the preceding field.
        // Optional should activate (use subcon) when the value is not None.
        match ctx.get("_") {
            Some(v) => !matches!(v, Value::None),
            None => match ctx.get("_embedding") {
                Some(v) => v.as_bool().unwrap_or(false),
                None => false,
            },
        }
    });
    Ok(PyIfThenElse {
        inner: construct::constructs::control_flow::IfThenElse::new(cond, sc, pass),
        cond_obj: py
            .eval_bound("lambda ctx: ctx.get('_') is not None", None, None)?
            .unbind(),
        then_obj: subcon.clone().unbind(),
        else_obj: py
            .get_type_bound::<crate::constructs_atomic::PyPass>()
            .into_any()
            .unbind(),
    })
}

// ===========================================================================
// Control flow: Switch
// ===========================================================================

/// PyO3 wrapper: multi-branch conditional construct.
///
/// Corresponds to Python `Switch(keyfunc, cases, default=None)`.
#[pyclass(name = "Switch", unsendable)]
pub struct PySwitch {
    /// The Rust Switch construct.
    pub(crate) inner: construct::constructs::control_flow::Switch,
    /// Original keyfunc callable.
    pub(crate) keyfunc_obj: PyObject,
    /// Original cases as (Value key, Python construct object) pairs.
    pub(crate) cases_objs: Vec<(Value, PyObject)>,
    /// Original default subcon Python object (if any).
    pub(crate) default_obj: Option<PyObject>,
}

impl PyConstructWrapper for PySwitch {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PySwitch {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let keyf = py_to_key_func(self.keyfunc_obj.clone_ref(py));
            let mut cases_vec = Vec::new();
            for (key, obj) in &self.cases_objs {
                let sc = extract_subcon(obj.bind(py))?;
                cases_vec.push((key.clone(), sc));
            }
            let default_sc = match &self.default_obj {
                Some(d) => Some(extract_subcon(d.bind(py))?),
                None => None,
            };
            let con: Box<dyn Construct> = Box::new(
                construct::constructs::control_flow::Switch::new(keyf, cases_vec, default_sc),
            );
            Ok(con)
        })
    }
}

/// Factory: `Switch(keyfunc, cases, default=None)`.
#[pyfunction]
#[pyo3(name = "Switch", signature = (keyfunc, cases, default=None))]
pub fn py_switch(
    py: Python<'_>,
    keyfunc: &Bound<PyAny>,
    cases: &Bound<PyDict>,
    default: Option<&Bound<PyAny>>,
) -> PyResult<PySwitch> {
    let keyf = py_to_key_func(keyfunc.clone().unbind());
    let mut cases_vec = Vec::new();
    let mut cases_objs = Vec::new();
    for (key, val) in cases.iter() {
        let key_val = py_to_value(py, &key)?;
        let sc = extract_subcon(&val)?;
        cases_vec.push((key_val.clone(), sc));
        cases_objs.push((key_val, val.clone().unbind()));
    }
    let (default_sc, default_obj) = match default {
        Some(d) => (Some(extract_subcon(d)?), Some(d.clone().unbind())),
        None => (None, None),
    };
    Ok(PySwitch {
        inner: construct::constructs::control_flow::Switch::new(keyf, cases_vec, default_sc),
        keyfunc_obj: keyfunc.clone().unbind(),
        cases_objs,
        default_obj,
    })
}

crate::impl_api_methods!(PySwitch);
crate::impl_construct_operators!(PySwitch);

// ===========================================================================
// Control flow: Check / StopIf
// ===========================================================================

/// PyO3 wrapper: context condition check (signal construct).
///
/// Corresponds to Python `Check(condfunc)`.
#[pyclass(name = "Check", unsendable)]
pub struct PyCheck {
    /// The Rust Check construct.
    pub(crate) inner: construct::constructs::control_flow::Check,
    /// Original check callable.
    pub(crate) check_obj: PyObject,
}

impl PyConstructWrapper for PyCheck {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyCheck {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let check = py_to_check_func(self.check_obj.clone_ref(py));
            let con: Box<dyn Construct> =
                Box::new(construct::constructs::control_flow::Check::new(check));
            Ok(con)
        })
    }
}

/// Factory: `Check(condfunc)`.
#[pyfunction]
#[pyo3(name = "Check")]
pub fn py_check(condfunc: &Bound<PyAny>) -> PyResult<PyCheck> {
    let check = py_to_check_func(condfunc.clone().unbind());
    Ok(PyCheck {
        inner: construct::constructs::control_flow::Check::new(check),
        check_obj: condfunc.clone().unbind(),
    })
}

crate::impl_api_methods!(PyCheck);
crate::impl_construct_operators!(PyCheck);

/// PyO3 wrapper: stop-field signal construct.
///
/// Corresponds to Python `StopIf(condfunc)`.
#[pyclass(name = "StopIf", unsendable)]
pub struct PyStopIf {
    /// The Rust StopIf construct.
    pub(crate) inner: construct::constructs::control_flow::StopIf,
    /// Original condition callable.
    pub(crate) cond_obj: PyObject,
}

impl PyConstructWrapper for PyStopIf {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyStopIf {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let cond = py_param_to_cond_func(py, self.cond_obj.bind(py))?;
            let con: Box<dyn Construct> =
                Box::new(construct::constructs::control_flow::StopIf::new(cond));
            Ok(con)
        })
    }
}

/// Factory: `StopIf(condfunc)`.
#[pyfunction]
#[pyo3(name = "StopIf")]
pub fn py_stop_if(condfunc: &Bound<PyAny>) -> PyResult<PyStopIf> {
    let py = condfunc.py();
    let cond = py_param_to_cond_func(py, condfunc)?;
    Ok(PyStopIf {
        inner: construct::constructs::control_flow::StopIf::new(cond),
        cond_obj: condfunc.clone().unbind(),
    })
}

crate::impl_api_methods!(PyStopIf);
crate::impl_construct_operators!(PyStopIf);

// ===========================================================================
// Enum / FlagsEnum / Mapping
// ===========================================================================

/// PyO3 wrapper: integer ↔ string enum mapping.
///
/// Corresponds to Python `Enum(subcon, **mapping)`.
#[pyclass(name = "Enum", unsendable)]
pub struct PyEnum {
    /// The Rust Enum construct.
    pub(crate) inner: construct::constructs::enum_::Enum,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyEnum {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyEnum {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let mapping = self.inner.mapping.clone();
            let con: Box<dyn Construct> =
                Box::new(construct::constructs::enum_::Enum::new(sc, mapping));
            Ok(con)
        })
    }
}

/// Factory: `Enum(subcon, **mapping)`.
#[pyfunction]
#[pyo3(name = "Enum", signature = (*args, **kw))]
pub fn py_enum(
    _py: Python<'_>,
    args: &Bound<PyTuple>,
    kw: Option<&Bound<PyDict>>,
) -> PyResult<PyEnum> {
    let args_vec: Vec<Bound<PyAny>> = args.iter().collect();
    if args_vec.len() != 1 {
        return Err(PyTypeError::new_err(
            "Enum requires exactly 1 positional arg (subcon)",
        ));
    }
    let sc = extract_subcon(&args_vec[0])?;
    let mut mapping = IndexMap::new();
    if let Some(kw_dict) = kw {
        for (key, val) in kw_dict.iter() {
            let label: String = key.extract()?;
            let int_val: u64 = val.extract()?;
            mapping.insert(label, int_val);
        }
    }
    Ok(PyEnum {
        inner: construct::constructs::enum_::Enum::new(sc, mapping),
        subcon_obj: args_vec[0].clone().unbind(),
    })
}

crate::impl_api_methods!(PyEnum);
crate::impl_construct_operators!(PyEnum);

/// PyO3 wrapper: bit-flag enum.
///
/// Corresponds to Python `FlagsEnum(subcon, **flags)`.
#[pyclass(name = "FlagsEnum", unsendable)]
pub struct PyFlagsEnum {
    /// The Rust FlagsEnum construct.
    pub(crate) inner: construct::constructs::enum_::FlagsEnum,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyFlagsEnum {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyFlagsEnum {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let flags = self.inner.flags.clone();
            let con: Box<dyn Construct> =
                Box::new(construct::constructs::enum_::FlagsEnum::new(sc, flags));
            Ok(con)
        })
    }
}

/// Factory: `FlagsEnum(subcon, **flags)`.
#[pyfunction]
#[pyo3(name = "FlagsEnum", signature = (*args, **kw))]
pub fn py_flags_enum(
    _py: Python<'_>,
    args: &Bound<PyTuple>,
    kw: Option<&Bound<PyDict>>,
) -> PyResult<PyFlagsEnum> {
    let args_vec: Vec<Bound<PyAny>> = args.iter().collect();
    if args_vec.len() != 1 {
        return Err(PyTypeError::new_err(
            "FlagsEnum requires exactly 1 positional arg (subcon)",
        ));
    }
    let sc = extract_subcon(&args_vec[0])?;
    let mut flags = IndexMap::new();
    if let Some(kw_dict) = kw {
        for (key, val) in kw_dict.iter() {
            let name: String = key.extract()?;
            let bit_val: u64 = val.extract()?;
            flags.insert(name, bit_val);
        }
    }
    Ok(PyFlagsEnum {
        inner: construct::constructs::enum_::FlagsEnum::new(sc, flags),
        subcon_obj: args_vec[0].clone().unbind(),
    })
}

crate::impl_api_methods!(PyFlagsEnum);
crate::impl_construct_operators!(PyFlagsEnum);

/// PyO3 wrapper: general-purpose bidirectional value mapping.
///
/// Corresponds to Python `Mapping(subcon, mapping)`.
#[pyclass(name = "Mapping", unsendable)]
pub struct PyMapping {
    /// The Rust Mapping construct.
    pub(crate) inner: construct::constructs::enum_::Mapping,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyMapping {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyMapping {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let pairs = self.inner.mapping.clone();
            let con: Box<dyn Construct> =
                Box::new(construct::constructs::enum_::Mapping::new(sc, pairs));
            Ok(con)
        })
    }
}

/// Factory: `Mapping(subcon, mapping)`.
///
/// The Python dict is `{decoded_value: raw_value}` — this is the
/// `encmapping` in the Python source (`Mapping.__init__` sets
/// `self.encmapping = mapping`). The dict key is the user-facing (decoded)
/// value and the dict value is the raw value read/written by the subcon.
///
/// The Rust `Mapping::new` expects `(build_key, build_value)` pairs where
/// `build_key` is the decoded value and `build_value` is the raw value, so
/// the pairs map directly without swapping.
#[pyfunction]
#[pyo3(name = "Mapping")]
pub fn py_mapping(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    mapping: &Bound<PyDict>,
) -> PyResult<PyMapping> {
    let sc = extract_subcon(subcon)?;
    let mut pairs = Vec::new();
    for (key, val) in mapping.iter() {
        // Python dict: {decoded_value: raw_value} (encmapping in Python source)
        // Rust Mapping: (build_key=decoded, build_value=raw)
        let decoded = py_to_value(py, &key)?;
        let raw = py_to_value(py, &val)?;
        pairs.push((decoded, raw));
    }
    Ok(PyMapping {
        inner: construct::constructs::enum_::Mapping::new(sc, pairs),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyMapping);
crate::impl_construct_operators!(PyMapping);

// ===========================================================================
// Hex / HexDump (pass-through display adapters)
// ===========================================================================

/// PyO3 wrapper: hex display adapter (pass-through).
///
/// Corresponds to Python `Hex(subcon)`. In Rust, Hex is a pass-through —
/// the parsed value is returned unchanged. Display semantics (hex-formatted
/// `str()`) are deferred to the Python side.
#[pyclass(name = "Hex", unsendable)]
pub struct PyHex {
    /// The Rust Hex construct.
    pub(crate) inner: construct::constructs::hex::Hex,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyHex {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyHex {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let con: Box<dyn Construct> = Box::new(construct::constructs::hex::Hex::new(sc));
            Ok(con)
        })
    }
}

/// Factory: `Hex(subcon)`.
#[pyfunction]
#[pyo3(name = "Hex")]
pub fn py_hex(subcon: &Bound<PyAny>) -> PyResult<PyHex> {
    let sc = extract_subcon(subcon)?;
    Ok(PyHex {
        inner: construct::constructs::hex::Hex::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyHex);
crate::impl_construct_operators!(PyHex);

/// PyO3 wrapper: hex dump display adapter (pass-through).
///
/// Corresponds to Python `HexDump(subcon)`.
#[pyclass(name = "HexDump", unsendable)]
pub struct PyHexDump {
    /// The Rust HexDump construct.
    pub(crate) inner: construct::constructs::hex::HexDump,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
}

impl PyConstructWrapper for PyHexDump {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyHexDump {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let con: Box<dyn Construct> = Box::new(construct::constructs::hex::HexDump::new(sc));
            Ok(con)
        })
    }
}

/// Factory: `HexDump(subcon)`.
#[pyfunction]
#[pyo3(name = "HexDump")]
pub fn py_hex_dump(subcon: &Bound<PyAny>) -> PyResult<PyHexDump> {
    let sc = extract_subcon(subcon)?;
    Ok(PyHexDump {
        inner: construct::constructs::hex::HexDump::new(sc),
        subcon_obj: subcon.clone().unbind(),
    })
}

crate::impl_api_methods!(PyHexDump);
crate::impl_construct_operators!(PyHexDump);

// ===========================================================================
// Adapter: ExprAdapter / SymmetricAdapter / ExprSymmetricAdapter
// ===========================================================================

/// PyO3 wrapper: bi-directional value adapter.
///
/// Corresponds to Python `ExprAdapter(subcon, decoder, encoder)`.
/// Accepts decoder/encoder lambdas with signature `lambda(obj, context)`.
#[pyclass(name = "ExprAdapter", unsendable)]
pub struct PyExprAdapter {
    /// The Rust Adapter construct.
    pub(crate) inner: construct::constructs::adapters::Adapter,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
    /// Original decoder callable.
    pub(crate) decoder_obj: PyObject,
    /// Original encoder callable.
    pub(crate) encoder_obj: PyObject,
}

impl PyConstructWrapper for PyExprAdapter {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyExprAdapter {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let decode = py_to_decode_func(self.decoder_obj.clone_ref(py));
            let encode = py_to_encode_func(self.encoder_obj.clone_ref(py));
            let con: Box<dyn Construct> = Box::new(construct::constructs::adapters::Adapter::new(
                sc, decode, encode,
            ));
            Ok(con)
        })
    }
}

/// Factory: `ExprAdapter(subcon, decoder, encoder)`.
#[pyfunction]
#[pyo3(name = "ExprAdapter")]
pub fn py_expr_adapter(
    subcon: &Bound<PyAny>,
    decoder: &Bound<PyAny>,
    encoder: &Bound<PyAny>,
) -> PyResult<PyExprAdapter> {
    let sc = extract_subcon(subcon)?;
    let decode = py_to_decode_func(decoder.clone().unbind());
    let encode = py_to_encode_func(encoder.clone().unbind());
    Ok(PyExprAdapter {
        inner: construct::constructs::adapters::Adapter::new(sc, decode, encode),
        subcon_obj: subcon.clone().unbind(),
        decoder_obj: decoder.clone().unbind(),
        encoder_obj: encoder.clone().unbind(),
    })
}

crate::impl_api_methods!(PyExprAdapter);
crate::impl_construct_operators!(PyExprAdapter);

/// PyO3 wrapper: symmetric adapter (decode == encode).
///
/// Corresponds to Python `SymmetricAdapter(subcon, func)`.
#[pyclass(name = "SymmetricAdapter", unsendable)]
pub struct PySymmetricAdapter {
    /// The Rust SymmetricAdapter construct.
    pub(crate) inner: construct::constructs::adapters::SymmetricAdapter,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
    /// Original symmetric func callable.
    pub(crate) func_obj: PyObject,
}

impl PyConstructWrapper for PySymmetricAdapter {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PySymmetricAdapter {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let func = py_to_symmetric_func(self.func_obj.clone_ref(py));
            let con: Box<dyn Construct> = Box::new(
                construct::constructs::adapters::SymmetricAdapter::new(sc, func),
            );
            Ok(con)
        })
    }
}

/// Factory: `SymmetricAdapter(subcon, func)` — wraps Rust generic SymmetricAdapter.
///
/// Exposed as `py_symmetric_adapter` (not `SymmetricAdapter`) to avoid collision
/// with the pure-Python `SymmetricAdapter` base class in `_adapter.py`. Users
/// typically use `ExprSymmetricAdapter` instead.
#[pyfunction]
pub fn py_symmetric_adapter(
    subcon: &Bound<PyAny>,
    func: &Bound<PyAny>,
) -> PyResult<PySymmetricAdapter> {
    let sc = extract_subcon(subcon)?;
    let symmetric = py_to_symmetric_func(func.clone().unbind());
    Ok(PySymmetricAdapter {
        inner: construct::constructs::adapters::SymmetricAdapter::new(sc, symmetric),
        subcon_obj: subcon.clone().unbind(),
        func_obj: func.clone().unbind(),
    })
}

crate::impl_api_methods!(PySymmetricAdapter);
crate::impl_construct_operators!(PySymmetricAdapter);

/// PyO3 wrapper: ExprSymmetricAdapter (semantic alias for SymmetricAdapter).
///
/// Corresponds to Python `ExprSymmetricAdapter(subcon, func)`. In Python this
/// is a subclass of ExprAdapter, but semantically equivalent to SymmetricAdapter.
#[pyclass(name = "ExprSymmetricAdapter", unsendable)]
pub struct PyExprSymmetricAdapter {
    /// The Rust SymmetricAdapter construct.
    pub(crate) inner: construct::constructs::adapters::SymmetricAdapter,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
    /// Original symmetric func callable.
    pub(crate) func_obj: PyObject,
}

impl PyConstructWrapper for PyExprSymmetricAdapter {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyExprSymmetricAdapter {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let func = py_to_symmetric_func(self.func_obj.clone_ref(py));
            let con: Box<dyn Construct> = Box::new(
                construct::constructs::adapters::SymmetricAdapter::new(sc, func),
            );
            Ok(con)
        })
    }
}

/// Factory: `ExprSymmetricAdapter(subcon, func)`.
#[pyfunction]
#[pyo3(name = "ExprSymmetricAdapter")]
pub fn py_expr_symmetric_adapter(
    subcon: &Bound<PyAny>,
    func: &Bound<PyAny>,
) -> PyResult<PyExprSymmetricAdapter> {
    let sc = extract_subcon(subcon)?;
    let symmetric = py_to_symmetric_func(func.clone().unbind());
    Ok(PyExprSymmetricAdapter {
        inner: construct::constructs::adapters::SymmetricAdapter::new(sc, symmetric),
        subcon_obj: subcon.clone().unbind(),
        func_obj: func.clone().unbind(),
    })
}

crate::impl_api_methods!(PyExprSymmetricAdapter);
crate::impl_construct_operators!(PyExprSymmetricAdapter);

// ===========================================================================
// Validator / ExprValidator
// ===========================================================================

/// PyO3 wrapper: value validator (pass-through on success).
///
/// Corresponds to Python `Validator(subcon, func)`. The func signature is
/// `func(obj, context) -> bool` (truthy = valid).
#[pyclass(name = "Validator", unsendable)]
pub struct PyValidator {
    /// The Rust Validator construct.
    pub(crate) inner: construct::constructs::adapters::Validator,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
    /// Original check callable.
    pub(crate) check_obj: PyObject,
}

impl PyConstructWrapper for PyValidator {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyValidator {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let check = py_to_adapter_check_func(self.check_obj.clone_ref(py));
            let con: Box<dyn Construct> =
                Box::new(construct::constructs::adapters::Validator::new(sc, check));
            Ok(con)
        })
    }
}

/// Factory: `Validator(subcon, func)` — wraps Rust generic Validator.
///
/// Exposed as `py_validator` (not `Validator`) to avoid collision with the
/// pure-Python `Validator` base class in `_adapter.py`. Users typically use
/// `ExprValidator` instead.
#[pyfunction]
pub fn py_validator(subcon: &Bound<PyAny>, func: &Bound<PyAny>) -> PyResult<PyValidator> {
    let sc = extract_subcon(subcon)?;
    let check = py_to_adapter_check_func(func.clone().unbind());
    Ok(PyValidator {
        inner: construct::constructs::adapters::Validator::new(sc, check),
        subcon_obj: subcon.clone().unbind(),
        check_obj: func.clone().unbind(),
    })
}

crate::impl_api_methods!(PyValidator);
crate::impl_construct_operators!(PyValidator);

/// PyO3 wrapper: expression validator (semantic alias for Validator).
///
/// Corresponds to Python `ExprValidator(subcon, validator)`.
#[pyclass(name = "ExprValidator", unsendable)]
pub struct PyExprValidator {
    /// The Rust ExprValidator construct.
    pub(crate) inner: construct::constructs::adapters::ExprValidator,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
    /// Original check callable.
    pub(crate) check_obj: PyObject,
}

impl PyConstructWrapper for PyExprValidator {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyExprValidator {
    /// Reconstructs an owned `Box<dyn Construct>` from stored Python objects.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let check = py_to_adapter_check_func(self.check_obj.clone_ref(py));
            let con: Box<dyn Construct> = Box::new(
                construct::constructs::adapters::ExprValidator::new(sc, check),
            );
            Ok(con)
        })
    }
}

/// Factory: `ExprValidator(subcon, validator)`.
#[pyfunction]
#[pyo3(name = "ExprValidator")]
pub fn py_expr_validator(
    subcon: &Bound<PyAny>,
    validator: &Bound<PyAny>,
) -> PyResult<PyExprValidator> {
    let sc = extract_subcon(subcon)?;
    let check = py_to_adapter_check_func(validator.clone().unbind());
    Ok(PyExprValidator {
        inner: construct::constructs::adapters::ExprValidator::new(sc, check),
        subcon_obj: subcon.clone().unbind(),
        check_obj: validator.clone().unbind(),
    })
}

crate::impl_api_methods!(PyExprValidator);
crate::impl_construct_operators!(PyExprValidator);

// ===========================================================================
// Functional factories: OneOf / NoneOf / Filter
// ===========================================================================

/// Creates a Python lambda in a persistent globals namespace.
///
/// The `expr` is evaluated with `bindings` as globals, so captured variables
/// stay alive via the function's `__globals__` attribute.
fn make_py_lambda(py: Python<'_>, expr: &str, bindings: &[(&str, PyObject)]) -> PyResult<PyObject> {
    let globals = PyDict::new_bound(py);
    for (name, obj) in bindings {
        globals.set_item(*name, obj.bind(py))?;
    }
    Ok(py.eval_bound(expr, Some(&globals), None)?.unbind())
}

/// Factory: `OneOf(subcon, valids)` → ExprValidator.
///
/// `valids` is any Python container (list/set/frozenset). The check function
/// uses Python's `in` operator via a synthesized lambda.
#[pyfunction]
#[pyo3(name = "OneOf")]
pub fn py_one_of(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    valids: &Bound<PyAny>,
) -> PyResult<PyExprValidator> {
    let sc = extract_subcon(subcon)?;
    let valids_obj = valids.clone().unbind();
    let check_obj = make_py_lambda(
        py,
        "lambda obj, ctx: obj in __v__",
        &[("__v__", valids_obj)],
    )?;
    let check = py_to_adapter_check_func(check_obj.clone_ref(py));
    Ok(PyExprValidator {
        inner: construct::constructs::adapters::ExprValidator::new(sc, check),
        subcon_obj: subcon.clone().unbind(),
        check_obj,
    })
}

/// Factory: `NoneOf(subcon, invalids)` → ExprValidator (negated OneOf).
#[pyfunction]
#[pyo3(name = "NoneOf")]
pub fn py_none_of(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    invalids: &Bound<PyAny>,
) -> PyResult<PyExprValidator> {
    let sc = extract_subcon(subcon)?;
    let invalids_obj = invalids.clone().unbind();
    let check_obj = make_py_lambda(
        py,
        "lambda obj, ctx: obj not in __v__",
        &[("__v__", invalids_obj)],
    )?;
    let check = py_to_adapter_check_func(check_obj.clone_ref(py));
    Ok(PyExprValidator {
        inner: construct::constructs::adapters::ExprValidator::new(sc, check),
        subcon_obj: subcon.clone().unbind(),
        check_obj,
    })
}

/// Factory: `Filter(predicate, subcon)` → ExprSymmetricAdapter.
///
/// `predicate` signature: `predicate(element, context) -> bool`.
/// The symmetric function applies a list comprehension filtering elements.
#[pyfunction]
#[pyo3(name = "Filter")]
pub fn py_filter(
    py: Python<'_>,
    predicate: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
) -> PyResult<PyExprSymmetricAdapter> {
    let sc = extract_subcon(subcon)?;
    let pred_obj = predicate.clone().unbind();
    let func_obj = make_py_lambda(
        py,
        "lambda obj, ctx: [x for x in obj if __p__(x, ctx)]",
        &[("__p__", pred_obj)],
    )?;
    let func = py_to_symmetric_func(func_obj.clone_ref(py));
    Ok(PyExprSymmetricAdapter {
        inner: construct::constructs::adapters::SymmetricAdapter::new(sc, func),
        subcon_obj: subcon.clone().unbind(),
        func_obj,
    })
}

// ===========================================================================
// Functional factories: Slicing / Indexing
// ===========================================================================

/// PyO3 wrapper: list slicing adapter.
///
/// Corresponds to Python `Slicing(subcon, count, start, stop, step, empty)`.
/// Uses Rust closures for decode/encode — stored config parameters allow
/// reconstruction.
#[pyclass(name = "Slicing", unsendable)]
pub struct PySlicing {
    /// The Rust Adapter construct (with slice closures).
    pub(crate) inner: construct::constructs::adapters::Adapter,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
    /// Config: total element count.
    pub(crate) count: usize,
    /// Config: slice start.
    pub(crate) start: Option<usize>,
    /// Config: slice stop.
    pub(crate) stop: Option<usize>,
    /// Config: slice step.
    pub(crate) step: usize,
    /// Config: fill value for empty positions.
    pub(crate) empty_val: Value,
}

impl PyConstructWrapper for PySlicing {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PySlicing {
    /// Builds the decode closure from stored config.
    fn make_decode(
        &self,
    ) -> Box<
        dyn Fn(&Value, &construct::core::context::Context) -> construct::core::error::Result<Value>,
    > {
        let (start, stop, step) = (self.start, self.stop, self.step);
        Box::new(move |obj, _ctx| {
            let list =
                obj.as_list()
                    .map_err(|e| construct::core::error::ConstructError::Generic {
                        path: e.path().to_string(),
                        message: format!("Slicing decode: {e}"),
                    })?;
            Ok(Value::List(slice_list(list, start, stop, step)))
        })
    }

    /// Builds the encode closure from stored config.
    fn make_encode(
        &self,
    ) -> Box<
        dyn Fn(&Value, &construct::core::context::Context) -> construct::core::error::Result<Value>,
    > {
        let (count, start, stop, step) = (self.count, self.start, self.stop, self.step);
        let empty = self.empty_val.clone();
        Box::new(move |obj, _ctx| {
            let input =
                obj.as_list()
                    .map_err(|e| construct::core::error::ConstructError::Generic {
                        path: e.path().to_string(),
                        message: format!("Slicing encode: {e}"),
                    })?;
            let mut output = vec![empty.clone(); count];
            place_into_slice(&mut output, input, start, stop, step);
            Ok(Value::List(output))
        })
    }

    /// Reconstructs an owned `Box<dyn Construct>` from stored data.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let decode = self.make_decode();
            let encode = self.make_encode();
            let con: Box<dyn Construct> = Box::new(construct::constructs::adapters::Adapter::new(
                sc, decode, encode,
            ));
            Ok(con)
        })
    }
}

/// Factory: `Slicing(subcon, count, start, stop, step=1, empty=None)`.
#[pyfunction]
#[pyo3(name = "Slicing", signature = (subcon, count, start, stop, step=1, empty=None))]
pub fn py_slicing(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    count: usize,
    start: Option<usize>,
    stop: Option<usize>,
    step: usize,
    empty: Option<&Bound<PyAny>>,
) -> PyResult<PySlicing> {
    let sc = extract_subcon(subcon)?;
    let empty_val = match empty {
        Some(e) => py_to_value(py, e)?,
        None => Value::None,
    };
    let mut wrapper = PySlicing {
        inner: construct::constructs::adapters::Adapter::new(
            sc,
            Box::new(|_, _| Ok(Value::None)),
            Box::new(|v, _| Ok(v.clone())),
        ),
        subcon_obj: subcon.clone().unbind(),
        count,
        start,
        stop,
        step,
        empty_val: empty_val.clone(),
    };
    // Replace closures with real ones.
    let decode = wrapper.make_decode();
    let encode = wrapper.make_encode();
    let sc2 = extract_subcon(subcon)?;
    wrapper.inner = construct::constructs::adapters::Adapter::new(sc2, decode, encode);
    Ok(wrapper)
}

crate::impl_api_methods!(PySlicing);
crate::impl_construct_operators!(PySlicing);

/// PyO3 wrapper: list indexing adapter.
///
/// Corresponds to Python `Indexing(subcon, count, index, empty)`.
#[pyclass(name = "Indexing", unsendable)]
pub struct PyIndexing {
    /// The Rust Adapter construct (with index closures).
    pub(crate) inner: construct::constructs::adapters::Adapter,
    /// Original subcon Python object.
    pub(crate) subcon_obj: PyObject,
    /// Config: total element count.
    pub(crate) count: usize,
    /// Config: index to extract/insert.
    pub(crate) index: usize,
    /// Config: fill value for empty positions.
    pub(crate) empty_val: Value,
}

impl PyConstructWrapper for PyIndexing {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

impl PyIndexing {
    /// Builds the decode closure: list → list[index].
    fn make_decode(
        &self,
    ) -> Box<
        dyn Fn(&Value, &construct::core::context::Context) -> construct::core::error::Result<Value>,
    > {
        let index = self.index;
        Box::new(move |obj, _ctx| {
            let list =
                obj.as_list()
                    .map_err(|e| construct::core::error::ConstructError::Generic {
                        path: e.path().to_string(),
                        message: format!("Indexing decode: {e}"),
                    })?;
            list.get(index).cloned().ok_or_else(|| {
                construct::core::error::ConstructError::Generic {
                    path: String::new(),
                    message: format!("Indexing: index {index} out of bounds (len {})", list.len()),
                }
            })
        })
    }

    /// Builds the encode closure: value → list[count] with value at index.
    fn make_encode(
        &self,
    ) -> Box<
        dyn Fn(&Value, &construct::core::context::Context) -> construct::core::error::Result<Value>,
    > {
        let (count, index) = (self.count, self.index);
        let empty = self.empty_val.clone();
        Box::new(move |obj, _ctx| {
            let mut output = vec![empty.clone(); count];
            if index < count {
                output[index] = obj.clone();
            }
            Ok(Value::List(output))
        })
    }

    /// Reconstructs an owned `Box<dyn Construct>` from stored data.
    pub(crate) fn make_owned(&self) -> PyResult<Box<dyn Construct>> {
        Python::with_gil(|py| {
            let sc = extract_subcon(self.subcon_obj.bind(py))?;
            let decode = self.make_decode();
            let encode = self.make_encode();
            let con: Box<dyn Construct> = Box::new(construct::constructs::adapters::Adapter::new(
                sc, decode, encode,
            ));
            Ok(con)
        })
    }
}

/// Factory: `Indexing(subcon, count, index, empty=None)`.
#[pyfunction]
#[pyo3(name = "Indexing", signature = (subcon, count, index, empty=None))]
pub fn py_indexing(
    py: Python<'_>,
    subcon: &Bound<PyAny>,
    count: usize,
    index: usize,
    empty: Option<&Bound<PyAny>>,
) -> PyResult<PyIndexing> {
    let sc = extract_subcon(subcon)?;
    let empty_val = match empty {
        Some(e) => py_to_value(py, e)?,
        None => Value::None,
    };
    let mut wrapper = PyIndexing {
        inner: construct::constructs::adapters::Adapter::new(
            sc,
            Box::new(|_, _| Ok(Value::None)),
            Box::new(|v, _| Ok(v.clone())),
        ),
        subcon_obj: subcon.clone().unbind(),
        count,
        index,
        empty_val: empty_val.clone(),
    };
    let decode = wrapper.make_decode();
    let encode = wrapper.make_encode();
    let sc2 = extract_subcon(subcon)?;
    wrapper.inner = construct::constructs::adapters::Adapter::new(sc2, decode, encode);
    Ok(wrapper)
}

crate::impl_api_methods!(PyIndexing);
crate::impl_construct_operators!(PyIndexing);

// ===========================================================================
// try_extract_registered: type-specific extraction for all 10.7 wrappers
// ===========================================================================

/// Tries to extract an owned `Box<dyn Construct>` from a Python object by
/// checking against all registered 10.7 `#[pyclass]` wrapper types.
///
/// Returns `Ok(Some(...))` if a match was found, `Ok(None)` if no registered
/// type matched (caller should fall back to duck typing).
///
/// This function is called by [`extract_subcon`] before the duck-typing
/// fallback, enabling direct Rust-to-Rust delegation without Python
/// round-trips for all 10.7 adapter/control-flow/enum/hex wrappers.
pub fn try_extract_registered(obj: &Bound<'_, PyAny>) -> PyResult<Option<Box<dyn Construct>>> {
    // Helper macro to avoid repetitive boilerplate.
    macro_rules! try_type {
        ($obj:expr, $wrapper:ty) => {
            if let Ok(w) = $obj.extract::<PyRef<$wrapper>>() {
                return Ok(Some(w.make_owned()?));
            }
        };
    }

    // Control flow
    try_type!(obj, PyIfThenElse);
    try_type!(obj, PySwitch);
    try_type!(obj, PyCheck);
    try_type!(obj, PyStopIf);

    // Enum / Mapping
    try_type!(obj, PyEnum);
    try_type!(obj, PyFlagsEnum);
    try_type!(obj, PyMapping);

    // Hex
    try_type!(obj, PyHex);
    try_type!(obj, PyHexDump);

    // Adapters
    try_type!(obj, PyExprAdapter);
    try_type!(obj, PySymmetricAdapter);
    try_type!(obj, PyExprSymmetricAdapter);
    try_type!(obj, PyValidator);
    try_type!(obj, PyExprValidator);

    // Functional (Rust-closure-backed)
    try_type!(obj, PySlicing);
    try_type!(obj, PyIndexing);

    Ok(None)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use construct::core::context::Context;
    use construct::core::stream::ByteStream;

    /// Helper: create a Python Byte (Int8ub) pyclass instance for tests.
    fn get_byte(py: Python<'_>) -> Py<PyAny> {
        let ff = crate::constructs_atomic::py_format_field(">", "B").unwrap();
        Py::new(py, ff).unwrap().into_any()
    }

    #[test]
    fn expr_adapter_parse() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let decoder = py
                .eval_bound("lambda obj, ctx: obj + 1", None, None)
                .unwrap();
            let encoder = py
                .eval_bound("lambda obj, ctx: obj - 1", None, None)
                .unwrap();
            let adapter = py_expr_adapter(byte.bind(py), &decoder, &encoder).unwrap();
            let mut stream = ByteStream::new_read(&[4]);
            let mut ctx = Context::new();
            let result = adapter.inner.parse(&mut stream, &mut ctx).unwrap();
            assert_eq!(result, Value::Int(5));
        });
    }

    #[test]
    fn expr_validator_rejects() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let validator = py
                .eval_bound("lambda obj, ctx: obj != 0", None, None)
                .unwrap();
            let v = py_expr_validator(byte.bind(py), &validator).unwrap();
            let mut stream = ByteStream::new_read(&[0]);
            let mut ctx = Context::new();
            let err = v.inner.parse(&mut stream, &mut ctx).unwrap_err();
            assert!(matches!(
                err,
                construct::core::error::ConstructError::Validation { .. }
            ));
        });
    }

    #[test]
    fn hex_passthrough() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let h = py_hex(byte.bind(py)).unwrap();
            let mut stream = ByteStream::new_read(&[42]);
            let mut ctx = Context::new();
            let result = h.inner.parse(&mut stream, &mut ctx).unwrap();
            assert_eq!(result, Value::Int(42));
        });
    }

    #[test]
    fn slice_list_basic() {
        let list = vec![
            Value::UInt(1),
            Value::UInt(2),
            Value::UInt(3),
            Value::UInt(4),
        ];
        let result = slice_list(&list, Some(1), Some(3), 1);
        assert_eq!(result, vec![Value::UInt(2), Value::UInt(3)]);
    }

    #[test]
    fn slice_list_step() {
        let list = vec![
            Value::UInt(1),
            Value::UInt(2),
            Value::UInt(3),
            Value::UInt(4),
            Value::UInt(5),
        ];
        let result = slice_list(&list, None, None, 2);
        assert_eq!(result, vec![Value::UInt(1), Value::UInt(3), Value::UInt(5)]);
    }

    #[test]
    fn place_into_slice_basic() {
        let mut output = vec![Value::None, Value::None, Value::None, Value::None];
        let input = vec![Value::UInt(2), Value::UInt(3)];
        place_into_slice(&mut output, &input, Some(1), Some(3), 1);
        assert_eq!(
            output,
            vec![Value::None, Value::UInt(2), Value::UInt(3), Value::None]
        );
    }

    // -- Enum / Mapping direction verification ---------------------------------

    /// Helper: build a Python dict `{key: value, ...}` from parallel slices.
    fn make_py_dict<'py>(py: Python<'py>, pairs: &[(&str, i64)]) -> Bound<'py, PyDict> {
        let dict = pyo3::types::PyDict::new_bound(py);
        for &(k, v) in pairs {
            dict.set_item(k, v).unwrap();
        }
        dict
    }

    /// Verifies `Enum(Byte, one=1, two=2)` mapping direction:
    /// parse raw 1 → "one", build "two" → raw 2.
    #[test]
    fn enum_mapping_direction() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let kw = make_py_dict(py, &[("one", 1), ("two", 2)]);
            // Positional args tuple containing the subcon.
            let args = PyTuple::new_bound(py, [byte.bind(py).as_ref()]);
            let e = py_enum(py, &args, Some(&kw)).unwrap();

            // Parse raw 1 → "one"
            let mut stream = ByteStream::new_read(&[1]);
            let mut ctx = Context::new();
            assert_eq!(
                e.inner.parse(&mut stream, &mut ctx).unwrap(),
                Value::String("one".to_string())
            );

            // Build "two" → raw 2
            let mut out = ByteStream::new_write();
            e.inner
                .build(&Value::String("two".to_string()), &mut out, &mut ctx)
                .unwrap();
            assert_eq!(out.into_bytes(), vec![2]);
        });
    }

    /// Verifies `Mapping(Byte, {"A": 0, "B": 1})` mapping direction.
    ///
    /// Python dict `{decoded: raw}` — the dict key is the user-facing decoded
    /// value and the dict value is the raw subcon value:
    ///   - parse raw 0 → decoded "A"
    ///   - build "B" → raw 1
    ///
    /// This mirrors Python original `Mapping.__init__` where
    /// `self.encmapping = mapping` (decoded→raw) and
    /// `self.decmapping = {v:k}` (raw→decoded).
    #[test]
    fn mapping_direction_parse_and_build() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            // Python dict: {"A": 0, "B": 1}  →  decoded "A" maps to raw 0
            let dict = make_py_dict(py, &[("A", 0), ("B", 1)]);
            let m = py_mapping(py, byte.bind(py), &dict).unwrap();

            // Parse raw 0 → decoded "A"
            let mut stream = ByteStream::new_read(&[0]);
            let mut ctx = Context::new();
            assert_eq!(
                m.inner.parse(&mut stream, &mut ctx).unwrap(),
                Value::String("A".to_string())
            );

            // Parse raw 1 → decoded "B"
            let mut stream2 = ByteStream::new_read(&[1]);
            assert_eq!(
                m.inner.parse(&mut stream2, &mut ctx).unwrap(),
                Value::String("B".to_string())
            );

            // Build "B" → raw 1
            let mut out = ByteStream::new_write();
            m.inner
                .build(&Value::String("B".to_string()), &mut out, &mut ctx)
                .unwrap();
            assert_eq!(out.into_bytes(), vec![1]);

            // Build "A" → raw 0
            let mut out2 = ByteStream::new_write();
            m.inner
                .build(&Value::String("A".to_string()), &mut out2, &mut ctx)
                .unwrap();
            assert_eq!(out2.into_bytes(), vec![0]);
        });
    }

    /// Verifies `Mapping` roundtrip: build "B" → parse → "B".
    #[test]
    fn mapping_roundtrip() {
        crate::ensure_python();
        Python::with_gil(|py| {
            let byte = get_byte(py);
            let dict = make_py_dict(py, &[("A", 0), ("B", 1)]);
            let m = py_mapping(py, byte.bind(py), &dict).unwrap();

            let mut out = ByteStream::new_write();
            let mut ctx = Context::new();
            m.inner
                .build(&Value::String("B".to_string()), &mut out, &mut ctx)
                .unwrap();
            let bytes = out.into_bytes();

            let mut stream = ByteStream::new_read(&bytes);
            assert_eq!(
                m.inner.parse(&mut stream, &mut ctx).unwrap(),
                Value::String("B".to_string())
            );
        });
    }
}
