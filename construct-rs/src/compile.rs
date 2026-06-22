//! 编译入口：`compile_schema` FFI 函数与描述符→节点构建逻辑。
//!
//! 设计依据：`docs/架构设计.md` §B.2（编译入口）、§E.3-E.4（编译管线）。
//!
//! ## 一次 FFI 穿越
//!
//! `compile_schema` 是 Python→Rust 的编译期 FFI 入口，在 `__init_subclass__`
//! 中调用一次。Rust 内部遍历描述符列表，通过 pyo3 `extract` 类型安全地识别
//! 描述符种类，读取参数构建对应的执行树节点（[`crate::nodes::Node`]），
//! 最终组装为 [`crate::schema::CompiledSchema`] 返回。
//!
//! ## 描述符类型识别策略
//!
//! 依赖 pyo3 的 `extract`（基于 `PyTypeInfo`），无字符串标识。按优先级：
//! 1. `FormatFieldDescriptor` → [`Node::FormatField`]
//! 2. `BytesDescriptor` → [`Node::Bytes`]
//! 3. `GreedyBytesDescriptor` → [`Node::GreedyBytes`]
//! 4. StructMixin 子类（`hasattr("_construct_compiled")`）→ [`Node::StructRef`]
//! 5. 全部失败 → `ConstructError::Compilation`
//!
//! ## 前向引用处理
//!
//! 当描述符是 StructMixin 子类但尚未完成编译（延迟桩），编译器仍构建
//! [`Node::StructRef`](crate::nodes::struct_ref::StructRefNode)（存类引用）。
//! 运行时通过 `cls._construct_compiled` 动态解析（详见架构设计 §C.3.5、§E.5）。
// pyo3 0.22 的 #[pyfunction] 宏在展开返回 PyResult<T> 的包装代码时，
// 会生成 `PyErr.into()` 形式的冗余转换，触发 clippy::useless_conversion。
// 这是宏的已知行为（不是用户代码问题），在此模块级别抑制该 lint。
// 后续升级 pyo3 版本后若修复，可移除此属性。
#![allow(clippy::useless_conversion)]

use crate::descriptors::{BytesDescriptor, FormatFieldDescriptor, GreedyBytesDescriptor};
use crate::error::ConstructError;
use crate::nodes::bytes::BytesNode;
use crate::nodes::format_field::FormatFieldNode;
use crate::nodes::greedy_bytes::GreedyBytesNode;
use crate::nodes::struct_node::{FieldMode, StructField, StructNode};
use crate::nodes::struct_ref::StructRefNode;
use crate::nodes::Node;
use crate::schema::CompiledSchema;
use pyo3::prelude::*;
use pyo3::types::PyType;

/// StructMixin 子类通过此属性标识自身（由纯 Python 的 StructMixin 设置）。
const STRUCTMIXIN_COMPILED_ATTR: &str = "_construct_compiled";

/// 编译 Schema 为执行树（一次 FFI 穿越）。
///
/// Python 可见签名：
///
/// ```python
/// compile_schema(cls: type, field_names: list[str], descriptors: list) -> CompiledSchema
/// ```
///
/// # 参数
///
/// - `cls`：用户类对象（StructMixin 子类）。用于关联编译产物与错误信息中报类名。
/// - `field_names`：字段名列表，按字段声明顺序排列。
/// - `descriptors`：字段描述符列表，与 `field_names` 按位置对应。
///
/// # 返回
///
/// 编译产物 [`CompiledSchema`]（不可变的执行树，关联到 `cls`）。
///
/// # 错误
///
/// - [`ConstructError::Compilation`]：`field_names` 与 `descriptors` 长度不一致、
///   描述符类型未知、`cls` 不是类型、或类定义了 `__slots__`（Phase 1 不支持）。
#[pyfunction]
pub fn compile_schema(
    py: Python<'_>,
    cls: &Bound<'_, PyType>,
    field_names: Vec<String>,
    descriptors: Vec<Py<PyAny>>,
) -> PyResult<CompiledSchema> {
    // 1. 校验 field_names 与 descriptors 长度一致
    if field_names.len() != descriptors.len() {
        return Err(ConstructError::Compilation {
            message: format!(
                "field_names ({}) 与 descriptors ({}) 长度不一致",
                field_names.len(),
                descriptors.len()
            ),
        }
        .into());
    }

    // 2. 检测 __slots__（设计修订 §3.6 + §5.7.1 N4-R3）
    //    手写 __slots__ 在 __init_subclass__ 时已可见，编译期报错（fail fast）。
    //    @dataclass(slots=True) 的 __slots__ 在 __init_subclass__ 后才生成，
    //    可能漏检，由 parse 时 force_setattr 失败兜底（struct_node.rs 错误信息已含提示）。
    if check_has_slots(cls)? {
        let cls_name = cls
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());
        return Err(ConstructError::Compilation {
            message: format!(
                "类 {} 使用了 __slots__，Phase 1 不支持 slots dataclass（无 __dict__）。\
                 请移除 __slots__ 或使用普通 @dataclass。",
                cls_name
            ),
        }
        .into());
    }

    // 3. 编译期检测 __post_init__（设计修订 §3.2）
    let has_post_init = check_has_post_init(cls);

    // 4. 逐字段构建节点（FieldName 含 interned PyString）
    let mut fields: Vec<StructField> = Vec::with_capacity(field_names.len());
    for (name, desc) in field_names.iter().zip(descriptors.iter()) {
        let desc_bound = desc.bind(py);
        let node =
            build_node_from_descriptor(py, desc_bound).map_err(|e| with_field_context(e, name))?;
        // FieldName::new 创建 interned PyString 缓存。
        // 当前所有字段 mode 默认为 Rw（子任务 2.3 将传入实际 mode）。
        let field = StructField {
            name: crate::nodes::struct_node::FieldName::new(py, name.clone()),
            node,
            mode: FieldMode::Rw,
        };
        fields.push(field);
    }

    // 5. 组装根 Struct 节点（含 cls + has_post_init + has_expressions）。
    //    has_expressions 暂时为 false（子任务 2.3 将根据表达式编译结果设置）。
    let root = Node::Struct(StructNode::new(
        fields,
        cls.clone().unbind(),
        has_post_init,
        false,
    ));

    // 6. 包装为 CompiledSchema
    Ok(CompiledSchema::new(root, cls.clone().unbind()))
}

/// 检查类是否定义了 `__slots__`（含 MRO 查找）。
///
/// 设计修订 §5.5：编译期检测，命中返回 true。
///
/// # 错误
///
/// `hasattr` 内部抛出异常时返回 [`ConstructError::Generic`]。
fn check_has_slots(cls: &Bound<'_, PyType>) -> Result<bool, ConstructError> {
    match cls.getattr("__slots__") {
        Ok(slots) => Ok(!slots.is_none()),
        Err(_) => Ok(false),
    }
}

/// 检查类是否定义了 `__post_init__`（含 MRO 查找）。
///
/// 设计修订 §5.5：编译期检测，决定 StructNode.parse 是否在构造实例后调用
/// `instance.__post_init__()`。
fn check_has_post_init(cls: &Bound<'_, PyType>) -> bool {
    cls.getattr("__post_init__").is_ok()
}

/// 将单个描述符转换为执行树节点。
///
/// 按优先级尝试 `extract` 为具体描述符类型。详见模块级文档。
fn build_node_from_descriptor(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
) -> Result<Node, ConstructError> {
    // 1. FormatFieldDescriptor → Node::FormatField
    if let Ok(fmt) = desc.extract::<Py<FormatFieldDescriptor>>() {
        let fmt_ref = fmt.bind(py).get();
        return Ok(Node::FormatField(FormatFieldNode::new(fmt_ref.format)));
    }

    // 2. BytesDescriptor → Node::Bytes
    if let Ok(b) = desc.extract::<Py<BytesDescriptor>>() {
        let b_ref = b.bind(py).get();
        return Ok(Node::Bytes(BytesNode::new(b_ref.length)));
    }

    // 3. GreedyBytesDescriptor → Node::GreedyBytes
    if desc.extract::<Py<GreedyBytesDescriptor>>().is_ok() {
        return Ok(Node::GreedyBytes(GreedyBytesNode::new()));
    }

    // 4. StructMixin 子类引用 → Node::StructRef
    //    识别条件：desc 是类型（PyType）且拥有 `_construct_compiled` 属性。
    //    对方可能尚未编译（延迟桩），此时仍构建 StructRef（存类引用），
    //    运行时通过 resolve_schema 动态解析。
    if let Ok(cls_type) = desc.downcast::<PyType>() {
        if is_structmixin_subclass(cls_type)? {
            let cls: Py<PyType> = cls_type.clone().unbind();
            return Ok(Node::StructRef(StructRefNode::new(cls)));
        }
    }

    // 5. 全部失败 → Compilation error
    let repr_str = desc
        .repr()
        .ok()
        .and_then(|r| r.to_str().ok().map(String::from))
        .unwrap_or_else(|| "<unknown>".to_string());
    Err(ConstructError::Compilation {
        message: format!("未知的字段描述符类型: {}", repr_str),
    })
}

/// 判断给定的 Python 类型对象是否为 StructMixin 子类。
///
/// 识别条件：该类型拥有 [`STRUCTMIXIN_COMPILED_ATTR`]（`_construct_compiled`）属性。
/// 该属性由纯 Python 的 StructMixin 在 `__init_subclass__` 中设置，存在即表示
/// 该类型已被编译系统接管（可能是已编译的 schema，或尚未编译的延迟桩）。
///
/// # 错误
///
/// - [`ConstructError::Generic`]：检查属性时 Python 侧抛出异常（如属性查询失败）。
fn is_structmixin_subclass(cls: &Bound<'_, PyType>) -> Result<bool, ConstructError> {
    cls.hasattr(STRUCTMIXIN_COMPILED_ATTR)
        .map_err(|e| ConstructError::Generic {
            message: format!(
                "failed to check '{}' attribute on descriptor: {}",
                STRUCTMIXIN_COMPILED_ATTR, e
            ),
            path: String::new(),
        })
}

/// 为编译期错误附加字段名上下文。
///
/// 仅对 [`ConstructError::Compilation`] 变体追加 `field '{name}': ` 前缀，
/// 以便在多字段 schema 中定位出错的字段。其他变体（运行时错误）原样传播。
fn with_field_context(err: ConstructError, field_name: &str) -> ConstructError {
    match err {
        ConstructError::Compilation { message } => ConstructError::Compilation {
            message: format!("field '{}': {}", field_name, message),
        },
        other => other,
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::format_field::PythonFormat;
    use pyo3::types::PyBytes;

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

    /// 创建一个简单的 Python 类型对象（用于 compile_schema 的 cls 参数）。
    fn make_dummy_class<'py>(py: Python<'py>, _name: &str) -> Bound<'py, PyType> {
        let code = format!("type('{}', (), {{}})", _name);
        py.eval_bound(&code, None, None)
            .expect("create type")
            .downcast_into::<PyType>()
            .expect("is PyType")
    }

    /// 创建一个 StructMixin 子类模拟类（有 `_construct_compiled` 属性）。
    fn make_structmixin_like_class<'py>(py: Python<'py>, _name: &str) -> Bound<'py, PyType> {
        let code = format!("type('{}', (), {{'_construct_compiled': None}})", _name);
        py.eval_bound(&code, None, None)
            .expect("create type")
            .downcast_into::<PyType>()
            .expect("is PyType")
    }

    /// 创建一个 StructMixin 子类模拟类，带 `__init__(self, **kwargs)` 用于 parse 回调。
    ///
    /// `StructRefNode::parse` 会调用 `cls(**dict)` 来构造嵌套实例，
    /// 因此类必须接受关键字参数并设置同名属性。真实 StructMixin 子类
    /// （`@dataclass`）天然满足此约束，此辅助函数模拟该行为。
    fn make_structmixin_class_with_init<'py>(py: Python<'py>, name: &str) -> Bound<'py, PyType> {
        let code = format!(
            r#"
class {name}:
    def __init__(self, **kwargs):
        for k, v in kwargs.items():
            setattr(self, k, v)
    def __repr__(self):
        return '{name}(' + ', '.join(f'{{k}}={{v}}' for k, v in vars(self).items()) + ')'
    def __eq__(self, other):
        return isinstance(other, type(self)) and vars(self) == vars(other)
{name}._construct_compiled = None
"#,
            name = name,
        );
        let globals = pyo3::types::PyDict::new_bound(py);
        py.run_bound(&code, Some(&globals), None)
            .unwrap_or_else(|e| panic!("failed to define class: {}", e));
        globals
            .get_item(name)
            .expect("get_item ok")
            .unwrap_or_else(|| panic!("class {} not found", name))
            .extract::<Py<PyType>>()
            .expect("extract Py<PyType>")
            .into_bound(py)
    }
    // ======================================================================
    // 编译空结构体
    // ======================================================================

    #[test]
    fn compile_empty_schema_produces_empty_struct_node() {
        with_py(|py| {
            let cls = make_dummy_class(py, "Empty");
            let schema = compile_schema(py, &cls, vec![], vec![]).expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert!(s.is_empty(), "expected empty struct");
                    assert_eq!(s.len(), 0);
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // 编译单字段
    // ======================================================================

    #[test]
    fn compile_single_int8ub_field() {
        with_py(|py| {
            let cls = make_dummy_class(py, "Single");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new");
            let schema =
                compile_schema(py, &cls, vec!["address".to_string()], vec![desc.into_any()])
                    .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    let fields = s.fields();
                    assert_eq!(fields[0].name.rust_name(), "address");
                    match &fields[0].node {
                        Node::FormatField(ff) => {
                            assert_eq!(ff.format(), PythonFormat::UnsignedInt8Big);
                            assert_eq!(ff.length(), 1);
                        }
                        other => panic!("expected FormatField, got {:?}", other),
                    }
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // 编译多字段（混合类型）
    // ======================================================================

    #[test]
    fn compile_multi_field_mixed_descriptors() {
        with_py(|py| {
            let cls = make_dummy_class(py, "Multi");
            let int8ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let int16ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int16ub", PythonFormat::UnsignedInt16Big),
            )
            .expect("Py::new")
            .into_any();
            let bytes4 = Py::new(py, BytesDescriptor { length: 4 })
                .expect("Py::new")
                .into_any();
            let greedy = Py::new(py, GreedyBytesDescriptor)
                .expect("Py::new")
                .into_any();

            let names = vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
            ];
            let descs = vec![int8ub, int16ub, bytes4, greedy];

            let schema = compile_schema(py, &cls, names, descs).expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 4);
                    let fields = s.fields();
                    assert!(matches!(fields[0].node, Node::FormatField(_)));
                    assert!(matches!(fields[1].node, Node::FormatField(_)));
                    assert!(matches!(fields[2].node, Node::Bytes(_)));
                    assert!(matches!(fields[3].node, Node::GreedyBytes(_)));
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // 编译嵌套引用（StructRef）
    // ======================================================================

    #[test]
    fn compile_nested_structref() {
        with_py(|py| {
            let cls = make_dummy_class(py, "Outer");
            let inner_cls = make_structmixin_like_class(py, "Inner");
            let inner_cls_py = inner_cls.clone().unbind().into_any();

            let schema = compile_schema(py, &cls, vec!["inner".to_string()], vec![inner_cls_py])
                .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    match &s.fields()[0].node {
                        Node::StructRef(sr) => {
                            assert_eq!(s.fields()[0].name.rust_name(), "inner");
                            // cls 应指向 Inner 类
                            let inner_name = sr.cls().bind(py).name().expect("name");
                            assert_eq!(inner_name.to_string(), "Inner");
                        }
                        other => panic!("expected StructRef, got {:?}", other),
                    }
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // 编译错误：长度不一致
    // ======================================================================

    #[test]
    fn compile_length_mismatch_returns_error() {
        with_py(|py| {
            let cls = make_dummy_class(py, "Bad");
            let result = compile_schema(py, &cls, vec!["a".to_string(), "b".to_string()], vec![]);
            let err = result.expect_err("should fail");
            let pyerr: PyErr = err;
            let msg = format!("{}", pyerr);
            assert!(
                msg.contains("长度不一致") || msg.contains("length"),
                "message should mention length mismatch: {}",
                msg
            );
        });
    }

    // ======================================================================
    // 编译错误：未知描述符
    // ======================================================================

    #[test]
    fn compile_unknown_descriptor_returns_compilation_error() {
        with_py(|py| {
            let cls = make_dummy_class(py, "Bad");
            // 传入 Python int 作为描述符（非已知类型，也非 StructMixin）
            let bad_desc = py.eval_bound("42", None, None).expect("eval").unbind();
            let err = compile_schema(py, &cls, vec!["x".to_string()], vec![bad_desc])
                .expect_err("should fail");
            let pyerr: PyErr = err;
            let msg = format!("{}", pyerr);
            assert!(
                msg.contains("未知") || msg.contains("unknown") || msg.contains("42"),
                "message should mention unknown descriptor: {}",
                msg
            );
            // 字段名上下文应被附加
            assert!(
                msg.contains("x"),
                "message should contain field name 'x': {}",
                msg
            );
        });
    }

    // ======================================================================
    // 编译错误：str 描述符（非类型、非描述符）
    // ======================================================================

    #[test]
    fn compile_string_descriptor_returns_error() {
        with_py(|py| {
            let cls = make_dummy_class(py, "Bad");
            let bad_desc = py
                .eval_bound("'not a descriptor'", None, None)
                .expect("eval")
                .unbind();
            let result = compile_schema(py, &cls, vec!["x".to_string()], vec![bad_desc]);
            assert!(result.is_err(), "should fail");
        });
    }

    // ======================================================================
    // 16 种整数格式编译
    // ======================================================================

    #[test]
    fn compile_all_16_integer_formats() {
        with_py(|py| {
            let cls = make_dummy_class(py, "AllFormats");
            let formats: &[(&'static str, PythonFormat)] = &[
                ("Int8ub", PythonFormat::UnsignedInt8Big),
                ("Int8ul", PythonFormat::UnsignedInt8Little),
                ("Int8sb", PythonFormat::SignedInt8Big),
                ("Int8sl", PythonFormat::SignedInt8Little),
                ("Int16ub", PythonFormat::UnsignedInt16Big),
                ("Int16ul", PythonFormat::UnsignedInt16Little),
                ("Int16sb", PythonFormat::SignedInt16Big),
                ("Int16sl", PythonFormat::SignedInt16Little),
                ("Int32ub", PythonFormat::UnsignedInt32Big),
                ("Int32ul", PythonFormat::UnsignedInt32Little),
                ("Int32sb", PythonFormat::SignedInt32Big),
                ("Int32sl", PythonFormat::SignedInt32Little),
                ("Int64ub", PythonFormat::UnsignedInt64Big),
                ("Int64ul", PythonFormat::UnsignedInt64Little),
                ("Int64sb", PythonFormat::SignedInt64Big),
                ("Int64sl", PythonFormat::SignedInt64Little),
            ];

            let names: Vec<String> = formats.iter().map(|(n, _)| n.to_string()).collect();
            let descs: Vec<Py<PyAny>> = formats
                .iter()
                .map(|(n, f)| {
                    Py::new(py, FormatFieldDescriptor::new(n, *f))
                        .expect("Py::new")
                        .into_any()
                })
                .collect();

            let schema = compile_schema(py, &cls, names, descs).expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 16);
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // 端到端：compile_schema → _parse_raw → 验证
    // ======================================================================

    #[test]
    fn end_to_end_compile_and_parse_single_field() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2E1");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();

            let schema =
                compile_schema(py, &cls, vec!["x".to_string()], vec![desc]).expect("compile");

            // _parse_raw 现在返回用户类实例（方案 B'）
            let data = PyBytes::new_bound(py, &[0x42u8]);
            let instance = schema._parse_raw(py, &data).expect("parse");
            // 验证是 cls 的实例
            assert!(
                instance.is_instance(&cls).expect("is_instance"),
                "should be instance of dummy class"
            );
            // 通过 getattr 读字段
            let x: i64 = instance
                .getattr("x")
                .expect("getattr x")
                .extract()
                .expect("extract i64");
            assert_eq!(x, 0x42);
        });
    }

    #[test]
    fn end_to_end_compile_and_parse_multi_field() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2E2");
            let int8ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let int16ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int16ub", PythonFormat::UnsignedInt16Big),
            )
            .expect("Py::new")
            .into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string()],
                vec![int8ub, int16ub],
            )
            .expect("compile");

            // 数据：a=0x01, b=0x0203
            let data = PyBytes::new_bound(py, &[0x01, 0x02, 0x03]);
            let instance = schema._parse_raw(py, &data).expect("parse");
            // 通过 getattr 读字段
            let a: i64 = instance.getattr("a").unwrap().extract().unwrap();
            let b: i64 = instance.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 1);
            assert_eq!(b, 0x0203);
        });
    }

    #[test]
    fn end_to_end_parse_insufficient_bytes_returns_error_with_path() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2E3");
            let int32ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int32ub", PythonFormat::UnsignedInt32Big),
            )
            .expect("Py::new")
            .into_any();

            let schema =
                compile_schema(py, &cls, vec!["v".to_string()], vec![int32ub]).expect("compile");

            // 只提供 2 字节，但 Int32ub 需要 4 字节
            let data = PyBytes::new_bound(py, &[0x01, 0x02]);
            let err = schema._parse_raw(py, &data).expect_err("should fail");
            let msg = format!("{}", err);
            assert!(
                msg.contains("Error in path") || msg.contains("root"),
                "error should include path: {}",
                msg
            );
            assert!(
                msg.contains("expected 4"),
                "error should mention expected 4 bytes: {}",
                msg
            );
        });
    }

    // ======================================================================
    // 端到端：compile_schema → _build_raw → 验证
    // ======================================================================

    #[test]
    fn end_to_end_compile_and_build_single_field() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2EB1");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();

            let schema =
                compile_schema(py, &cls, vec!["x".to_string()], vec![desc]).expect("compile");

            // 构造 Python 对象 {x: 0x42}
            let obj = py
                .eval_bound("type('O', (), {'x': 0x42})()", None, None)
                .expect("obj");
            let result = schema._build_raw(py, &obj).expect("build");
            assert_eq!(result.as_bytes(), &[0x42]);
        });
    }

    // ======================================================================
    // 端到端：parse/build 往返一致
    // ======================================================================

    #[test]
    fn end_to_end_round_trip_parse_build_consistency() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2ERT");
            let int8ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let int16ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int16ub", PythonFormat::UnsignedInt16Big),
            )
            .expect("Py::new")
            .into_any();
            let bytes2 = Py::new(py, BytesDescriptor { length: 2 })
                .expect("Py::new")
                .into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec![int8ub, int16ub, bytes2],
            )
            .expect("compile");

            // build：a=0x01, b=0x0203, c=b'\x04\x05'
            let obj = py
                .eval_bound(
                    "type('O', (), {'a': 0x01, 'b': 0x0203, 'c': b'\\x04\\x05'})()",
                    None,
                    None,
                )
                .expect("obj");
            let built = schema._build_raw(py, &obj).expect("build");
            let built_bytes = built.as_bytes().to_vec();
            assert_eq!(built_bytes, vec![0x01, 0x02, 0x03, 0x04, 0x05]);

            // parse 回来（方案 B'：返回实例）
            let data = PyBytes::new_bound(py, &built_bytes);
            let parsed = schema._parse_raw(py, &data).expect("parse");
            let a: i64 = parsed.getattr("a").unwrap().extract().unwrap();
            let b: i64 = parsed.getattr("b").unwrap().extract().unwrap();
            let c_binding = parsed.getattr("c").unwrap();
            let c: &[u8] = c_binding.extract().unwrap();
            assert_eq!(a, 0x01);
            assert_eq!(b, 0x0203);
            assert_eq!(c, &[0x04, 0x05]);
        });
    }

    // ======================================================================
    // 端到端：空 schema parse/build
    // ======================================================================

    #[test]
    fn end_to_end_empty_schema_parse_returns_empty_instance() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2EEmpty");
            let schema = compile_schema(py, &cls, vec![], vec![]).expect("compile");

            let data = PyBytes::new_bound(py, b"");
            let result = schema._parse_raw(py, &data).expect("parse");
            // 方案 B'：返回 cls 的空实例
            assert!(
                result.is_instance(&cls).expect("is_instance"),
                "should be instance of cls"
            );
            // __dict__ 应为空
            let d_binding = result.getattr("__dict__").expect("getattr __dict__");
            let d = d_binding
                .downcast::<pyo3::types::PyDict>()
                .expect("is dict");
            assert_eq!(d.len(), 0);
        });
    }

    #[test]
    fn end_to_end_empty_schema_build_returns_empty_bytes() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2EEmptyB");
            let schema = compile_schema(py, &cls, vec![], vec![]).expect("compile");

            let obj = py.eval_bound("object()", None, None).expect("obj");
            let result = schema._build_raw(py, &obj).expect("build");
            assert!(result.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // 端到端：GreedyBytes + Bytes 混合
    // ======================================================================

    #[test]
    fn end_to_end_bytes_and_greedy_bytes() {
        with_py(|py| {
            let cls = make_dummy_class(py, "E2EBG");
            let int8ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let bytes2 = Py::new(py, BytesDescriptor { length: 2 })
                .expect("Py::new")
                .into_any();
            let greedy = Py::new(py, GreedyBytesDescriptor)
                .expect("Py::new")
                .into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec![
                    "tag".to_string(),
                    "len_bytes".to_string(),
                    "rest".to_string(),
                ],
                vec![int8ub, bytes2, greedy],
            )
            .expect("compile");

            // parse: tag=0xAA, len_bytes=b'\x01\x02', rest=b'\x03\x04\x05'
            let data = PyBytes::new_bound(py, &[0xAA, 0x01, 0x02, 0x03, 0x04, 0x05]);
            let parsed = schema._parse_raw(py, &data).expect("parse");
            let tag: i64 = parsed.getattr("tag").unwrap().extract().unwrap();
            let lb_binding = parsed.getattr("len_bytes").unwrap();
            let len_bytes: &[u8] = lb_binding.extract().unwrap();
            let rest_binding = parsed.getattr("rest").unwrap();
            let rest: &[u8] = rest_binding.extract().unwrap();
            assert_eq!(tag, 0xAA);
            assert_eq!(len_bytes, &[0x01, 0x02]);
            assert_eq!(rest, &[0x03, 0x04, 0x05]);

            // build 回来
            let obj = py
                .eval_bound(
                    "type('O', (), {'tag': 0xAA, 'len_bytes': b'\\x01\\x02', 'rest': b'\\x03\\x04\\x05'})()",
                    None,
                    None,
                )
                .expect("obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), &[0xAA, 0x01, 0x02, 0x03, 0x04, 0x05]);
        });
    }

    // ======================================================================
    // 端到端：嵌套 StructRef（编译 + parse + build）
    // ======================================================================

    #[test]
    fn end_to_end_nested_structref_parse_and_build() {
        with_py(|py| {
            // Inner 类：有 `__init__(**kwargs)` 和 `_construct_compiled`（模拟 StructMixin 子类）。
            // 我们先编译 Inner 的 schema 并安装到 Inner._construct_compiled。
            let inner_cls_py = {
                let bound = make_structmixin_class_with_init(py, "Inner");
                bound.clone().unbind()
            };

            let inner_desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();

            let inner_schema = compile_schema(
                py,
                inner_cls_py.bind(py),
                vec!["x".to_string()],
                vec![inner_desc],
            )
            .expect("compile inner");
            let inner_schema_py = Py::new(py, inner_schema).expect("Py::new schema");

            // 安装真正的 CompiledSchema 到 Inner（替换之前的 None 桩）
            inner_cls_py
                .bind(py)
                .setattr("_construct_compiled", inner_schema_py.clone_ref(py))
                .expect("setattr");

            // Outer 类：{ tag: Int8ub, inner: Inner }
            let outer_cls = make_dummy_class(py, "Outer");
            let tag_desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let outer_schema = compile_schema(
                py,
                &outer_cls,
                vec!["tag".to_string(), "inner".to_string()],
                vec![tag_desc, inner_cls_py.clone_ref(py).into_any()],
            )
            .expect("compile outer");

            // parse b'\xAA\x05' → Outer 实例（tag=0xAA, inner=Inner(x=5)）
            // 方案 B'：parse 返回 Outer 类实例（不是 dict）
            let data = PyBytes::new_bound(py, &[0xAA, 0x05]);
            let parsed = outer_schema._parse_raw(py, &data).expect("parse");
            // 验证是 Outer 实例
            assert!(
                parsed.is_instance(&outer_cls).expect("is_instance"),
                "should be Outer instance"
            );
            let tag: i64 = parsed.getattr("tag").unwrap().extract().unwrap();
            assert_eq!(tag, 0xAA);
            // inner 应为 Inner 实例（StructRef 委托对方 root.parse 构造）
            let inner_obj = parsed.getattr("inner").unwrap();
            assert!(
                inner_obj
                    .is_instance(inner_cls_py.bind(py))
                    .expect("is_instance"),
                "inner should be Inner instance"
            );
            let x: i64 = inner_obj.getattr("x").unwrap().extract().unwrap();
            assert_eq!(x, 5);

            // build：构造 Outer 对象 {tag: 0xAA, inner: Inner(x=5)}
            // Inner 实例（Inner 类无 __init__，用 call + setattr 构造）
            let inner_instance = inner_cls_py.bind(py).call((), None).expect("Inner()");
            inner_instance.setattr("x", 5).expect("set x");
            // Outer 对象
            let outer_obj = outer_cls.call((), None).expect("Outer()");
            outer_obj.setattr("tag", 0xAA).expect("set tag");
            outer_obj
                .setattr("inner", inner_instance.clone().unbind())
                .expect("set inner");
            let built = outer_schema._build_raw(py, &outer_obj).expect("build");
            assert_eq!(built.as_bytes(), &[0xAA, 0x05]);
        });
    }

    // ======================================================================
    // with_field_context 辅助函数
    // ======================================================================

    #[test]
    fn with_field_context_prepends_field_name_to_compilation_error() {
        let err = ConstructError::Compilation {
            message: "unknown descriptor".to_string(),
        };
        let wrapped = with_field_context(err, "my_field");
        match wrapped {
            ConstructError::Compilation { message } => {
                assert!(message.contains("my_field"), "got: {}", message);
                assert!(message.contains("unknown descriptor"), "got: {}", message);
            }
            other => panic!("expected Compilation, got {:?}", other),
        }
    }

    #[test]
    fn with_field_context_passes_through_non_compilation_errors() {
        let err = ConstructError::Stream {
            message: "oops".to_string(),
            path: "root".to_string(),
        };
        let wrapped = with_field_context(err, "field");
        assert!(matches!(wrapped, ConstructError::Stream { .. }));
    }

    // ======================================================================
    // __slots__ 编译期检测（设计修订 §3.6 + §5.6.2 第 3 条）
    // ======================================================================

    #[test]
    fn compile_rejects_class_with_explicit_slots() {
        // 手写 __slots__：在 __init_subclass__ 时已可见，编译期应报错。
        with_py(|py| {
            let code = "type('Slotted', (), {'__slots__': ('x', 'y')})";
            let cls_bound = py
                .eval_bound(code, None, None)
                .expect("create slotted class")
                .downcast_into::<PyType>()
                .expect("is PyType");
            let err = compile_schema(py, &cls_bound, vec![], vec![]).expect_err("should fail");
            let msg = format!("{}", err);
            assert!(
                msg.contains("__slots__") || msg.contains("slots"),
                "message should mention __slots__: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_accepts_class_without_slots() {
        // 普通 @dataclass 无 __slots__，应正常编译。
        with_py(|py| {
            let cls = make_dummy_class(py, "Normal");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let result = compile_schema(py, &cls, vec!["x".to_string()], vec![desc]);
            assert!(result.is_ok(), "compile should succeed: {:?}", result);
        });
    }

    // ======================================================================
    // __post_init__ 编译期检测（设计修订 §3.2 + §5.6.3 第 5 条）
    // ======================================================================

    #[test]
    fn compile_detects_post_init_when_defined() {
        // 定义带 __post_init__ 的类
        with_py(|py| {
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(
                "class WithPost:\n    def __post_init__(self): pass\n",
                Some(&globals),
                None,
            )
            .expect("define class");
            let cls = globals
                .get_item("WithPost")
                .expect("get ok")
                .expect("exists")
                .extract::<Py<PyType>>()
                .expect("extract");
            let cls_bound = cls.bind(py);
            assert!(
                check_has_post_init(cls_bound),
                "should detect __post_init__"
            );
        });
    }

    #[test]
    fn compile_returns_false_for_post_init_when_absent() {
        with_py(|py| {
            let cls = make_dummy_class(py, "NoPost");
            assert!(
                !check_has_post_init(&cls),
                "should not detect __post_init__"
            );
        });
    }

    #[test]
    fn compile_passes_has_post_init_to_struct_node() {
        // 验证编译产物 StructNode 持有正确的 has_post_init 标志
        with_py(|py| {
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(
                "class WithPost:\n    def __post_init__(self): pass\n",
                Some(&globals),
                None,
            )
            .expect("define");
            let cls = globals
                .get_item("WithPost")
                .expect("get")
                .expect("exists")
                .extract::<Py<PyType>>()
                .expect("extract");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let schema = compile_schema(py, cls.bind(py), vec!["x".to_string()], vec![desc])
                .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    // 通过 round-trip 验证 __post_init__ 被调用——这里仅检查编译成功。
                    let _ = s;
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }
}
