//! StructNode：字段序列根节点。
//!
//! 设计依据：`docs/架构设计.md` §C.3.4、`docs/设计修订-parse路径优化.md` §3.2（方案 B'）。
//! Python 参考：`construct/construct/core.py` `Struct._parse` / `_build`（L2162-2268）。
//!
//! ## 行为概述
//!
//! StructNode 是 StructMixin 子类执行树的根节点：按顺序解析/构建一组命名字段。
//!
//! - **parse（方案 B'）**：Rust 内完整构造用户类实例——创建独立 PyDict →
//!   逐字段解析并 `set_item` → [`crate::instance::create_class`] 创建空实例 →
//!   [`crate::instance::force_setattr`] 整体替换 `__dict__` → 可选调用 `__post_init__`。
//!   返回最终的用户类实例（不是 dict），消除 Python 侧 `cls(**dict)` O(N²) 开销。
//! - **build**：逐字段从 Python 对象 `getattr` 取值，递归子节点构建，写入 stream。
//! - **sizeof**：累加所有字段 sizeof；任一字段返回 Err 则整体返回 Err。
//!
//! ## 方案 B' 与 pydantic-core 对齐
//!
//! 实例构造策略与 `pydantic_core::validators::model` 一致：
//! - `create_class`（model.rs:348-365）：`tp_new(cls, (), NULL)`。
//! - `set_model_attrs`（model.rs:367-379）：构造独立 dict 并整体替换 `__dict__`。
//! - `force_setattr`（model.rs:381-394）：`PyObject_GenericSetAttr` 绕过自定义 `__setattr__`。
//!
//! ## 路径追踪（P0-3 优化：成功路径零成本）
//!
//! 成功路径**不**调用 `path.push_field`/`path.pop`（消除每字段 `String` 堆分配）。
//! 子节点返回 `Err` 时，通过 [`ConstructError::push_path_segment`] 将当前字段名
//! 插入错误路径，重建完整路径（仅错误路径开销，对齐 pydantic-core 的
//! `validation_state.rs` 设计）。
//!
//! ## 关于 Context 的嵌套
//!
//! Phase 1 不支持 this 表达式，context 不会被读取。因此 StructNode.parse/build
//! **不调用 `ctx.set_field`**（设计修订 §3.5）。

use crate::context::Context;
use crate::error::ConstructError;
use crate::instance::{create_class, force_setattr, intern_pystring};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyString, PyType};

use super::{Construct, Node};

/// 编译期缓存的字段名：包含 interned PyString（C API 快速比较）和 Rust 侧 String。
///
/// 设计依据：设计修订 §3.2。
///
/// - `py_name`：interned `Py<PyString>`，parse 时作为 dict key（避免每次创建 str
///   + 计算 hash）、build 时作为 `getattr` 参数（interned 可命中 method cache）。
/// - `rust_name`：Rust 侧字符串副本，用于 path 追踪与错误信息。
#[derive(Debug)]
pub struct FieldName {
    /// 缓存的 interned Python 字符串引用。
    py_name: Py<PyString>,
    /// Rust 侧字段名（用于 path、错误信息）。
    rust_name: String,
}

impl FieldName {
    /// 创建字段名。`py` 用于创建 interned PyString。
    ///
    /// # 参数
    ///
    /// - `py`：GIL token。
    /// - `name`：字段名字符串。
    pub fn new(py: Python<'_>, name: impl Into<String>) -> Self {
        let rust_name = name.into();
        let py_name = intern_pystring(py, &rust_name);
        Self { py_name, rust_name }
    }

    /// 返回 interned Python 字符串引用（`Py<PyString>`，拥有所有权）。
    pub fn py_name(&self) -> &Py<PyString> {
        &self.py_name
    }

    /// 返回 Rust 侧字段名（用于 path 追踪与错误信息）。
    pub fn rust_name(&self) -> &str {
        &self.rust_name
    }
}

/// 字段模式：RW（读写）、RO（只读）、WO（只写）。
///
/// 设计依据：`docs/模块设计-表达式系统.md` §5.2。
///
/// - `Rw`：读写——build 从实例取值，parse 存入实例。
/// - `Ro`：只读——build 自动计算（不从实例取值），parse 存入实例。
///   （RO 节点 Tell/Computed 在子任务 2.6 实现。）
/// - `Wo`：只写——build 从实例取值，parse 不存入实例（丢弃）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldMode {
    /// 读写：build 从实例取值，parse 存入实例。
    Rw,
    /// 只读：build 自动计算（不从实例取值），parse 存入实例。
    Ro,
    /// 只写：build 从实例取值，parse 不存入实例（丢弃）。
    Wo,
}

/// StructNode 的单个字段：携带模式信息。
///
/// 设计依据：`docs/模块设计-表达式系统.md` §5.2。
///
/// 将 Phase 1 的 `(FieldName, Node)` 元组扩展为携带 `mode`（RW/RO/WO）的结构体，
/// 供 parse/build 路径按模式分支处理。
#[derive(Debug)]
pub struct StructField {
    /// 字段名（interned PyString + Rust String）。
    pub name: FieldName,
    /// 子节点。
    pub node: Node,
    /// 字段模式（RW/RO/WO）。
    pub mode: FieldMode,
}

/// StructMixin 子类的根节点：按顺序解析/构建一组命名字段。
///
/// 对应 Python construct 的 `Struct`。
///
/// # parse 行为（方案 B'）
///
/// 1. 创建/复用 `PyDict`（见下方 `has_expressions` 分支）。
/// 2. 对每个 `StructField`：
///    - `field.node.parse(...)` —— 递归解析子节点
///    - 根据 `field.mode` 分支：
///      - `Rw`/`Ro`：`dict.set_item(py_name, value)` —— 存入 dict
///      - `Wo`：`drop(value)` —— 仅消费字节，不存入 dict
///    - 子节点 `Err` 时 `push_path_segment(rust_name)` 重建路径（仅错误路径）
/// 3. [`create_class`]：`tp_new(cls, (), NULL)` 创建空实例。
/// 4. [`force_setattr`]：`PyObject_GenericSetAttr(instance, "__dict__", dict)`
///    整体替换实例 `__dict__`。
/// 5. 若 `has_post_init`：调用 `instance.__post_init__()`。
/// 6. 返回实例（用户类对象，不是 dict）。
///
/// `has_expressions` 控制 context/dict 模式：
/// - `false`：新建独立 `PyDict`（Phase 1 行为，使用 `placeholder` context）。
/// - `true`：复用 ctx 的 `PyDict` 作为 instance dict（`new_root` context）。
///   parse 结束时 `clone_ref` dict 给实例，ctx 的 dict 引用随 ctx 丢弃。
///
/// 成功路径**不维护 Path**（P0-3 优化，零 String 分配）。
///
/// parse **不要求消费全部输入**：多余字节被忽略（对齐 Python construct Struct 语义）。
///
/// # build 行为
///
/// 对每个 `StructField`，根据 `field.mode` 分支：
/// - `Rw`：`obj.getattr(py_name)` 取值 → `ctx.set_field_interned`（若 has_expressions）
///   → `node.build(value, ...)`
/// - `Wo`：`obj.getattr(py_name)` 取值 → `node.build(value, ...)`（不写 context）
/// - `Ro`：通过 `compute_ro_value` 从节点逻辑计算（不从实例取值），写入 context
///   （若有表达式），然后 `node.build`（对 Tell/Computed 是 no-op）
///
/// 子节点 `Err` 时 `push_path_segment(rust_name)` 重建路径（仅错误路径）。
///
/// # sizeof
///
/// 累加所有字段 sizeof；任一字段返回 Err（如 `GreedyBytes`）则整体返回 Err。
#[derive(Debug)]
pub struct StructNode {
    /// 有序字段列表（携带模式信息）。
    fields: Vec<StructField>,
    /// 预计算的字段名 PyString 列表（构造时一次性 `clone_ref`）。
    ///
    /// parse/build 时通过 [`Context::set_field_names_ref`] 零拷贝引用此 cache，
    /// 避免每次 parse/build 的 `Vec` 分配 + N 次 `clone_ref`（节省 ~10-20ns/parse）。
    /// 与 [`StructNode::fields`] 顺序对齐。
    field_names_cache: Vec<Py<PyString>>,
    /// 用户类引用（parse 时 `create_class` 构造实例 + 错误信息中报告类名）。
    cls: Py<PyType>,
    /// 编译期检测：用户类是否定义了 `__post_init__`。
    has_post_init: bool,
    /// 该 Struct 是否含有表达式（决定 parse/build 入口用 `new_root` 还是 `placeholder`）。
    ///
    /// - `false` → `placeholder`（Phase 1 性能优化保留）
    /// - `true` → `new_root`（恢复 context 使用）
    has_expressions: bool,
}

impl StructNode {
    /// 创建一个 `StructNode`，包含给定的有序字段列表与用户类引用。
    ///
    /// 在构造时一次性预计算 `field_names_cache`（所有字段的 interned PyString
    /// `clone_ref`），parse/build 时通过 [`Context::set_field_names_ref`] 零拷贝引用，
    /// 避免每次 parse/build 的堆分配开销。
    ///
    /// # 参数
    ///
    /// - `py`：GIL token（用于 `clone_ref` 预计算 cache）。
    /// - `fields`：有序字段列表（`StructField` 已含 interned PyString + mode）。
    /// - `cls`：用户类 Python 引用（`@dataclass` 装饰的 StructMixin 子类）。
    /// - `has_post_init`：编译期检测用户类是否定义了 `__post_init__`。
    /// - `has_expressions`：该 Struct 是否含有表达式字段（决定 context 模式）。
    pub fn new(
        py: Python<'_>,
        fields: Vec<StructField>,
        cls: Py<PyType>,
        has_post_init: bool,
        has_expressions: bool,
    ) -> Self {
        // 预计算 field_names_cache：一次性 clone_ref 所有字段的 interned PyString。
        let field_names_cache = fields
            .iter()
            .map(|f| f.name.py_name().clone_ref(py))
            .collect();
        Self {
            fields,
            field_names_cache,
            cls,
            has_post_init,
            has_expressions,
        }
    }

    /// 测试专用构造器：用给定的字段名（非 interned）与默认 mock 类创建 StructNode。
    ///
    /// 自动 intern 传入的字段名，并创建一个最小 Python `object` 子类作为 `cls`，
    /// `has_post_init = false`，`has_expressions = false`，所有字段 `mode = Rw`。
    /// 供单元测试无需手工构造 `Py<PyType>` 与 `FieldName`（设计修订 §5.7.2 N6-R3 建议）。
    ///
    /// # 参数
    ///
    /// - `py`：GIL token。
    /// - `fields`：`(字段名字符串, 子节点)` 列表。
    #[cfg(test)]
    pub fn new_for_test(py: Python<'_>, fields: Vec<(String, Node)>) -> Self {
        let struct_fields = fields
            .into_iter()
            .map(|(name, node)| StructField {
                name: FieldName::new(py, name),
                node,
                mode: FieldMode::Rw,
            })
            .collect();
        let cls = py
            .eval_bound("type('MockStruct', (), {})", None, None)
            .expect("create mock class")
            .extract::<Py<PyType>>()
            .expect("extract Py<PyType>");
        Self::new(py, struct_fields, cls, false, false)
    }

    /// 返回字段数量。
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// 是否为空结构体（无字段）。
    ///
    /// 空结构体合法：parse(b'') → {}，build({}) → b''（见 §A.6）。
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// 返回字段列表的只读切片。
    pub fn fields(&self) -> &[StructField] {
        &self.fields
    }

    /// 返回用户类引用。
    pub fn cls(&self) -> &Py<PyType> {
        &self.cls
    }

    /// 返回该 Struct 是否含有表达式字段（决定 context 模式）。
    pub fn has_expressions(&self) -> bool {
        self.has_expressions
    }
}

impl Construct for StructNode {
    fn parse(
        &self,
        py: Python<'_>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 方案 B' 步骤 1-2：获取 dict 并逐字段解析。
        //
        // has_expressions=false（Phase 1 路径）：新建独立 PyDict，写入此 dict。
        //   ctx 为 placeholder（无 PyDict），不写 context。WO 字段仍按 mode 丢弃。
        //
        // has_expressions=true（Phase 2 路径）：复用 ctx 的 PyDict 作为 instance dict
        //   （零额外 PyDict 创建，设计 §5.5.2）。通过 ctx.set_field_interned 写入
        //   （每次调用独立借用 ctx.fields，与 node.parse 的 &mut ctx 不冲突——顺序调用）。
        //   最后 clone_ref ctx 的 dict 给实例。
        //
        // **mode 分支在两条路径中都生效**：WO 字段无论是否有表达式都仅消费字节。

        // 用于 force_setattr 的 dict（Py<PyAny>，拥有所有权）。
        let dict_for_instance: Py<PyAny> = if self.has_expressions {
            // Phase 2 表达式路径：使用 ctx 的 dict。
            // 设置 field_names 引用（零拷贝，引用预计算的 field_names_cache），
            // 供子节点（如 BytesNode 表达式长度）求值表达式使用。
            ctx.set_field_names_ref(&self.field_names_cache);
            for field in &self.fields {
                let value = match field.node.parse(py, stream, ctx, path) {
                    Ok(v) => v,
                    Err(mut e) => {
                        e.push_path_segment(field.name.rust_name());
                        return Err(e);
                    }
                };
                match field.mode {
                    FieldMode::Rw | FieldMode::Ro => {
                        // 写入 ctx 的 dict（同时充当 context 与 instance dict）。
                        ctx.set_field_interned(field.name.py_name(), value.bind(py), py)?;
                    }
                    FieldMode::Wo => {
                        // WO：仅消费字节，不写入 dict/context。
                        drop(value);
                    }
                }
            }
            // clone_ref ctx 的 dict 给实例（ctx 仍持有引用，parse 结束后随 ctx 丢弃）。
            // SAFETY: ctx.fields() 返回有效的 Bound<PyDict> 引用，as_ptr() 是有效的
            // Python 对象指针。from_borrowed_ptr 递增引用计数（等价于 clone_ref 语义）。
            // 持有 GIL（由 py 参数保证）。
            // MF-1 修复：不使用 expect()，而是返回 ExprContext 错误（防御性，
            // has_expressions=true 时 ctx 必有 PyDict，此分支理论上不触发）。
            let dict_ptr = ctx
                .fields()
                .ok_or_else(|| ConstructError::ExprContext {
                    message: "expression struct requires context with PyDict, but got placeholder"
                        .to_string(),
                    path: path.to_string(),
                })?
                .as_ptr();
            unsafe { Py::<PyAny>::from_borrowed_ptr(py, dict_ptr) }
        } else {
            // Phase 1 路径：新建独立 PyDict。
            let dict = PyDict::new_bound(py);
            for field in &self.fields {
                let value = match field.node.parse(py, stream, ctx, path) {
                    Ok(v) => v,
                    Err(mut e) => {
                        e.push_path_segment(field.name.rust_name());
                        return Err(e);
                    }
                };
                match field.mode {
                    FieldMode::Rw | FieldMode::Ro => {
                        // RW/RO：存入 instance dict。
                        // 裸 `?`：PyErr → ConstructError::Generic（From<PyErr>）。
                        dict.set_item(field.name.py_name().bind(py), value.bind(py))?;
                    }
                    FieldMode::Wo => {
                        // WO：仅消费字节，不存入 dict。
                        drop(value);
                    }
                }
            }
            dict.into_any().unbind()
        };

        // 方案 B' 步骤 3：tp_new 创建空实例（直接读 tp_new 槽位）。
        let instance = create_class(self.cls.bind(py)).map_err(|e| ConstructError::Generic {
            message: format!("failed to create instance via tp_new: {}", e),
            path: path.to_string(),
        })?;

        // 方案 B' 步骤 4：整体替换 __dict__（绕过自定义 __setattr__）。
        // 错误时附加 slots 提示（设计修订 §5.7.1 N4-R3 运行期兜底）。
        force_setattr(py, &instance, "__dict__", dict_for_instance).map_err(|e| {
            ConstructError::Generic {
                message: format!(
                    "failed to set __dict__ on instance: {}. \
                     该类可能使用了 __slots__ 或 @dataclass(slots=True)，\
                     Phase 1 不支持 slots dataclass。",
                    e
                ),
                path: path.to_string(),
            }
        })?;

        // 方案 B' 步骤 5：可选 __post_init__ 调用（仅 has_post_init=True 时）。
        if self.has_post_init {
            instance
                .call_method0("__post_init__")
                .map_err(|e| ConstructError::Generic {
                    message: format!("__post_init__ raised: {}", e),
                    path: path.to_string(),
                })?;
        }

        Ok(instance.unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 有表达式时：设置 field_names 引用（零拷贝），供子节点表达式求值使用。
        if self.has_expressions {
            ctx.set_field_names_ref(&self.field_names_cache);
        }
        for field in &self.fields {
            match field.mode {
                FieldMode::Rw => {
                    // RW：从实例 getattr 取值，写入 context（若有表达式），递归 build。
                    let value = obj.getattr(field.name.py_name().bind(py)).map_err(|e| {
                        ConstructError::Generic {
                            message: format!(
                                "object has no attribute '{}' (required for build): {}",
                                field.name.rust_name(),
                                e
                            ),
                            path: path.to_string(),
                        }
                    })?;
                    // 写入 context（供后续表达式引用）。
                    if self.has_expressions {
                        ctx.set_field_interned(field.name.py_name(), &value, py)?;
                    }
                    // P0-3：成功路径不 push/pop；子节点 Err 时重建路径。
                    match field.node.build(py, &value, stream, ctx, path) {
                        Ok(()) => {}
                        Err(mut e) => {
                            e.push_path_segment(field.name.rust_name());
                            return Err(e);
                        }
                    }
                }
                FieldMode::Wo => {
                    // WO：从实例 getattr 取值，递归 build。
                    // 不写入 context（编译期已禁止表达式引用 WO 字段，§3.4.4）。
                    let value = obj.getattr(field.name.py_name().bind(py)).map_err(|e| {
                        ConstructError::Generic {
                            message: format!(
                                "object has no attribute '{}' (required for build): {}",
                                field.name.rust_name(),
                                e
                            ),
                            path: path.to_string(),
                        }
                    })?;
                    match field.node.build(py, &value, stream, ctx, path) {
                        Ok(()) => {}
                        Err(mut e) => {
                            e.push_path_segment(field.name.rust_name());
                            return Err(e);
                        }
                    }
                }
                FieldMode::Ro => {
                    // RO：不从实例取值，通过节点自身逻辑计算（Tell/Computed/Const/ContextParam）。
                    // compute_ro_value 根据节点类型取值：
                    // - Tell：stream.tell()
                    // - Computed：eval_expr_int(expr, ctx)
                    // - Const/ContextParam：见 compute_ro_value 实现（Phase 3 扩展）
                    let value = field.node.compute_ro_value(py, stream, ctx, path)?;
                    let value_bound = value.bind(py);

                    // 写入 context（供后续表达式引用）
                    if self.has_expressions {
                        ctx.set_field_interned(field.name.py_name(), value_bound, py)?;
                    }

                    // build（对 sizeof=0 的节点如 Tell/Computed 是 no-op；
                    // 其他可能写字节，仍递归调用以处理）
                    match field.node.build(py, value_bound, stream, ctx, path) {
                        Ok(()) => {}
                        Err(mut e) => {
                            e.push_path_segment(field.name.rust_name());
                            return Err(e);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        let mut total = 0usize;
        for field in &self.fields {
            total = total.checked_add(field.node.sizeof(ctx)?).ok_or_else(|| {
                ConstructError::Generic {
                    message: "struct size overflowed usize".to_string(),
                    path: String::new(),
                }
            })?;
        }
        Ok(total)
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

    /// 构造 Int8ub 节点的便捷函数。
    fn u8_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    /// 构造 Int16ub 节点的便捷函数。
    fn u16_node() -> Node {
        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt16Big))
    }

    /// 构造 Rw StructField 的便捷函数（测试专用）。
    fn rw_field(py: Python<'_>, name: &str, node: Node) -> StructField {
        StructField {
            name: FieldName::new(py, name),
            node,
            mode: FieldMode::Rw,
        }
    }

    // ======================================================================
    // 空结构体
    // ======================================================================

    #[test]
    fn empty_struct_parse_empty_bytes_returns_empty_instance() {
        with_py(|py| {
            let node = StructNode::new_for_test(py, Vec::new());
            let mut stream = ParseStream::new(b"");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 返回的是用户类实例
            assert!(
                result
                    .bind(py)
                    .is_instance(node.cls().bind(py))
                    .expect("is_instance"),
                "result should be an instance of the user class"
            );
            // 实例的 __dict__ 应为空
            let d_binding = result
                .bind(py)
                .getattr("__dict__")
                .expect("getattr __dict__");
            let d = d_binding.downcast::<PyDict>().expect("is dict");
            assert_eq!(d.len(), 0);
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn empty_struct_parse_ignores_extra_bytes() {
        // 对齐 Python construct Struct：不要求消费全部输入。
        with_py(|py| {
            let node = StructNode::new_for_test(py, Vec::new());
            let mut stream = ParseStream::new(b"abc");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let d_binding = result
                .bind(py)
                .getattr("__dict__")
                .expect("getattr __dict__");
            let d = d_binding.downcast::<PyDict>().expect("is dict");
            assert_eq!(d.len(), 0);
            // 多余字节未被消费
            assert_eq!(stream.tell(), 0);
            assert!(!stream.is_at_end());
        });
    }

    #[test]
    fn empty_struct_build_empty_dict_returns_empty_bytes() {
        with_py(|py| {
            let node = StructNode::new_for_test(py, Vec::new());
            let obj = py.eval_bound("{}", None, None).expect("empty dict");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    // ======================================================================
    // 单字段结构体
    // ======================================================================

    #[test]
    fn single_field_parse_returns_instance_with_value() {
        with_py(|py| {
            let node = StructNode::new_for_test(py, vec![("x".to_string(), u8_node())]);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 验证属性可读
            let x: i64 = result
                .bind(py)
                .getattr("x")
                .expect("getattr x")
                .extract()
                .expect("extract i64");
            assert_eq!(x, 0x42);
        });
    }

    #[test]
    fn single_field_build_writes_bytes() {
        with_py(|py| {
            let node = StructNode::new_for_test(py, vec![("x".to_string(), u8_node())]);
            // 用简单 Python 对象 {x: 42}
            let obj = py
                .eval_bound("type('O', (), {'x': 42})()", None, None)
                .expect("create obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[42]);
        });
    }

    #[test]
    fn single_field_round_trip_preserves_value() {
        with_py(|py| {
            let node = StructNode::new_for_test(py, vec![("x".to_string(), u8_node())]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            // build
            let obj = py
                .eval_bound("type('O', (), {'x': 200})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            // parse
            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let x: i64 = result
                .bind(py)
                .getattr("x")
                .expect("getattr x")
                .extract()
                .expect("extract");
            assert_eq!(x, 200);
        });
    }

    // ======================================================================
    // 多字段结构体
    // ======================================================================

    #[test]
    fn multi_field_parse_returns_instance_with_all_values() {
        // Int8ub + Int16ub + Bytes(2)
        with_py(|py| {
            let node = StructNode::new_for_test(
                py,
                vec![
                    ("a".to_string(), u8_node()),
                    ("b".to_string(), u16_node()),
                    ("c".to_string(), Node::Bytes(BytesNode::new_const(2))),
                ],
            );
            // a=0x01, b=0x0203, c=0x0405
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04, 0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            let c_binding = inst.getattr("c").unwrap();
            let c: &[u8] = c_binding.extract().unwrap();
            assert_eq!(a, 1);
            assert_eq!(b, 0x0203);
            assert_eq!(c, &[0x04, 0x05]);
            assert!(stream.is_at_end());
        });
    }

    #[test]
    fn multi_field_build_writes_all_fields_in_order() {
        with_py(|py| {
            let node = StructNode::new_for_test(
                py,
                vec![
                    ("a".to_string(), u8_node()),
                    ("b".to_string(), u16_node()),
                    ("c".to_string(), Node::Bytes(BytesNode::new_const(2))),
                ],
            );
            let obj = py
                .eval_bound(
                    "type('O', (), {'a': 1, 'b': 0x0203, 'c': b'\\x04\\x05'})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x02, 0x03, 0x04, 0x05]);
        });
    }

    #[test]
    fn multi_field_parse_does_not_consume_extra_bytes() {
        with_py(|py| {
            let node = StructNode::new_for_test(
                py,
                vec![("a".to_string(), u8_node()), ("b".to_string(), u8_node())],
            );
            // 只消费 2 字节，多余 3 字节忽略
            let mut stream = ParseStream::new(&[0x10, 0x20, 0x30, 0x40, 0x50]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0x10);
            assert_eq!(b, 0x20);
            assert_eq!(stream.tell(), 2);
            assert!(!stream.is_at_end());
        });
    }

    // ======================================================================
    // build 缺字段 → 错误
    // ======================================================================

    #[test]
    fn build_missing_field_returns_generic_error_with_field_name() {
        with_py(|py| {
            let node = StructNode::new_for_test(
                py,
                vec![("a".to_string(), u8_node()), ("b".to_string(), u8_node())],
            );
            // 对象只有 a，缺 b
            let obj = py
                .eval_bound("type('O', (), {'a': 1})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("b"),
                        "error should mention field 'b': {}",
                        message
                    );
                }
                other => panic!("expected Generic error, got {:?}", other),
            }
            // a 已写入，b 未写入
            assert_eq!(stream.as_bytes(), &[1]);
        });
    }

    // ======================================================================
    // 嵌套 StructNode（StructNode 内嵌 StructNode）
    // ======================================================================

    #[test]
    fn nested_struct_parse_returns_nested_instance() {
        with_py(|py| {
            // 外层 { len: Int8ub, inner: Struct { value: Int8ub } }
            // 内层 StructNode 也走方案 B' 流程，构造内层实例
            let inner = Node::Struct(StructNode::new_for_test(
                py,
                vec![("value".to_string(), u8_node())],
            ));
            let outer = StructNode::new_for_test(
                py,
                vec![("len".to_string(), u8_node()), ("inner".to_string(), inner)],
            );
            let mut stream = ParseStream::new(&[0x03, 0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = outer
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let len: i64 = inst.getattr("len").unwrap().extract().unwrap();
            assert_eq!(len, 3);
            // inner 是内层 mock 类的实例
            let inner_inst = inst.getattr("inner").unwrap();
            let value: i64 = inner_inst.getattr("value").unwrap().extract().unwrap();
            assert_eq!(value, 0x42);
        });
    }

    #[test]
    fn nested_struct_build_writes_from_nested_instance() {
        with_py(|py| {
            let inner = Node::Struct(StructNode::new_for_test(
                py,
                vec![("value".to_string(), u8_node())],
            ));
            let outer = StructNode::new_for_test(
                py,
                vec![("len".to_string(), u8_node()), ("inner".to_string(), inner)],
            );
            // 构造嵌套实例对象
            let obj = py
                .eval_bound(
                    "type('O', (), {'len': 3, 'inner': type('I',(),{'value':0x42})()})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            outer
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x03, 0x42]);
        });
    }

    #[test]
    fn nested_struct_round_trip() {
        with_py(|py| {
            let inner = Node::Struct(StructNode::new_for_test(
                py,
                vec![("value".to_string(), u8_node())],
            ));
            let outer = StructNode::new_for_test(
                py,
                vec![
                    ("a".to_string(), u8_node()),
                    ("inner".to_string(), inner),
                    ("b".to_string(), u8_node()),
                ],
            );

            // build
            let obj = py
                .eval_bound(
                    "type('O', (), {'a': 0xAA, 'inner': type('I',(),{'value':0xBB})(), 'b': 0xCC})()",
                    None,
                    None,
                )
                .expect("obj");
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let mut stream = BuildStream::new();
            outer
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[0xAA, 0xBB, 0xCC]);

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let result = outer
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            assert_eq!(a, 0xAA);
            let inner_inst = inst.getattr("inner").unwrap();
            let value: i64 = inner_inst.getattr("value").unwrap().extract().unwrap();
            assert_eq!(value, 0xBB);
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(b, 0xCC);
        });
    }

    // ======================================================================
    // 错误 path 追踪
    // ======================================================================

    #[test]
    fn error_path_tracks_field_on_stream_error() {
        with_py(|py| {
            // { a: Int8ub, b: Int32ub }
            // b 读取时字节不足
            let node = StructNode::new_for_test(
                py,
                vec![
                    ("a".to_string(), u8_node()),
                    (
                        "b".to_string(),
                        Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big)),
                    ),
                ],
            );
            let mut stream = ParseStream::new(&[0x01]); // 只够 a，b 需 4 字节
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { path, message } => {
                    assert!(path.contains(".b"), "path should contain '.b': {}", path);
                    assert!(message.contains("expected 4"), "got: {}", message);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    #[test]
    fn error_path_tracks_nested_field() {
        with_py(|py| {
            // { a: Int8ub, inner: Struct { c: Int32ub } }
            let inner = Node::Struct(StructNode::new_for_test(
                py,
                vec![(
                    "c".to_string(),
                    Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big)),
                )],
            ));
            let outer = StructNode::new_for_test(
                py,
                vec![("a".to_string(), u8_node()), ("inner".to_string(), inner)],
            );
            // 只够 a（1 字节），inner.c 需要 4 字节
            let mut stream = ParseStream::new(&[0x01]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = outer
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Stream { path, message } => {
                    // path 应为 "root.inner.c"
                    assert!(
                        path.contains("inner"),
                        "path should contain 'inner': {}",
                        path
                    );
                    assert!(path.contains("c"), "path should contain 'c': {}", path);
                    assert!(message.contains("expected 4"), "got: {}", message);
                }
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_empty_struct_is_zero() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = StructNode::new_for_test(py, Vec::new());
            assert_eq!(node.sizeof(&ctx).unwrap(), 0);
        });
    }

    #[test]
    fn sizeof_sums_all_fields() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = StructNode::new_for_test(
                py,
                vec![
                    ("a".to_string(), u8_node()),                            // 1
                    ("b".to_string(), u16_node()),                           // 2
                    ("c".to_string(), Node::Bytes(BytesNode::new_const(4))), // 4
                ],
            );
            assert_eq!(node.sizeof(&ctx).unwrap(), 7);
        });
    }

    #[test]
    fn sizeof_with_greedy_bytes_returns_err() {
        with_py(|py| {
            use crate::nodes::greedy_bytes::GreedyBytesNode;
            let ctx = Context::new_root(py).expect("ctx");
            let node = StructNode::new_for_test(
                py,
                vec![
                    ("a".to_string(), u8_node()),
                    ("b".to_string(), Node::GreedyBytes(GreedyBytesNode::new())),
                ],
            );
            assert!(node.sizeof(&ctx).is_err());
        });
    }

    // ======================================================================
    // StructNode 访问器
    // ======================================================================

    #[test]
    fn len_and_is_empty() {
        with_py(|py| {
            let empty = StructNode::new_for_test(py, Vec::new());
            assert!(empty.is_empty());
            assert_eq!(empty.len(), 0);

            let nonempty = StructNode::new_for_test(py, vec![("x".to_string(), u8_node())]);
            assert!(!nonempty.is_empty());
            assert_eq!(nonempty.len(), 1);
        });
    }

    #[test]
    fn fields_returns_slice() {
        with_py(|py| {
            let node = StructNode::new_for_test(
                py,
                vec![("a".to_string(), u8_node()), ("b".to_string(), u16_node())],
            );
            let fields = node.fields();
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].name.rust_name(), "a");
            assert_eq!(fields[1].name.rust_name(), "b");
        });
    }

    // ======================================================================
    // 方案 B' 新增测试
    // ======================================================================

    #[test]
    fn parse_returns_instance_of_user_class() {
        // 方案 B' 核心断言：parse 返回用户类实例（不是 dict）。
        with_py(|py| {
            let node = StructNode::new_for_test(py, vec![("x".to_string(), u8_node())]);
            let mut stream = ParseStream::new(&[0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            assert!(
                result
                    .bind(py)
                    .is_instance(node.cls().bind(py))
                    .expect("is_instance"),
                "parse should return instance of user class"
            );
        });
    }

    #[test]
    fn parse_dict_keys_match_field_names() {
        // 验证写入 __dict__ 的 key 与编译期字段名一致（设计修订 §5.6.4 第 8 条）。
        with_py(|py| {
            let node = StructNode::new_for_test(
                py,
                vec![
                    ("alpha".to_string(), u8_node()),
                    ("beta".to_string(), u8_node()),
                ],
            );
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let d_binding = result
                .bind(py)
                .getattr("__dict__")
                .expect("getattr __dict__");
            let d = d_binding.downcast::<PyDict>().expect("is dict");
            assert_eq!(d.len(), 2);
            assert!(d.contains("alpha").expect("contains alpha"));
            assert!(d.contains("beta").expect("contains beta"));
        });
    }

    #[test]
    fn parse_dict_keys_are_interned_identity() {
        // 验证 __dict__ 的 key 是 interned（与 field_name.py_name 同一对象）。
        // 设计修订 §5.6.5 第 10 条。
        with_py(|py| {
            let node = StructNode::new_for_test(py, vec![("count".to_string(), u8_node())]);
            // 提取 field_name 的 py_name 用于后续比较
            let expected_key = node.fields()[0].name.py_name().clone_ref(py);

            let mut stream = ParseStream::new(&[0x10]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let d_binding = result.bind(py).getattr("__dict__").expect("dict");
            let d = d_binding.downcast::<PyDict>().expect("is dict");
            let actual_key = d.keys().into_iter().next().expect("at least one key");
            // identity 比较（is）—— interned 应共享同一对象
            assert!(
                actual_key.is(&expected_key),
                "dict key should be identical to interned field name (same object)"
            );
        });
    }

    #[test]
    fn parse_invokes_post_init_when_flag_set() {
        // 验证 has_post_init 标志触发 __post_init__ 调用（§5.6.3 第 5 条）。
        with_py(|py| {
            // 定义带 __post_init__ 的类
            let code = concat!(
                "class WithPostInit:\n",
                "    def __post_init__(self):\n",
                "        self.post_init_called = True\n",
            );
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None)
                .expect("define class");
            let cls = globals
                .get_item("WithPostInit")
                .expect("get_item ok")
                .expect("class exists")
                .extract::<Py<PyType>>()
                .expect("extract Py<PyType>");

            let fields = vec![rw_field(py, "x", u8_node())];
            let node = StructNode::new(py, fields, cls, true, false);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // __post_init__ 应已写入 post_init_called 属性
            let called: bool = result
                .bind(py)
                .getattr("post_init_called")
                .expect("getattr post_init_called")
                .extract()
                .expect("extract bool");
            assert!(called, "__post_init__ should have been called");
            // 字段也已写入
            let x: i64 = result
                .bind(py)
                .getattr("x")
                .expect("getattr x")
                .extract()
                .expect("extract i64");
            assert_eq!(x, 0x42);
        });
    }

    #[test]
    fn parse_does_not_invoke_post_init_when_flag_clear() {
        with_py(|py| {
            let code = concat!("class WithoutPostInit:\n", "    pass\n",);
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None)
                .expect("define class");
            let cls = globals
                .get_item("WithoutPostInit")
                .expect("get_item ok")
                .expect("class exists")
                .extract::<Py<PyType>>()
                .expect("extract Py<PyType>");

            let fields = vec![rw_field(py, "x", u8_node())];
            let node = StructNode::new(py, fields, cls, false, false);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 无 post_init_called 属性
            assert!(
                result.bind(py).getattr("post_init_called").is_err(),
                "no post_init_called attribute expected"
            );
        });
    }

    #[test]
    fn parse_frozen_dataclass_does_not_raise() {
        // 验证 frozen dataclass 可正常 parse（force_setattr 绕过 frozen 拦截）。
        // 设计修订 §5.6.3 第 7 条。
        with_py(|py| {
            let code = concat!(
                "from dataclasses import dataclass\n",
                "@dataclass(frozen=True)\n",
                "class Frozen:\n",
                "    x: int = 0\n",
                "    y: int = 0\n",
            );
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None)
                .expect("define Frozen");
            let cls = globals
                .get_item("Frozen")
                .expect("get_item ok")
                .expect("Frozen exists")
                .extract::<Py<PyType>>()
                .expect("extract");

            let fields = vec![rw_field(py, "x", u8_node()), rw_field(py, "y", u8_node())];
            let node = StructNode::new(py, fields, cls, false, false);
            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse should succeed on frozen dataclass");
            let x: i64 = result
                .bind(py)
                .getattr("x")
                .expect("getattr x")
                .extract()
                .expect("extract");
            let y: i64 = result
                .bind(py)
                .getattr("y")
                .expect("getattr y")
                .extract()
                .expect("extract");
            assert_eq!(x, 0xAA);
            assert_eq!(y, 0xBB);
        });
    }

    // ======================================================================
    // FieldMode 分支测试（Phase 2 子任务 2.4）
    // ======================================================================

    /// 构造 WO StructField 的便捷函数（测试专用）。
    fn wo_field(py: Python<'_>, name: &str, node: Node) -> StructField {
        StructField {
            name: FieldName::new(py, name),
            node,
            mode: FieldMode::Wo,
        }
    }

    /// 构造 RO StructField 的便捷函数（测试专用）。
    fn ro_field(py: Python<'_>, name: &str, node: Node) -> StructField {
        StructField {
            name: FieldName::new(py, name),
            node,
            mode: FieldMode::Ro,
        }
    }

    /// 创建 mock 类的便捷函数。
    fn mock_cls(py: Python<'_>) -> Py<PyType> {
        py.eval_bound("type('MockStruct', (), {})", None, None)
            .expect("create mock class")
            .extract::<Py<PyType>>()
            .expect("extract Py<PyType>")
    }

    #[test]
    fn wo_field_parse_consumes_bytes_but_not_stored_in_instance() {
        // WO 字段 parse 时消费字节，但不存入实例 dict。
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                wo_field(py, "pad", u8_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let mut stream = ParseStream::new(&[0x01, 0xFF, 0x02]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0x01);
            assert_eq!(b, 0x02);
            // pad 字段不应存在于实例中
            assert!(
                inst.getattr("pad").is_err(),
                "WO field 'pad' should not be in instance"
            );
            // 所有 3 字节都被消费
            assert_eq!(stream.tell(), 3);
        });
    }

    #[test]
    fn wo_field_build_reads_from_instance_and_writes_bytes() {
        // WO 字段 build 时从实例 getattr 取值并写字节（与 RW 行为一致）。
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node()), wo_field(py, "pad", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let obj = py
                .eval_bound("type('O', (), {'a': 0x01, 'pad': 0xFF})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0xFF]);
        });
    }

    #[test]
    fn ro_field_build_with_unsupported_node_returns_error() {
        // RO 字段使用不支持的节点（FormatField）→ compute_ro_value 返回 Generic error。
        // 编译期校验（§5.6）应保证 RO 只用 Tell/Computed/Const/ContextParam，
        // 此测试模拟运行时兜底场景。
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "computed", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let obj = py
                .eval_bound("type('O', (), {'a': 1, 'computed': 0})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("compute_ro_value") || message.contains("RO"),
                        "error should mention compute_ro_value/RO: {}",
                        message
                    );
                }
                other => panic!("expected Generic error, got {:?}", other),
            }
        });
    }

    #[test]
    fn ro_field_parse_stores_value_in_instance() {
        // RO 字段 parse 时正常解析并存入实例（与 RW 相同）。
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node()), ro_field(py, "c", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let mut stream = ParseStream::new(&[0x10, 0x20]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let c: i64 = inst.getattr("c").unwrap().extract().unwrap();
            assert_eq!(a, 0x10);
            assert_eq!(c, 0x20);
        });
    }

    // ======================================================================
    // has_expressions 标志测试
    // ======================================================================

    #[test]
    fn has_expressions_getter_returns_false_by_default() {
        with_py(|py| {
            let node = StructNode::new_for_test(py, vec![("x".to_string(), u8_node())]);
            assert!(!node.has_expressions());
        });
    }

    #[test]
    fn has_expressions_true_uses_new_root_context_for_parse() {
        // has_expressions=true 时，parse 路径使用 ctx 的 dict（new_root 创建）。
        // 验证：parse 结果正确 + ctx 的 dict 被填充（表达式求值可用）。
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node()), rw_field(py, "b", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 验证实例属性正确
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0x01);
            assert_eq!(b, 0x02);
            // 验证 ctx 的 dict 也被填充（表达式求值可用）
            let ctx_dict = ctx.fields().expect("ctx has dict");
            assert_eq!(ctx_dict.len(), 2);
            let ctx_a: i64 = ctx_dict.get_item("a").unwrap().unwrap().extract().unwrap();
            assert_eq!(ctx_a, 0x01);
        });
    }

    #[test]
    fn has_expressions_true_with_wo_field_skips_context_write() {
        // has_expressions=true + WO 字段：WO 不写入 ctx dict。
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                wo_field(py, "pad", u8_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let mut stream = ParseStream::new(&[0x01, 0xFF, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            // pad 不在实例中
            assert!(inst.getattr("pad").is_err());
            // ctx dict 只包含 RW 字段（a, b），不含 WO（pad）
            let ctx_dict = ctx.fields().expect("ctx has dict");
            assert_eq!(ctx_dict.len(), 2);
            assert!(ctx_dict.contains("a").unwrap());
            assert!(ctx_dict.contains("b").unwrap());
            assert!(!ctx_dict.contains("pad").unwrap());
        });
    }

    #[test]
    fn has_expressions_true_build_writes_to_context() {
        // has_expressions=true 时，build 的 RW 字段写入 ctx dict。
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node()), rw_field(py, "b", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let obj = py
                .eval_bound("type('O', (), {'a': 0x01, 'b': 0x02})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[0x01, 0x02]);
            // ctx dict 应被填充
            let ctx_dict = ctx.fields().expect("ctx has dict");
            assert_eq!(ctx_dict.len(), 2);
        });
    }

    #[test]
    fn has_expressions_false_build_does_not_write_context() {
        // has_expressions=false 时，build 不写入 ctx（placeholder 为 no-op）。
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let obj = py
                .eval_bound("type('O', (), {'a': 42})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[42]);
            // placeholder ctx 无 dict
            assert!(ctx.fields().is_none());
        });
    }

    #[test]
    fn wo_field_with_expressions_does_not_write_context_on_build() {
        // has_expressions=true + WO 字段 build：不写入 ctx dict。
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node()), wo_field(py, "pad", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let obj = py
                .eval_bound("type('O', (), {'a': 1, 'pad': 0xFF})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 0xFF]);
            // ctx dict 只含 RW 字段（a），不含 WO（pad）
            let ctx_dict = ctx.fields().expect("ctx has dict");
            assert_eq!(ctx_dict.len(), 1);
            assert!(ctx_dict.contains("a").unwrap());
            assert!(!ctx_dict.contains("pad").unwrap());
        });
    }

    // ======================================================================
    // RO 字段 build（Tell/Computed）：compute_ro_value 路径（Phase 2.6）
    // ======================================================================

    /// 构造 Tell 节点的便捷函数。
    fn tell_node() -> Node {
        Node::Tell(crate::nodes::tell::TellNode::new())
    }

    /// 构造 Computed 节点的便捷函数。
    fn computed_node(expr: crate::expr::ExprProgram) -> Node {
        Node::Computed(crate::nodes::computed::ComputedNode::new(expr))
    }

    #[test]
    fn ro_tell_field_build_records_stream_position() {
        // RO Tell 字段 build：不在实例中取值，从 stream.tell() 计算。
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "pos", tell_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            // 对象只有 RW 字段
            let obj = py
                .eval_bound("type('O', (), {'a': 0xAA, 'b': 0xBB})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // 写入两个字节 a, b（Tell 不写字节）
            assert_eq!(stream.as_bytes(), &[0xAA, 0xBB]);
        });
    }

    #[test]
    fn ro_tell_field_build_writes_to_context_when_has_expressions() {
        // has_expressions=true + RO Tell：build 时位置写入 ctx dict（供后续表达式引用）。
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "pos", tell_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let obj = py
                .eval_bound("type('O', (), {'a': 0x42})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            // ctx dict 应包含 a=0x42 和 pos=1（写完 a 字节后的位置）
            let ctx_dict = ctx.fields().expect("ctx has dict");
            assert_eq!(ctx_dict.len(), 2);
            let pos: i64 = ctx_dict
                .get_item("pos")
                .expect("get pos")
                .expect("pos exists")
                .extract()
                .expect("extract i64");
            assert_eq!(pos, 1);
        });
    }

    #[test]
    fn ro_computed_field_build_evaluates_expression() {
        // RO Computed 字段 build：求值表达式，不从实例取值。
        // Computed(a * 2) → [GetInt(0), Const(2), Mul]
        with_py(|py| {
            use crate::expr::{ExprOp, ExprProgram};
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "doubled", computed_node(prog)),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let obj = py
                .eval_bound("type('O', (), {'a': 21})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[21]); // Computed 不写字节
            let ctx_dict = ctx.fields().expect("ctx has dict");
            let doubled: i64 = ctx_dict
                .get_item("doubled")
                .expect("get doubled")
                .expect("doubled exists")
                .extract()
                .expect("extract i64");
            assert_eq!(doubled, 42);
        });
    }

    #[test]
    fn ro_tell_and_computed_round_trip_build_parse() {
        // 完整往返：{ a: Int8ub, pos: Tell(), b: Int8ub }
        // build 不写字节给 RO 字段；parse 时 Tell 返回流位置。
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "pos", tell_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);

            // build
            let obj = py
                .eval_bound("type('O', (), {'a': 0x11, 'b': 0x22})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[0x11, 0x22]);

            // parse
            let mut pstream = ParseStream::new(&bytes);
            let mut pctx = Context::placeholder(py);
            let mut ppath = Path::new();
            let result = node
                .parse(py, &mut pstream, &mut pctx, &mut ppath)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let pos: i64 = inst.getattr("pos").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0x11);
            assert_eq!(pos, 1); // 在读完 a 之后，pos = 1
            assert_eq!(b, 0x22);
        });
    }

    #[test]
    fn ro_computed_referencing_other_ro_field_build() {
        // 表达式引用前序 RO 字段（如 Computed(pos) 引用 Tell）。
        // 在 build 方向：Tell 先写入 ctx，然后 Computed 从 ctx 读取 pos。
        with_py(|py| {
            use crate::expr::{ExprOp, ExprProgram};
            // pos_at_b = pos (GetInt(1)，pos 是 field index 1)
            let prog = ExprProgram::new(vec![ExprOp::GetInt(1)]);
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "pos", tell_node()),
                ro_field(py, "pos_at_b", computed_node(prog)),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let obj = py
                .eval_bound("type('O', (), {'a': 7, 'b': 9})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[7, 9]);
            let ctx_dict = ctx.fields().expect("ctx has dict");
            // pos 应为 1（a 写完后），pos_at_b 应等于 pos=1
            let pos: i64 = ctx_dict
                .get_item("pos")
                .expect("get pos")
                .expect("exists")
                .extract()
                .expect("i64");
            let pos_at_b: i64 = ctx_dict
                .get_item("pos_at_b")
                .expect("get pos_at_b")
                .expect("exists")
                .extract()
                .expect("i64");
            assert_eq!(pos, 1);
            assert_eq!(pos_at_b, 1);
        });
    }

    #[test]
    fn ro_field_does_not_read_from_instance_on_build() {
        // RO 字段 build：不应从实例 getattr。
        // 即使实例没有该字段属性，build 也不应失败。
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "pos", tell_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            // 对象没有 'pos' 属性
            let obj = py
                .eval_bound("type('O', (), {'a': 5})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed without 'pos' attribute");
            assert_eq!(stream.as_bytes(), &[5]);
        });
    }
}
