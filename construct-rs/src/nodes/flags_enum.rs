//! FlagsEnumNode：标志位枚举节点。
//!
//! 设计依据：`docs/design/模块设计/模块设计-Phase8-P1P2.md` §1.3.2。
//! Python 参考：`construct/construct/core.py` `FlagsEnum`（L2018-2109）。
//!
//! ## 行为概述
//!
//! `FlagsEnum(subcon, *merge, **flags)` 把 subcon 返回的整数映射为多个 bool 标志。
//!
//! - parse：inner.parse(int) → 遍历 flags 构造 PyDict（每项 `name: (obj & value) == value`）
//!   并设置 `_flagsenum=True` 特殊标志。返回该 dict
//! - build：按 obj 类型分支：
//!   - int → inner.build(obj)
//!   - str → split("|") 后查 encmapping 累加 OR（key 缺失 → MappingError）
//!   - dict → 遍历 items，name 不以 "_" 开头且 value truthy 时累加 OR
//! - sizeof：转发 inner.sizeof
//!
//! ## 编译期物化
//!
//! `flags` 编译期物化为 `Vec<(Py<PyString>, i64)>`（name + value），减少 PyDict 遍历
//! 开销（Rust 端直接遍历 Vec）。`encmapping` 同 EnumNode（str→int）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyLong, PyString};

use super::Construct;

/// 标志位枚举节点：subcon 整数 → dict（每 flag 一 bool）。
///
/// 对应 Python construct `FlagsEnum(subcon, *merge, **flags)`（core.py L2018）。
/// 详见模块级注释。
#[derive(Debug)]
pub struct FlagsEnumNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// flags 列表：编译期物化为 Rust Vec（避免运行时 PyDict 遍历）。
    /// (label_name, raw_value)。name 是 Py<PyString>（用于 parse 构造 dict key）。
    flags: Vec<(Py<PyString>, i64)>,
    /// 编码映射：label(str) → value(int)。用于 build 的 str/dict 分支。
    encmapping: Py<PyDict>,
}

impl FlagsEnumNode {
    /// 创建 `FlagsEnumNode`。
    pub fn new(inner: Node, flags: Vec<(Py<PyString>, i64)>, encmapping: Py<PyDict>) -> Self {
        Self {
            inner: Box::new(inner),
            flags,
            encmapping,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回 flags 列表切片。
    pub fn flags(&self) -> &[(Py<PyString>, i64)] {
        &self.flags
    }
}

impl Construct for FlagsEnumNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let int_val: i64 = obj
            .bind(py)
            .extract()
            .map_err(|_| ConstructError::Generic {
                message: format!(
                    "FlagsEnum inner must return int, got {}",
                    obj.bind(py)
                        .get_type()
                        .name()
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                ),
                path: path.to_string(),
            })?;
        let result = PyDict::new_bound(py);
        // _flagsenum=True 标志（对齐 Python core.py L2072）。
        result.set_item("_flagsenum", true)?;
        for (name, value) in &self.flags {
            let set = (int_val & value) == *value;
            result.set_item(name.bind(py), set)?;
        }
        Ok(result.into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let int_val: i64 = if obj.is_instance_of::<PyLong>() {
            // int 直接用（含 bool：Python bool 是 int 子类，C-6 同 Enum 处理）。
            obj.extract().map_err(|_| ConstructError::Generic {
                message: "FlagsEnum build: int obj extract failed".to_string(),
                path: path.to_string(),
            })?
        } else if obj.is_instance_of::<PyString>() {
            // str → split("|") 后查 encmapping OR。
            let s: String = obj.extract().map_err(|_| ConstructError::Generic {
                message: "FlagsEnum build: str obj extract failed".to_string(),
                path: path.to_string(),
            })?;
            let mut acc: i64 = 0;
            for name in s.split('|') {
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let name_py = PyString::new_bound(py, trimmed);
                match self
                    .encmapping
                    .bind(py)
                    .get_item(&name_py)
                    .map_err(ConstructError::from)?
                {
                    Some(v) => {
                        acc |= v.extract::<i64>().map_err(|_| ConstructError::Generic {
                            message: "FlagsEnum encmapping value not int".to_string(),
                            path: path.to_string(),
                        })?
                    }
                    None => {
                        return Err(ConstructError::Mapping {
                            message: format!(
                                "building failed, unknown label: {}",
                                obj.repr().map(|r| r.to_string()).unwrap_or_default()
                            ),
                            path: path.to_string(),
                        })
                    }
                }
            }
            acc
        } else if obj.is_instance_of::<PyDict>() {
            // dict → 遍历 items，name 不以 "_" 开头且 value truthy 时 OR。
            let d = obj
                .downcast::<PyDict>()
                .map_err(|_| ConstructError::Generic {
                    message: "FlagsEnum build: dict downcast failed".to_string(),
                    path: path.to_string(),
                })?;
            let mut acc: i64 = 0;
            for (k, v) in d.iter() {
                let name_str: String = match k.extract() {
                    Ok(s) => s,
                    Err(_) => continue, // 非 str key 跳过（对齐 Python 隐式行为）。
                };
                if name_str.starts_with('_') {
                    continue;
                }
                let truthy = v.is_truthy()?;
                if truthy {
                    let k_repr = k.repr().ok().map(|r| r.to_string()).unwrap_or_default();
                    match self
                        .encmapping
                        .bind(py)
                        .get_item(k)
                        .map_err(ConstructError::from)?
                    {
                        Some(val) => {
                            acc |= val.extract::<i64>().map_err(|_| ConstructError::Generic {
                                message: "FlagsEnum encmapping value not int".to_string(),
                                path: path.to_string(),
                            })?
                        }
                        None => {
                            return Err(ConstructError::Mapping {
                                message: format!("building failed, unknown label: {}", k_repr),
                                path: path.to_string(),
                            })
                        }
                    }
                }
            }
            acc
        } else {
            return Err(ConstructError::Mapping {
                message: format!(
                    "building failed, FlagsEnum build expects int/str/dict, got {}",
                    obj.get_type()
                        .name()
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                ),
                path: path.to_string(),
            });
        };
        let int_obj = int_val.into_py(py);
        self.inner.build(py, int_obj.bind(py), stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.inner.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::Construct;

    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    /// 构造 FlagsEnumNode 测试 fixture：inner=Byte, flags={one:1, two:2, four:4, eight:8}。
    fn make_flags_enum_node(py: Python<'_>) -> FlagsEnumNode {
        let flags = vec![
            (PyString::new_bound(py, "one").unbind(), 1),
            (PyString::new_bound(py, "two").unbind(), 2),
            (PyString::new_bound(py, "four").unbind(), 4),
            (PyString::new_bound(py, "eight").unbind(), 8),
        ];
        let encmapping = PyDict::new_bound(py);
        encmapping.set_item("one", 1).unwrap();
        encmapping.set_item("two", 2).unwrap();
        encmapping.set_item("four", 4).unwrap();
        encmapping.set_item("eight", 8).unwrap();
        let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big));
        FlagsEnumNode::new(inner, flags, encmapping.unbind())
    }

    #[test]
    fn parse_returns_flag_dict() {
        // FE-1: FlagsEnum(Byte, one=1, two=2, four=4, eight=8).parse(b'\x03')
        //      → dict{_flagsenum=True, one=True, two=True, four=False, eight=False}
        with_py(|py| {
            let node = make_flags_enum_node(py);
            let mut stream = ParseStream::new(&[0x03]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let d = result.bind(py).downcast::<PyDict>().expect("dict");
            assert_eq!(
                d.get_item("one")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                true
            );
            assert_eq!(
                d.get_item("two")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                true
            );
            assert_eq!(
                d.get_item("four")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                false
            );
            assert_eq!(
                d.get_item("eight")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                false
            );
            assert_eq!(
                d.get_item("_flagsenum")
                    .unwrap()
                    .unwrap()
                    .extract::<bool>()
                    .unwrap(),
                true
            );
        });
    }

    #[test]
    fn build_from_dict() {
        // FE-2: build(dict(one=True, two=True)) → b'\x03'
        with_py(|py| {
            let node = make_flags_enum_node(py);
            let obj = py
                .eval_bound("dict(one=True, two=True)", None, None)
                .expect("dict");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x03]);
        });
    }

    #[test]
    fn build_from_pipe_string() {
        // FE-3: build('one|two') → b'\x03'
        with_py(|py| {
            let node = make_flags_enum_node(py);
            let obj = py.eval_bound("'one|two'", None, None).expect("str");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x03]);
        });
    }

    #[test]
    fn build_from_int() {
        // FE-4: build(3) → b'\x03'
        with_py(|py| {
            let node = make_flags_enum_node(py);
            let obj = py.eval_bound("3", None, None).expect("int");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x03]);
        });
    }

    #[test]
    fn build_dict_with_underscore_key_skipped() {
        // FE-5: build(dict(one=True, _flagsenum=True)) → b'\x01'（_flagsenum 跳过）
        with_py(|py| {
            let node = make_flags_enum_node(py);
            let obj = py
                .eval_bound("dict(one=True, _flagsenum=True)", None, None)
                .expect("dict");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01]);
        });
    }

    #[test]
    fn build_unknown_label_raises_mapping_error() {
        // FE-6: build('unknown') → MappingError
        with_py(|py| {
            let node = make_flags_enum_node(py);
            let obj = py.eval_bound("'unknown'", None, None).expect("str");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Mapping { .. }));
        });
    }

    #[test]
    fn sizeof_forwards_to_inner() {
        // FE-9（隐式）
        with_py(|py| {
            let node = make_flags_enum_node(py);
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 1);
        });
    }
}
