//! PyO3 module entry point for the `construct_rust` native extension.
//!
//! The compiled cdylib is loaded as `construct_rust._core` (see
//! `pyproject.toml` -> `module-name`). The pure-Python package
//! `construct_rust/__init__.py` re-exports everything via `from ._core import *`.
//!
//! Subsequent Phase 10 sub-tasks (10.2-10.9) register their `#[pyclass]` /
//! `#[pyfunction]` items inside the [`_core`] function body.

use pyo3::prelude::*;

/// PyO3 module entry -- registers the native extension symbols.
///
/// In 10.1 only `__version__` is exposed. Each downstream sub-task appends its
/// `m.add_class::<X>()` / `m.add_function(...)` calls here (the per-type logic
/// lives in dedicated `src/*.rs` files, not in this entry point).
#[pymodule]
fn _core(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Crate version (from Cargo.toml). The pure-Python `version.py` carries the
    // construct-compatible version tuple separately.
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    // 10.2 will register the ConstructError exception hierarchy here, e.g.:
    //   m.add("ConstructError", ...)?;
    // 10.3+ will register Container/ListContainer, construct wrappers, etc.

    Ok(())
}
