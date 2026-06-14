//! PyO3 wrappers for composite constructors (Struct, Sequence, Array, etc.).

use construct::core::Construct;

use pyo3::exceptions::PyAttributeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PySlice, PyTuple};

use crate::construct_macros::PyConstructWrapper;
use crate::exceptions::ConstructError;
use crate::expr_bridge::{py_param_to_evaluate, py_to_repeat_predicate};
use crate::py_adapter::extract_subcon;
use crate::py_renamed::{apply_rename, get_subcon_name};

// ===========================================================================
// Helper: collect subcons for + / >> merge
// ===========================================================================

/// Collects subcons from a Python object for the `+` operator (Struct merge).
///
/// If `obj` is a `PyStruct`, expands its `py_subcons`. Otherwise (including
/// `PySequence`), treats `obj` as a single subcon.
///
/// Mirrors Python `__add__` (`core.py:764`):
/// `lhs = self.subcons if isinstance(self, Struct) else [self]`
fn collect_subcons_for_struct_merge(
    py: Python<'_>,
    obj: &Bound<PyAny>,
    out_py: &mut Vec<PyObject>,
) -> PyResult<Vec<(Option<String>, Box<dyn Construct>)>> {
    let mut result = Vec::new();

    // Expand PyStruct only; PySequence and other types are treated as single subcons
    if let Ok(s) = obj.extract::<PyRef<crate::constructs_composite::PyStruct>>() {
        for sub in &s.py_subcons {
            let inner = extract_subcon(sub.bind(py))?;
            let name = get_subcon_name(sub.bind(py))?;
            result.push((name, inner));
            out_py.push(sub.clone_ref(py));
        }
        return Ok(result);
    }
    // Single subcon
    let inner = extract_subcon(obj)?;
    let name = get_subcon_name(obj)?;
    result.push((name, inner));
    out_py.push(obj.clone().unbind());
    Ok(result)
}

/// Collects subcons from a Python object for the `>>` operator (Sequence merge).
///
/// If `obj` is a `PySequence`, expands its `py_subcons`. Otherwise (including
/// `PyStruct`), treats `obj` as a single subcon.
///
/// Mirrors Python `__rshift__` (`core.py:772`):
/// `lhs = self.subcons if isinstance(self, Sequence) else [self]`
fn collect_subcons_for_sequence_merge(
    py: Python<'_>,
    obj: &Bound<PyAny>,
    out_py: &mut Vec<PyObject>,
) -> PyResult<Vec<(Option<String>, Box<dyn Construct>)>> {
    let mut result = Vec::new();

    // Expand PySequence only; PyStruct and other types are treated as single subcons
    if let Ok(s) = obj.extract::<PyRef<crate::constructs_composite::PySequence>>() {
        for sub in &s.py_subcons {
            let inner = extract_subcon(sub.bind(py))?;
            let name = get_subcon_name(sub.bind(py))?;
            result.push((name, inner));
            out_py.push(sub.clone_ref(py));
        }
        return Ok(result);
    }
    // Single subcon
    let inner = extract_subcon(obj)?;
    let name = get_subcon_name(obj)?;
    result.push((name, inner));
    out_py.push(obj.clone().unbind());
    Ok(result)
}

// ===========================================================================
// make_struct_from_add / make_sequence_from_rshift / make_array_from_getitem
// ===========================================================================

/// `a + b` → Struct. Merges if either side is already a Struct.
pub fn make_struct_from_add(
    py: Python<'_>,
    lhs: PyObject,
    rhs: &Bound<PyAny>,
) -> PyResult<PyObject> {
    let mut py_subs: Vec<PyObject> = Vec::new();
    let lhs_cons = collect_subcons_for_struct_merge(py, lhs.bind(py), &mut py_subs)?;
    let rhs_cons = collect_subcons_for_struct_merge(py, rhs, &mut py_subs)?;

    let mut builder = construct::constructs::Struct::new();
    for (name, sc) in lhs_cons.into_iter().chain(rhs_cons) {
        match name {
            Some(n) => builder = builder.field(n, sc),
            None => builder = builder.anonymous(sc),
        }
    }
    let py_struct = PyStruct {
        inner: builder,
        py_subcons: py_subs,
    };
    Ok(Py::new(py, py_struct)?.into_any())
}

/// `a >> b` → Sequence. Merges if either side is already a Sequence.
pub fn make_sequence_from_rshift(
    py: Python<'_>,
    lhs: PyObject,
    rhs: &Bound<PyAny>,
) -> PyResult<PyObject> {
    let mut py_subs: Vec<PyObject> = Vec::new();
    let lhs_cons = collect_subcons_for_sequence_merge(py, lhs.bind(py), &mut py_subs)?;
    let rhs_cons = collect_subcons_for_sequence_merge(py, rhs, &mut py_subs)?;

    let mut builder = construct::constructs::Sequence::new();
    for (name, sc) in lhs_cons.into_iter().chain(rhs_cons) {
        match name {
            Some(n) => builder = builder.named(n, sc),
            None => builder = builder.push(sc),
        }
    }
    let py_seq = PySequence {
        inner: builder,
        py_subcons: py_subs,
    };
    Ok(Py::new(py, py_seq)?.into_any())
}

/// `subcon[n]` → Array. n can be int or callable.
pub fn make_array_from_getitem(
    py: Python<'_>,
    subcon: PyObject,
    count: &Bound<PyAny>,
) -> PyResult<PyObject> {
    // Check for slice → error
    if count.is_instance_of::<PySlice>() {
        return Err(ConstructError::new_err(
            "subcon[N] syntax can only be used for Arrays; use GreedyRange(subcon) instead",
        ));
    }
    let sc = extract_subcon(subcon.bind(py))?;
    if count.is_callable() {
        let expr = py_param_to_evaluate(py, count)?;
        let inner: Box<dyn Construct> = Box::new(construct::constructs::ArrayExpr::new(expr, sc));
        Ok(Py::new(py, PyArray { inner })?.into_any())
    } else {
        let n = count.extract::<usize>()?;
        let inner: Box<dyn Construct> = Box::new(construct::constructs::Array::new(n, sc));
        Ok(Py::new(py, PyArray { inner })?.into_any())
    }
}

// ===========================================================================
// Struct
// ===========================================================================

/// PyO3 wrapper: Struct composite construct.
#[pyclass(name = "Struct", unsendable)]
pub struct PyStruct {
    pub(crate) inner: construct::constructs::Struct,
    pub(crate) py_subcons: Vec<PyObject>,
}

impl PyConstructWrapper for PyStruct {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Adds a subcon to a Struct builder, handling list unpacking.
fn add_struct_subcon(
    py: Python<'_>,
    mut builder: construct::constructs::Struct,
    py_subcons: &mut Vec<PyObject>,
    obj: &Bound<PyAny>,
) -> PyResult<construct::constructs::Struct> {
    // Unpack lists/tuples of subcons
    if obj.is_instance_of::<PyTuple>() || obj.is_instance_of::<pyo3::types::PyList>() {
        let items: Vec<PyObject> = obj.extract()?;
        for item in &items {
            builder = add_struct_subcon(py, builder, py_subcons, item.bind(py))?;
        }
        return Ok(builder);
    }
    let inner = extract_subcon(obj)?;
    let name = get_subcon_name(obj)?;
    match name {
        Some(n) => {
            builder = builder.field(n, inner);
        }
        None => {
            builder = builder.anonymous(inner);
        }
    }
    py_subcons.push(obj.clone().unbind());
    Ok(builder)
}

/// Factory: `Struct(*subcons, **subconskw)`.
#[pyfunction]
#[pyo3(name = "Struct", signature = (*subcons, **subconskw))]
pub fn py_struct(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PyStruct> {
    let mut builder = construct::constructs::Struct::new();
    let mut py_subcons = Vec::new();

    for item in subcons.iter() {
        builder = add_struct_subcon(py, builder, &mut py_subcons, &item)?;
    }
    for (key, val) in subconskw.into_iter().flat_map(|d| d.iter()) {
        let name: String = key.extract()?;
        let renamed = apply_rename(py, &val, &name)?;
        let inner = extract_subcon(renamed.bind(py))?;
        builder = builder.field(name, inner);
        py_subcons.push(renamed);
    }

    Ok(PyStruct {
        inner: builder,
        py_subcons,
    })
}

#[pymethods]
impl PyStruct {
    /// Member attribute exposure: `struct.fieldname` → subcon.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        for sub in &self.py_subcons {
            if let Some(n) = get_subcon_name(sub.bind(py)).ok().flatten() {
                if n == name {
                    return Ok(sub.clone_ref(py));
                }
            }
        }
        Err(PyAttributeError::new_err(format!(
            "Struct has no field '{name}'"
        )))
    }

    fn __repr__(&self) -> String {
        format!("Struct({} fields)", self.py_subcons.len())
    }
}

crate::impl_api_methods!(PyStruct);
crate::impl_construct_operators!(PyStruct);

// ===========================================================================
// Sequence
// ===========================================================================

/// PyO3 wrapper: Sequence composite construct.
#[pyclass(name = "Sequence", unsendable)]
pub struct PySequence {
    pub(crate) inner: construct::constructs::Sequence,
    pub(crate) py_subcons: Vec<PyObject>,
}

impl PyConstructWrapper for PySequence {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Sequence(*subcons, **subconskw)`.
#[pyfunction]
#[pyo3(name = "Sequence", signature = (*subcons, **subconskw))]
pub fn py_sequence(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PySequence> {
    let mut builder = construct::constructs::Sequence::new();
    let mut py_subcons = Vec::new();

    for item in subcons.iter() {
        let inner = extract_subcon(&item)?;
        let name = get_subcon_name(&item)?;
        match name {
            Some(n) => builder = builder.named(n, inner),
            None => builder = builder.push(inner),
        }
        py_subcons.push(item.clone().unbind());
    }
    for (key, val) in subconskw.into_iter().flat_map(|d| d.iter()) {
        let name: String = key.extract()?;
        let renamed = apply_rename(py, &val, &name)?;
        let inner = extract_subcon(renamed.bind(py))?;
        builder = builder.named(name, inner);
        py_subcons.push(renamed);
    }

    Ok(PySequence {
        inner: builder,
        py_subcons,
    })
}

crate::impl_api_methods!(PySequence);
crate::impl_construct_operators!(PySequence);

// ===========================================================================
// Array
// ===========================================================================

/// PyO3 wrapper: Array (fixed or dynamic count).
#[pyclass(name = "Array", unsendable)]
pub struct PyArray {
    pub(crate) inner: Box<dyn Construct>,
}

impl PyConstructWrapper for PyArray {
    fn as_construct(&self) -> &dyn Construct {
        &*self.inner
    }
}

/// Factory: `Array(count, subcon)`. count int → fixed, callable → dynamic.
#[pyfunction]
#[pyo3(name = "Array", signature = (count, subcon))]
pub fn py_array(py: Python<'_>, count: &Bound<PyAny>, subcon: &Bound<PyAny>) -> PyResult<PyArray> {
    let sc = extract_subcon(subcon)?;
    if count.is_callable() {
        let expr = py_param_to_evaluate(py, count)?;
        Ok(PyArray {
            inner: Box::new(construct::constructs::ArrayExpr::new(expr, sc)),
        })
    } else {
        let n = count.extract::<usize>()?;
        Ok(PyArray {
            inner: Box::new(construct::constructs::Array::new(n, sc)),
        })
    }
}

crate::impl_api_methods!(PyArray);
crate::impl_construct_operators!(PyArray);
// ===========================================================================
// GreedyRange / RepeatUntil
// ===========================================================================

/// PyO3 wrapper: GreedyRange (parse until EOF).
#[pyclass(name = "GreedyRange", unsendable)]
pub struct PyGreedyRange {
    pub(crate) inner: construct::constructs::GreedyRange,
}

impl PyConstructWrapper for PyGreedyRange {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `GreedyRange(subcon, discard=False)`.
#[pyfunction]
#[pyo3(name = "GreedyRange", signature = (subcon, discard=false))]
pub fn py_greedy_range(
    _py: Python<'_>,
    subcon: &Bound<PyAny>,
    discard: bool,
) -> PyResult<PyGreedyRange> {
    let sc = extract_subcon(subcon)?;
    let inner = if discard {
        construct::constructs::GreedyRange::new_discard(sc)
    } else {
        construct::constructs::GreedyRange::new(sc)
    };
    Ok(PyGreedyRange { inner })
}

crate::impl_api_methods!(PyGreedyRange);
crate::impl_construct_operators!(PyGreedyRange);

/// PyO3 wrapper: RepeatUntil (repeat until predicate returns true).
#[pyclass(name = "RepeatUntil", unsendable)]
pub struct PyRepeatUntil {
    pub(crate) inner: construct::constructs::RepeatUntil,
}

impl PyConstructWrapper for PyRepeatUntil {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `RepeatUntil(func, subcon, discard=False)`.
#[pyfunction]
#[pyo3(name = "RepeatUntil", signature = (func, subcon, discard=false))]
pub fn py_repeat_until(
    _py: Python<'_>,
    func: &Bound<PyAny>,
    subcon: &Bound<PyAny>,
    discard: bool,
) -> PyResult<PyRepeatUntil> {
    let predicate = py_to_repeat_predicate(func.clone().unbind());
    let sc = extract_subcon(subcon)?;
    let inner = if discard {
        construct::constructs::RepeatUntil::new_discard(predicate, sc)
    } else {
        construct::constructs::RepeatUntil::new(predicate, sc)
    };
    Ok(PyRepeatUntil { inner })
}

crate::impl_api_methods!(PyRepeatUntil);
crate::impl_construct_operators!(PyRepeatUntil);

// ===========================================================================
// Union / Select / FocusedSeq
// ===========================================================================

/// PyO3 wrapper: Union (parse one, build one).
#[pyclass(name = "Union", unsendable)]
pub struct PyUnion {
    pub(crate) inner: construct::constructs::Union,
}

impl PyConstructWrapper for PyUnion {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Collect subcons for Union/Select from *args and **kwargs.
fn collect_struct_fields(
    py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<(
    Vec<construct::constructs::struct_::StructField>,
    Vec<PyObject>,
)> {
    let mut fields = Vec::new();
    let mut py_subs = Vec::new();
    for item in subcons.iter() {
        let inner = extract_subcon(&item)?;
        let name = get_subcon_name(&item)?;
        match name {
            Some(n) => fields.push(construct::constructs::struct_::StructField::new(n, inner)),
            None => fields.push(construct::constructs::struct_::StructField::anonymous(
                inner,
            )),
        }
        py_subs.push(item.clone().unbind());
    }
    for (key, val) in subconskw.into_iter().flat_map(|d| d.iter()) {
        let name: String = key.extract()?;
        let renamed = apply_rename(py, &val, &name)?;
        let inner = extract_subcon(renamed.bind(py))?;
        fields.push(construct::constructs::struct_::StructField::new(
            name, inner,
        ));
        py_subs.push(renamed);
    }
    Ok((fields, py_subs))
}

/// Factory: `Union(parsefrom, *subcons, **subconskw)`.
#[pyfunction]
#[pyo3(name = "Union", signature = (parsefrom, *subcons, **subconskw))]
pub fn py_union(
    py: Python<'_>,
    parsefrom: &Bound<PyAny>,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PyUnion> {
    let target = if parsefrom.is_none() {
        None
    } else if let Ok(i) = parsefrom.extract::<usize>() {
        Some(construct::constructs::UnionTarget::Index(i))
    } else if let Ok(s) = parsefrom.extract::<String>() {
        Some(construct::constructs::UnionTarget::Name(s))
    } else {
        return Err(pyo3::exceptions::PyTypeError::new_err(
            "parsefrom must be int, str, or None",
        ));
    };
    let (fields, _py) = collect_struct_fields(py, subcons, subconskw)?;
    Ok(PyUnion {
        inner: construct::constructs::Union::new(target, fields),
    })
}

crate::impl_api_methods!(PyUnion);
crate::impl_construct_operators!(PyUnion);

/// PyO3 wrapper: Select (try each subcon until one succeeds).
#[pyclass(name = "Select", unsendable)]
pub struct PySelect {
    pub(crate) inner: construct::constructs::Select,
}

impl PyConstructWrapper for PySelect {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Select(*subcons, **subconskw)`.
#[pyfunction]
#[pyo3(name = "Select", signature = (*subcons, **subconskw))]
pub fn py_select(
    _py: Python<'_>,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PySelect> {
    let mut subs: Vec<Box<dyn Construct>> = Vec::new();
    for item in subcons.iter() {
        subs.push(extract_subcon(&item)?);
    }
    if let Some(d) = subconskw {
        for (_key, val) in d.iter() {
            subs.push(extract_subcon(&val)?);
        }
    }
    Ok(PySelect {
        inner: construct::constructs::Select::new(subs),
    })
}

crate::impl_api_methods!(PySelect);
crate::impl_construct_operators!(PySelect);

/// PyO3 wrapper: FocusedSeq (parse/build only the focused field).
#[pyclass(name = "FocusedSeq", unsendable)]
pub struct PyFocusedSeq {
    pub(crate) inner: construct::constructs::FocusedSeq,
}

impl PyConstructWrapper for PyFocusedSeq {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `FocusedSeq(parsebuildfrom, *subcons, **subconskw)`.
#[pyfunction]
#[pyo3(name = "FocusedSeq", signature = (parsebuildfrom, *subcons, **subconskw))]
pub fn py_focused_seq(
    py: Python<'_>,
    parsebuildfrom: &str,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PyFocusedSeq> {
    let (fields, _py) = collect_struct_fields(py, subcons, subconskw)?;
    Ok(PyFocusedSeq {
        inner: construct::constructs::FocusedSeq::new(parsebuildfrom, fields),
    })
}

crate::impl_api_methods!(PyFocusedSeq);
crate::impl_construct_operators!(PyFocusedSeq);

// ===========================================================================
// Padded / Aligned
// ===========================================================================

/// PyO3 wrapper: Padded (pad subcon to fixed length).
#[pyclass(name = "Padded", unsendable)]
pub struct PyPadded {
    pub(crate) inner: construct::constructs::Padded,
}

impl PyConstructWrapper for PyPadded {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Padded(length, subcon, pattern=b"\x00")`.
#[pyfunction]
#[pyo3(name = "Padded", signature = (length, subcon, pattern=0))]
pub fn py_padded(
    _py: Python<'_>,
    length: usize,
    subcon: &Bound<PyAny>,
    pattern: u8,
) -> PyResult<PyPadded> {
    let sc = extract_subcon(subcon)?;
    let pad = pattern;
    Ok(PyPadded {
        inner: construct::constructs::Padded::new(length, sc, pad, false),
    })
}

crate::impl_api_methods!(PyPadded);
crate::impl_construct_operators!(PyPadded);

/// PyO3 wrapper: Aligned (align subcon to modulus boundary).
#[pyclass(name = "Aligned", unsendable)]
pub struct PyAligned {
    pub(crate) inner: construct::constructs::Aligned,
}

impl PyConstructWrapper for PyAligned {
    fn as_construct(&self) -> &dyn Construct {
        &self.inner
    }
}

/// Factory: `Aligned(modulus, subcon, pattern=b"\x00")`.
#[pyfunction]
#[pyo3(name = "Aligned", signature = (modulus, subcon, pattern=0))]
pub fn py_aligned(
    _py: Python<'_>,
    modulus: usize,
    subcon: &Bound<PyAny>,
    pattern: u8,
) -> PyResult<PyAligned> {
    let sc = extract_subcon(subcon)?;
    let pad = pattern;
    Ok(PyAligned {
        inner: construct::constructs::Aligned::new(modulus, sc, pad),
    })
}

crate::impl_api_methods!(PyAligned);
crate::impl_construct_operators!(PyAligned);

/// Function: `Padding(length, pattern=b"\x00")` → Padded(length, Pass).
#[pyfunction]
#[pyo3(name = "Padding", signature = (length, pattern=0))]
pub fn py_padding(length: usize, pattern: u8) -> PyResult<PyPadded> {
    let pad = pattern;
    Ok(PyPadded {
        inner: construct::constructs::Padded::new(
            length,
            Box::new(construct::constructs::Pass::new()),
            pad,
            false,
        ),
    })
}

/// Function: `AlignedStruct(modulus, *subcons, **subconskw)` → Struct with Aligned fields.
#[pyfunction]
#[pyo3(name = "AlignedStruct", signature = (modulus, *subcons, **subconskw))]
pub fn py_aligned_struct(
    _py: Python<'_>,
    modulus: usize,
    subcons: &Bound<PyTuple>,
    subconskw: Option<&Bound<PyDict>>,
) -> PyResult<PyStruct> {
    let mut builder = construct::constructs::Struct::new();
    let mut py_subcons = Vec::new();

    for item in subcons.iter() {
        let inner = extract_subcon(&item)?;
        let name = get_subcon_name(&item)?.ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("AlignedStruct requires named subcons")
        })?;
        let aligned = construct::constructs::Aligned::new(modulus, inner, 0x00);
        builder = builder.field(name.clone(), Box::new(aligned));
        py_subcons.push(item.clone().unbind());
    }
    for (key, val) in subconskw.into_iter().flat_map(|d| d.iter()) {
        let name: String = key.extract()?;
        let inner = extract_subcon(&val)?;
        let aligned = construct::constructs::Aligned::new(modulus, inner, 0x00);
        builder = builder.field(name.clone(), Box::new(aligned));
        py_subcons.push(val.clone().unbind());
    }

    Ok(PyStruct {
        inner: builder,
        py_subcons,
    })
}
