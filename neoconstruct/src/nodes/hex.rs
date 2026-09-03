//! HexNode / HexDumpNode：Hex 显示包装节点（共享模块）。
//!
//! Python 参考：`construct/construct/core.py` `Hex`（L3523-3580）+ `HexDump`（L3583-3635）。
//!
//! ## 关键设计决策：Hex/HexDump 走 Rust Node（非 AdapterCallbackNode）
//!
//! Hex/HexDump 是 construct 核心库内置 Adapter（非用户自定义），与 Subconstruct/RawCopy 同档。
//! parse 逻辑简单（按 obj 类型分支 + Python 显示类 factory 调用），Rust 内联判断 +
//! Python 类构造即可（C API 调用不算额外 FFI）。
//!
//! ## 性能优化
//!
//! - **int 分支直调**：int 分支用 Rust 内 `call1(cls, (intvalue,))` + `setattr("fmtstr", fmtstr)`
//!   替代跨 FFI 的 `call_method1("new", ...)`，消除 Python 字节码进入。
//! - **fmtstr 编译期 intern**：fmtstr 编译期 intern 为 `Py<PyString>`，parse 时直接借用，
//!   消除每次 parse 的 String clone + PyString::new。
//!
//! ## 共享 HexDisplayClasses
//!
//! Hex 需要 3 个显示类（Integer/Bytes/Dict），HexDump 需要 2 个（Bytes/Dict）。
//! 两者都从 `neoconstruct.lib.hex` 模块编译期加载。

use crate::context::Context;
use crate::error::ConstructError;
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyLong, PyString, PyType};

use super::Construct;

/// Hex 三类显示类的引用（编译期物化）。
///
/// 从 `neoconstruct.lib.hex` 模块 getattr 一次，存入 HexNode。
/// 运行时 parse 通过 Rust C API 直接实例化：
/// - int 分支：`call1(cls, (intvalue,))` + `setattr("fmtstr", fmtstr)`
/// - bytes/dict 分支：`call1(cls, (obj,))`（原实现）
#[derive(Debug)]
pub struct HexDisplayClasses {
    /// `HexDisplayedInteger`（int 子类，含 `new(intvalue, fmtstr)` 静态方法）。
    pub integer: Py<PyType>,
    /// `HexDisplayedBytes`（bytes 子类，`__init__(self, obj)`）。
    pub bytes_cls: Py<PyType>,
    /// `HexDisplayedDict`（dict 子类，`__init__(self, obj)`）。
    pub dict: Py<PyType>,
}

/// HexDump 两类显示类的引用（编译期物化）。
#[derive(Debug)]
pub struct HexDumpDisplayClasses {
    /// `HexDumpDisplayedBytes`（bytes 子类）。
    pub bytes_cls: Py<PyType>,
    /// `HexDumpDisplayedDict`（dict 子类）。
    pub dict: Py<PyType>,
}

/// 编译期从 `neoconstruct.lib.hex` 模块加载 Hex 3 个显示类。
///
/// 模块加载失败（理论不应发生，`neoconstruct.lib.hex` 是核心库）→ CompilationError。
pub fn load_hex_display_classes(py: Python<'_>) -> Result<HexDisplayClasses, ConstructError> {
    let module =
        py.import_bound("neoconstruct.lib.hex")
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to import neoconstruct.lib.hex: {}", e),
            })?;
    let get = |name: &str| -> Result<Py<PyType>, ConstructError> {
        module
            .getattr(name)
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to get neoconstruct.lib.hex.{}: {}", name, e),
            })?
            .extract::<Py<PyType>>()
            .map_err(|e| ConstructError::Compilation {
                message: format!("neoconstruct.lib.hex.{} is not a Python type: {}", name, e),
            })
    };
    Ok(HexDisplayClasses {
        integer: get("HexDisplayedInteger")?,
        bytes_cls: get("HexDisplayedBytes")?,
        dict: get("HexDisplayedDict")?,
    })
}

/// 编译期从 `neoconstruct.lib.hex` 模块加载 HexDump 2 个显示类。
pub fn load_hexdump_display_classes(
    py: Python<'_>,
) -> Result<HexDumpDisplayClasses, ConstructError> {
    let module =
        py.import_bound("neoconstruct.lib.hex")
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to import neoconstruct.lib.hex: {}", e),
            })?;
    let get = |name: &str| -> Result<Py<PyType>, ConstructError> {
        module
            .getattr(name)
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to get neoconstruct.lib.hex.{}: {}", name, e),
            })?
            .extract::<Py<PyType>>()
            .map_err(|e| ConstructError::Compilation {
                message: format!("neoconstruct.lib.hex.{} is not a Python type: {}", name, e),
            })
    };
    Ok(HexDumpDisplayClasses {
        bytes_cls: get("HexDumpDisplayedBytes")?,
        dict: get("HexDumpDisplayedDict")?,
    })
}

/// Hex 显示包装节点：根据 inner.parse 结果类型分派到对应 Python 显示类。
///
/// 对应 Python construct `Hex(subcon)`（core.py L3523）。在 neoconstruct 中
/// 实现为 Rust Node（非 AdapterCallbackNode）。
///
/// # 三方法行为
///
/// - parse：inner.parse → 按 obj 类型分派 → 调用对应 Python 显示类 factory
///   - int → `HexDisplayedInteger(intvalue)` + `setattr("fmtstr", fmtstr)`
///     （Rust 内 call1+setattr 替代跨 FFI 的 `call_method1("new")`，
///     消除 Python 字节码进入）
///   - bytes → `HexDisplayedBytes(obj)`
///   - dict → `HexDisplayedDict(obj)`
///   - else → 透传
/// - build：透传 inner.build（obj 是显示对象，但有对应原始类型 __base__，
///   pyo3 提取时自动转 int/bytes/dict）
/// - sizeof：转发 inner.sizeof
#[derive(Debug)]
pub struct HexNode {
    /// 被包装的子树根。
    inner: Box<Node>,
    /// 编译期加载的 Python 显示类引用。
    display_classes: HexDisplayClasses,
    /// 编译期 intern 的 fmtstr PyString（parse 时直接借用，消除每次 PyString::new）。
    /// None 表示 inner sizeof 不可静态计算，需运行期 fallback 构造。
    fmtstr: Option<Py<PyString>>,
}

impl HexNode {
    /// 创建 `HexNode`。
    ///
    /// `fmtstr` 参数应为编译期 intern 的 `Py<PyString>`。
    /// 传入 `None` 时 parse 将运行期计算 fmtstr（inner sizeof fallback）。
    pub fn new(
        inner: Node,
        display_classes: HexDisplayClasses,
        fmtstr: Option<Py<PyString>>,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            display_classes,
            fmtstr,
        }
    }

    /// 返回内部子树根节点的引用。
    pub fn inner(&self) -> &Node {
        &self.inner
    }

    /// 返回编译期 intern 的 fmtstr 引用。
    pub fn fmtstr(&self) -> Option<&Py<PyString>> {
        self.fmtstr.as_ref()
    }
}

impl Construct for HexNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let obj = self.inner.parse(py, stream, ctx, path)?;
        let bound = obj.bind(py);
        // Rust 内 is_instance_of 判断（pyo3 内部走 PyObject_IsInstance）。
        if bound.is_instance_of::<PyLong>() {
            // int → HexDisplayedInteger(intvalue) + setattr("fmtstr", fmtstr)
            //
            // int 分支用 Rust 内 call1+setattr 替代 call_method1("new")，
            // 消除 getattr(cls,"new") + staticmethod 描述符 + ceval 字节码 dispatch。
            // call1 走 type.__call__ → long_new（C 级），setattr 走 PyObject_GenericSetAttr（C 级）。
            // 等价完成 HexDisplayedInteger.new(intvalue, fmtstr) 字节码体：
            //   obj = HexDisplayedInteger(intvalue); obj.fmtstr = fmtstr; return obj
            let cls_bound = self.display_classes.integer.bind(py);
            let new_obj = cls_bound
                .call1((bound,))
                .map_err(|e| ConstructError::Generic {
                    message: format!("HexDisplayedInteger() instantiation failed: {}", e),
                    path: path.to_string(),
                })?;

            // 编译期 intern fmtstr，parse 时直接借用。
            match &self.fmtstr {
                Some(interned) => {
                    new_obj.setattr("fmtstr", interned.bind(py)).map_err(|e| {
                        ConstructError::Generic {
                            message: format!("setattr fmtstr failed: {}", e),
                            path: path.to_string(),
                        }
                    })?;
                }
                None => {
                    // 运行期 fallback：用 inner sizeof 计算 fmtstr
                    // （inner sizeof 不可静态计算时走此路径，每次构造新 PyString）
                    let size = self.inner.sizeof(ctx).unwrap_or(0);
                    let fmt_str = format!("0{}X", 2 * size);
                    let fmt_py = PyString::new_bound(py, &fmt_str);
                    new_obj
                        .setattr("fmtstr", &fmt_py)
                        .map_err(|e| ConstructError::Generic {
                            message: format!("setattr fmtstr failed: {}", e),
                            path: path.to_string(),
                        })?;
                }
            }

            Ok(new_obj.unbind())
        } else if bound.is_instance_of::<PyBytes>() {
            // bytes → HexDisplayedBytes(obj)
            let cls_bound = self.display_classes.bytes_cls.bind(py);
            let new_obj = cls_bound
                .call1((bound,))
                .map_err(|e| ConstructError::Generic {
                    message: format!("HexDisplayedBytes() failed: {}", e),
                    path: path.to_string(),
                })?;
            Ok(new_obj.unbind())
        } else if bound.is_instance_of::<PyDict>() {
            // dict → HexDisplayedDict(obj)
            let cls_bound = self.display_classes.dict.bind(py);
            let new_obj = cls_bound
                .call1((bound,))
                .map_err(|e| ConstructError::Generic {
                    message: format!("HexDisplayedDict() failed: {}", e),
                    path: path.to_string(),
                })?;
            Ok(new_obj.unbind())
        } else {
            // 未知类型透传
            Ok(obj)
        }
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // obj 可能是 HexDisplayedInteger（int 子类）/ HexDisplayedBytes（bytes 子类）等
        // 直接转发 inner.build——inner 自己 extract（pyo3 子类转基类自动）。
        self.inner.build(py, obj, stream, ctx, path)
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
    use crate::nodes::bytes::BytesNode;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};

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

    /// 加载真实 Hex 显示类（如果 neoconstruct.lib.hex 可用）。
    fn try_load_classes(py: Python<'_>) -> Option<HexDisplayClasses> {
        load_hex_display_classes(py).ok()
    }

    /// 测试辅助：创建 intern 的 fmtstr PyString。
    fn intern_fmtstr(py: Python<'_>, s: &str) -> Py<PyString> {
        PyString::new_bound(py, s).into_py(py)
    }

    #[test]
    fn parse_int_returns_hex_displayed_integer() {
        // Hex(Int32ub).parse(b'\x00\x00\x01\x02') → HexDisplayedInteger(258)
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => {
                    eprintln!("skipping: neoconstruct.lib.hex not available");
                    return;
                }
            };
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = HexNode::new(inner, classes, Some(intern_fmtstr(py, "08X")));
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x01, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bound = result.bind(py);
            assert!(bound.is_instance_of::<PyLong>());
            let v: i64 = bound.extract().unwrap();
            assert_eq!(v, 258);
            // 验证是 HexDisplayedInteger（不是普通 int）：检查 str
            let locals = pyo3::types::PyDict::new_bound(py);
            locals.set_item("x", bound).unwrap();
            let s: String = py
                .eval_bound("str(x)", None, Some(&locals))
                .ok()
                .and_then(|r| r.extract().ok())
                .unwrap_or_default();
            assert_eq!(s, "0x00000102");
            // parity 验证：fmtstr 属性已设置
            let fmtstr_val: String = bound.getattr("fmtstr").unwrap().extract().unwrap();
            assert_eq!(fmtstr_val, "08X");
        });
    }

    #[test]
    fn parse_bytes_returns_hex_displayed_bytes() {
        // Hex(Bytes(4)).parse(b'\x00\x00\x01\x02') → HexDisplayedBytes
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => return,
            };
            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = HexNode::new(inner, classes, None);
            let mut stream = ParseStream::new(&[0x00, 0x00, 0x01, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let bound = result.bind(py);
            assert!(bound.is_instance_of::<PyBytes>());
        });
    }

    #[test]
    fn build_forwards_to_inner() {
        // Hex(Int32ub).build(HexDisplayedInteger(258, "08X")) → b'\x00\x00\x01\x02'
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => return,
            };
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = HexNode::new(inner, classes, Some(intern_fmtstr(py, "08X")));
            // 构造 HexDisplayedInteger(258, "08X")
            let cls = node_display_cls(py, "HexDisplayedInteger");
            let obj = cls.call_method1("new", (258i64, "08X")).expect("new");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x00, 0x00, 0x01, 0x02]);
        });
    }

    fn node_display_cls<'py>(py: Python<'py>, name: &str) -> Bound<'py, PyType> {
        py.import_bound("neoconstruct.lib.hex")
            .expect("import")
            .getattr(name)
            .expect("getattr")
            .extract::<Bound<'_, PyType>>()
            .expect("extract type")
    }

    #[test]
    fn sizeof_returns_inner_size() {
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => return,
            };
            let inner = Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big));
            let node = HexNode::new(inner, classes, Some(intern_fmtstr(py, "08X")));
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 4);
        });
    }

    #[test]
    fn debug_format_includes_hex_node() {
        with_py(|py| {
            let classes = match try_load_classes(py) {
                Some(c) => c,
                None => return,
            };
            let inner = Node::Bytes(BytesNode::new_const(4));
            let node = HexNode::new(inner, classes, None);
            let s = format!("{:?}", node);
            assert!(s.contains("HexNode"), "got: {}", s);
        });
    }
}
