use pyo3::prelude::*;
use pyo3::types::PyString;

#[pyclass]
struct TestClass { val: i32 }

#[pymethods]
impl TestClass {
    #[new]
    fn new(val: i32) -> Self { TestClass { val } }

    fn get_pyobj_via_intoref(slf: PyRef<Self>, py: Python<'_>) -> PyObject {
        slf.into_py(py)
    }

    fn get_pyobj_via_intoref_any(slf: PyRef<Self>, py: Python<'_>) -> PyObject {
        let py_t: Py<Self> = slf.into_py(py);
        py_t.into_any()
    }
}

fn main() {}