//! StructNode：字段序列根节点。
//!
//! Python 参考：`construct/construct/core.py` `Struct._parse` / `_build`（L2162-2268）。
//!
//! ## 行为概述
//!
//! StructNode 是 StructMixin 子类执行树的根节点：按顺序解析/构建一组命名字段。
//!
//! - **parse（借用实例 `__dict__`）**：Rust 内完整构造用户类实例——
//!   [`crate::instance::create_class`] 创建空实例（实例自带空 `__dict__`）→
//!   `instance.getattr("__dict__")` 借用实例的 dict → 逐字段解析并直接 `set_item`
//!   写入实例 dict（不经过 `tp_setattro`，天然绕过 frozen 拦截）→ 可选调用
//!   `__post_init__`。返回最终的用户类实例（不是 dict），消除 Python 侧
//!   `cls(**dict)` O(N²) 开销，且无需 `force_setattr` 整体替换。
//! - **build**：逐字段从 Python 对象 `getattr` 取值，递归子节点构建，写入 stream。
//! - **sizeof**：累加所有字段 sizeof；任一字段返回 Err 则整体返回 Err。
//!
//! ## 实例 dict 的来源
//!
//! parse 借用实例自带的 `__dict__`（而非新建独立 dict 后整体替换）：
//! 语义不变（parse 返回用户类实例、兼容 frozen dataclass、绕过自定义
//! `__setattr__`），并消除中间 dict 创建与 `force_setattr` 固定开销
//! （合计 ~62-98ns/parse）。
//!
//! ## 路径追踪（成功路径零成本）
//!
//! 成功路径**不**调用 `path.push_field`/`path.pop`（消除每字段 `String` 堆分配）。
//! 子节点返回 `Err` 时，通过 [`ConstructError::push_path_segment`] 将当前字段名
//! 插入错误路径，重建完整路径（仅错误路径开销，对齐 pydantic-core 的
//! `validation_state.rs` 设计）。
//!
//! ## 关于 Context 的嵌套
//!
//! parse 路径中 StructNode 自行管理 dict：先创建实例，inject 实例 `__dict__`
//! 到 ctx（has_expressions=true 时），随后 `set_field_at` 同时写 dict 与
//! expr_values_buf。无表达式的 Struct 不读写 ctx（直接操作实例 dict）。
//!
//! build 方向不变：有表达式时 `new_root` 创建独立 dict 作为表达式求值的临时存储。

use crate::context::Context;
use crate::error::ConstructError;
use crate::instance::{
    create_class, dict_via_generic_getdict, intern_pystring, set_item_knownhash,
};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyString, PyType};

use super::{Construct, Node};

/// 编译期缓存的字段名：包含 interned PyString（C API 快速比较）、Rust 侧 String、
/// 以及 interned key 的 PyObject_Hash 缓存值。
///
/// - `py_name`：interned `Py<PyString>`，parse 时作为 dict key（避免每次创建 str
///   + 计算 hash）、build 时作为 `getattr` 参数（interned 可命中 method cache）。
/// - `rust_name`：Rust 侧字符串副本，用于 path 追踪与错误信息。
/// - `cached_hash`：编译期一次性 `PyObject_Hash` 计算并缓存的 hash 值。interned
///   PyString 的 hash 在其生命周期内不变（CPython 强约束），用于
///   `_PyDict_SetItem_KnownHash` 跳过运行期 hash 重算。
#[derive(Debug)]
pub struct FieldName {
    /// 缓存的 interned Python 字符串引用。
    py_name: Py<PyString>,
    /// Rust 侧字段名（用于 path、错误信息）。
    rust_name: String,
    /// 编译期缓存的 PyObject_Hash 结果。
    ///
    /// interned PyString 的 hash 在其生命周期内不变（CPython 强约束：
    /// interned 字符串不可变 + hash 字段 once-cached）。`FieldName::new` 构造时
    /// 一次性 `PyObject_Hash` 计算并缓存，供 [`set_item_knownhash`] 使用。
    cached_hash: ffi::Py_hash_t,
}

impl FieldName {
    /// 创建字段名。`py` 用于创建 interned PyString + 一次性计算 hash。
    ///
    /// # 参数
    ///
    /// - `py`：GIL token。
    /// - `name`：字段名字符串。
    ///
    /// # Panic
    ///
    /// `PyObject_Hash` 返回 -1 时 panic（构造期错误，比运行期退化更明确——
    /// 避免缓存 -1 hash 传给 `_PyDict_SetItem_KnownHash` 触发 dict 内部
    /// hash 冲突路径性能退化）。interned PyString 理论上 hash 不会失败
    /// （CPython 内部 hash 算法对任意 Unicode 字符串均有效），此 panic
    /// 仅在 OOM 等极端场景触发。
    pub fn new(py: Python<'_>, name: impl Into<String>) -> Self {
        let rust_name = name.into();
        let py_name = intern_pystring(py, &rust_name);
        // SAFETY: py_name 是有效的 PyString 指针，PyObject_Hash 是 CPython Stable ABI。
        // interned PyString hash 在其生命周期内不变（CPython 强约束）。
        let cached_hash = unsafe { ffi::PyObject_Hash(py_name.as_ptr()) };
        // PyObject_Hash 返回 -1 表示失败（异常已挂起）。
        // interned PyString 理论不会失败，但若发生则 panic（构造期错误）。
        if cached_hash == -1 {
            let err = PyErr::fetch(py);
            panic!(
                "FieldName::new: PyObject_Hash failed for interned string {:?}: {:?}",
                rust_name, err
            );
        }
        Self {
            py_name,
            rust_name,
            cached_hash,
        }
    }

    /// 返回 interned Python 字符串引用（`Py<PyString>`，拥有所有权）。
    pub fn py_name(&self) -> &Py<PyString> {
        &self.py_name
    }

    /// 返回 Rust 侧字段名（用于 path 追踪与错误信息）。
    pub fn rust_name(&self) -> &str {
        &self.rust_name
    }

    /// 返回编译期缓存的 PyObject_Hash 值（供 [`set_item_knownhash`] 使用）。
    ///
    /// interned PyString hash 在其生命周期内不变，多次调用返回相同值。
    pub fn cached_hash(&self) -> ffi::Py_hash_t {
        self.cached_hash
    }
}

/// 字段模式：RW（读写）、RO（只读）、WO（只写）。
///
/// - `Rw`：读写——build 从实例取值，parse 存入实例。
/// - `Ro`：只读——build 自动计算（不从实例取值），parse 存入实例。
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
/// 携带 `mode`（RW/RO/WO），供 parse/build 路径按模式分支处理。
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
/// # parse 行为（借用实例 `__dict__`）
///
/// 1. [`create_class`]：`tp_new(cls, (), NULL)` 创建空实例（实例自带空 `__dict__`）。
/// 2. `instance.getattr(interned "__dict__")`：借用实例自带的 `__dict__`（非新建），
///    downcast 为 `PyDict`。slots 类此处失败 → 返回错误。
/// 3. 对每个 `StructField`：
///    - `field.node.parse(...)` —— 递归解析子节点
///    - 根据 `field.mode` 分支：
///      - `Rw`/`Ro`：写入实例 dict（dict 即实例的 `__dict__`）
///      - `Wo`：`drop(value)` —— 仅消费字节，不存入 dict
///    - 子节点 `Err` 时 `push_path_segment(rust_name)` 重建路径（仅错误路径）
/// 4. 若 `has_post_init`：调用 `instance.__post_init__()`（dict 填充之后）。
/// 5. 返回实例（无需 `force_setattr`——dict 本就是实例的 `__dict__`）。
///
/// `has_expressions` 控制 dict 填充方式：
/// - `false`：直接 `dict.set_item`（不经过 context）。
/// - `true`：`ctx.inject_fields(dict)` 让 Context 使用此 dict，
///   通过 `ctx.set_field_at` 同步写入 expr_values_buf（供 GetInt 按索引快速取值）。
///
/// `PyDict_SetItem` 直接操作 dict 的内部哈希表，不触发 `tp_setattro`（即不调用
/// `__setattr__`），因此天然绕过 frozen dataclass 的 `FrozenInstanceError` 拦截。
///
/// 成功路径**不维护 Path**（零 String 分配）。
///
/// parse **不要求消费全部输入**：多余字节被忽略（对齐 Python construct Struct 语义）。
///
/// # build 行为
///
/// 对每个 `StructField`，根据 `field.mode` 分支：
/// - `Rw`：`obj.getattr(py_name)` 取值 → `ctx.set_field_at`（若 has_expressions）
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
    /// 用户类引用（parse 时 `create_class` 构造实例 + 错误信息中报告类名）。
    cls: Py<PyType>,
    /// 编译期检测：用户类是否定义了 `__post_init__`。
    has_post_init: bool,
    /// 该 Struct 是否含有表达式（决定 parse/build 入口用 `new_root` 还是 `placeholder`）。
    ///
    /// - `false` → `placeholder`（性能优化）
    /// - `true` → `new_root`（恢复 context 使用）
    has_expressions: bool,
    /// interned `"__dict__"`（getattr fallback 路径复用）。
    ///
    /// 主路径改用 `dict_via_generic_getdict`，此字段仅供 GenericGetDict
    /// 失败时的 fallback getattr 路径使用。
    /// 由于 `"__dict__"` 是 CPython 内部高频使用的字符串，实际开销接近零
    /// （命中 interned 池，仅一次指针比较）。
    dict_attr_name: Py<PyString>,
}

impl StructNode {
    /// 创建一个 `StructNode`，包含给定的有序字段列表与用户类引用。
    ///
    /// # 参数
    ///
    /// - `py`：GIL token。
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
        Self {
            fields,
            cls,
            has_post_init,
            has_expressions,
            dict_attr_name: intern_pystring(py, "__dict__"),
        }
    }

    /// 测试专用构造器：用给定的字段名（非 interned）与默认 mock 类创建 StructNode。
    ///
    /// 自动 intern 传入的字段名，并创建一个最小 Python `object` 子类作为 `cls`，
    /// `has_post_init = false`，`has_expressions = false`，所有字段 `mode = Rw`。
    /// 供单元测试无需手工构造 `Py<PyType>` 与 `FieldName`。
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
    /// 空结构体合法：parse(b'') → {}，build({}) → b''。
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
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 步骤 1：先创建实例（tp_new，实例自带空 __dict__）。
        // 错误路径：实例创建失败时无 dict 需清理，Rust RAII 保证安全。
        let instance = create_class(self.cls.bind(py)).map_err(|e| ConstructError::Generic {
            message: format!("failed to create instance via tp_new: {}", e),
            path: path.to_string(),
        })?;

        // 步骤 2：获取实例的 __dict__（借用实例自带的空 dict，非新建）。
        // 直接调 PyObject_GenericGetDict，绕过 pyo3
        // getattr 包装（MRO + 描述符 dispatch + Bound 构造 + downcast）。
        // GenericGetDict 是 CPython Stable ABI（since 3.10），正确处理 Python 3.13
        // managed dict + lazy materialize。
        //
        // 退化路径：GenericGetDict 失败（NULL / 非 PyDict / slots class）
        // 时 fallback 到 getattr 路径，保留明确错误信息。
        let dict_bound = match dict_via_generic_getdict(py, &instance) {
            Ok(d) => d,
            Err(_) => {
                // Fallback：保留 getattr 路径（slots class / __dict__ override /
                // GenericGetDict 抛 Python 异常等异常场景）。
                instance
                    .getattr(self.dict_attr_name.bind(py))
                    .map_err(|e| ConstructError::Generic {
                        message: format!(
                            "failed to get __dict__ from instance: {}. \
                             该类可能使用了 __slots__ 或 @dataclass(slots=True)，\
                             不支持 slots dataclass。",
                            e
                        ),
                        path: path.to_string(),
                    })?
                    .downcast_into::<PyDict>()
                    .map_err(|_| ConstructError::Generic {
                        message: "instance __dict__ is not a dict (possible __slots__ class)"
                            .into(),
                        path: path.to_string(),
                    })?
            }
        };

        // 步骤 3：根据 has_expressions 选择填充路径。
        if self.has_expressions {
            // 表达式路径：将实例 dict 注入 ctx，通过 set_field_at 同步
            // expr_values_buf（供 GetInt 按索引快速取值）。
            ctx.inject_fields(dict_bound);
            ctx.init_expr_values(self.fields.len());

            for (idx, field) in self.fields.iter().enumerate() {
                let value = match field.node.parse(py, stream, ctx, path) {
                    Ok(v) => v,
                    // StopField 捕获：StopIf 在 Struct 字段中触发时，
                    // 停止后续字段，正常返回当前实例（已解析字段在 dict 中，
                    // 未解析字段不写入——对齐 Python Struct._parse 的 except StopFieldError）。
                    Err(ConstructError::StopField { .. }) => break,
                    Err(mut e) => {
                        e.push_path_segment(field.name.rust_name());
                        return Err(e);
                    }
                };
                match field.mode {
                    FieldMode::Rw | FieldMode::Ro => {
                        // 写入 ctx 的 fields（即实例 dict）+ expr_values_buf。
                        // 用 KnownHash 跳过 hash 重算。
                        ctx.set_field_at_knownhash(
                            idx,
                            field.name.py_name(),
                            value.bind(py),
                            field.name.cached_hash(),
                            py,
                        )?;
                    }
                    FieldMode::Wo => {
                        // WO：仅消费字节，不写入 dict/context。
                        // expr_values_buf[idx] 保持 null（GetInt 引用 WO 字段为编译期错误）。
                        drop(value);
                    }
                }
            }
            // dict 已在 ctx.fields 中，随 ctx 存活，无需 take_fields。
        } else {
            // 无表达式路径：直接操作 dict（不经过 ctx）。
            // PyDict_SetItem 不触发 __setattr__，天然绕过 frozen dataclass 拦截。
            // 用 _PyDict_SetItem_KnownHash 跳过
            // interned key 的运行期 hash 重算。FieldName::new 已缓存 PyObject_Hash。
            for field in &self.fields {
                let value = match field.node.parse(py, stream, ctx, path) {
                    Ok(v) => v,
                    // StopField 捕获：停止后续字段。
                    Err(ConstructError::StopField { .. }) => break,
                    Err(mut e) => {
                        e.push_path_segment(field.name.rust_name());
                        return Err(e);
                    }
                };
                match field.mode {
                    FieldMode::Rw | FieldMode::Ro => {
                        set_item_knownhash(
                            py,
                            &dict_bound,
                            field.name.py_name().bind(py),
                            value.bind(py),
                            field.name.cached_hash(),
                        )?;
                    }
                    FieldMode::Wo => {
                        drop(value);
                    }
                }
            }
        }

        // 步骤 4：可选 __post_init__（在 dict 填充之后）。
        if self.has_post_init {
            instance
                .call_method0("__post_init__")
                .map_err(|e| ConstructError::Generic {
                    message: format!("__post_init__ raised: {}", e),
                    path: path.to_string(),
                })?;
        }

        // 步骤 5：返回实例（无需 force_setattr——dict 本就是实例的 __dict__）。
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
        // 有表达式时：初始化 expr_values_buf（栈分配内联数组，零堆开销），供子节点表达式求值使用。
        if self.has_expressions {
            ctx.init_expr_values(self.fields.len());
        }
        for (idx, field) in self.fields.iter().enumerate() {
            match field.mode {
                FieldMode::Rw | FieldMode::Wo => {
                    // RW/WO：从实例 getattr 取值，递归 build。
                    // 区别仅在于 RW 在 has_expressions 时额外写入 context（供后续表达式引用）。
                    // WO 不写入 context（编译期已禁止表达式引用 WO 字段）。
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
                    // 仅 RW 字段写入 context（WO 字段不参与表达式引用）。
                    if matches!(field.mode, FieldMode::Rw) && self.has_expressions {
                        ctx.set_field_at(idx, field.name.py_name(), &value, py)?;
                    }
                    // 成功路径不 push/pop；子节点 Err 时重建路径。
                    match field.node.build(py, &value, stream, ctx, path) {
                        Ok(()) => {}
                        // StopField 捕获：StopIf 在 build 方向
                        // 同样停止后续字段。对齐 Python Struct._build 的 except StopFieldError。
                        Err(ConstructError::StopField { .. }) => break,
                        Err(mut e) => {
                            e.push_path_segment(field.name.rust_name());
                            return Err(e);
                        }
                    }
                }
                FieldMode::Ro => {
                    // RO：不从实例取值，通过节点自身逻辑计算（Tell/Computed/Const/ContextParam）。
                    let value = field.node.compute_ro_value(py, stream, ctx, path)?;
                    let value_bound = value.bind(py);

                    // 写入 context（供后续表达式引用）
                    if self.has_expressions {
                        ctx.set_field_at(idx, field.name.py_name(), value_bound, py)?;
                    }

                    // build（对 sizeof=0 的节点如 Tell/Computed 是 no-op；
                    // 其他可能写字节，仍递归调用以处理）
                    match field.node.build(py, value_bound, stream, ctx, path) {
                        Ok(()) => {}
                        // StopField 捕获：RO 路径同样支持（虽然 StopIf 通常不用 RO）。
                        Err(ConstructError::StopField { .. }) => break,
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
            // 内层 StructNode 同样构造内层实例
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
    // parse 返回用户类实例
    // ======================================================================

    #[test]
    fn parse_returns_instance_of_user_class() {
        // 核心断言：parse 返回用户类实例（不是 dict）。
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
        // 验证写入 __dict__ 的 key 与编译期字段名一致。
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
        // 验证 has_post_init 标志触发 __post_init__ 调用。
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
        // 验证：frozen dataclass 可正常 parse。
        // PyDict_SetItem 直接操作 dict，不经过 tp_setattro（即不触发 __setattr__），
        // 天然绕过 frozen dataclass 的 FrozenInstanceError 拦截。
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

    #[test]
    fn parse_slots_class_returns_error_mentioning_dict_or_slots() {
        // slots 类无 __dict__，getattr("__dict__") 失败 → 返回错误。
        // 错误消息应提示 __dict__ 或 slots，便于用户定位问题。
        with_py(|py| {
            let code = "class WithSlots:\n    __slots__ = ('x',)\n";
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None)
                .expect("define WithSlots");
            let cls = globals
                .get_item("WithSlots")
                .expect("get_item ok")
                .expect("class exists")
                .extract::<Py<PyType>>()
                .expect("extract");

            let fields = vec![rw_field(py, "x", u8_node())];
            let node = StructNode::new(py, fields, cls, false, false);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("slots class should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("__dict__") || message.contains("slots"),
                        "error should mention __dict__/slots: {}",
                        message
                    );
                }
                other => panic!("expected Generic error for slots class, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // FieldMode 分支测试
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
        // 编译期校验应保证 RO 只用 Tell/Computed/Const/ContextParam，
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
    fn ro_terminated_field_build_is_noop() {
        // Terminated 作为 RO 字段时 compute_ro_value 返回 Py_None，
        // 随后 TerminatedNode.build 是 no-op（不写字节，不从 obj 取值）。
        with_py(|py| {
            let terminated_node = Node::Terminated(crate::nodes::terminated::TerminatedNode::new());
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "t", terminated_node),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            // obj 只有 RW 字段 'a'，无需为 RO Terminated 提供 't' 属性
            let obj = py
                .eval_bound("type('O', (), {'a': 0x42})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed");
            // 只写字段 a 的字节（Terminated 是 no-op）
            assert_eq!(stream.as_bytes(), &[0x42]);
        });
    }

    #[test]
    fn ro_default_field_build_uses_constant_value() {
        // Default(Byte, 0) 作为 RO 字段时 compute_ro_value 求值
        // value 表达式（Const(0) → PyLong(0)），随后 DefaultNode.build 转发 inner。
        with_py(|py| {
            let inner = u8_node();
            let value = crate::expr::ExprProgram::new(vec![crate::expr::ExprOp::Const(0)]);
            let default_node =
                Node::Default(crate::nodes::default_node::DefaultNode::new(inner, value));
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "d", default_node),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            // obj 只有 RW 字段 'a'，Default 的值由 compute_ro_value 计算
            let obj = py
                .eval_bound("type('O', (), {'a': 0x07})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed");
            // 字段 a=0x07，Default 值=0x00
            assert_eq!(stream.as_bytes(), &[0x07, 0x00]);
        });
    }

    #[test]
    fn ro_const_field_build_uses_constant_value() {
        // Const(5, Int8ub) 作为 RO 字段时 compute_ro_value 返回常量值 5
        // （v0.1.2 B1），随后 ConstNode.build 收到 obj == value → inner.build(5)。
        // rfield(Const(...)) 场景：用户无需为该字段提供实例属性。
        with_py(|py| {
            let const_node = crate::nodes::const_node::ConstNode::new(u8_node(), 5i64.into_py(py));
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "c", Node::Const(const_node)),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            // obj 只有 RW 字段 'a'，Const 的值由 compute_ro_value 提供
            let obj = py
                .eval_bound("type('O', (), {'a': 0x07})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed");
            // 字段 a=0x07，Const 值=0x05
            assert_eq!(stream.as_bytes(), &[0x07, 0x05]);
        });
    }

    #[test]
    fn ro_padding_field_build_writes_pattern() {
        // Padding(2) 作为 RO 字段时 compute_ro_value 返回 Py_None（占位），
        // PaddingNode.build 忽略 obj 写 pattern 字节（v0.1.2 B1 同族）。
        // rfield(Padding(...)) 是 README/docstring 宣称的用法。
        with_py(|py| {
            let padding_node = crate::nodes::padding::PaddingNode::new_const(2, 0x00);
            let fields = vec![
                rw_field(py, "a", u8_node()),
                ro_field(py, "p", Node::Padding(padding_node)),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let obj = py
                .eval_bound("type('O', (), {'a': 0x01, 'b': 0x02})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed");
            // a=0x01 + 2 字节 pad + b=0x02
            assert_eq!(stream.as_bytes(), &[0x01, 0x00, 0x00, 0x02]);
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
    fn has_expressions_true_injects_instance_dict_into_context() {
        // has_expressions=true 时，parse 先创建实例，借用实例 __dict__ 注入 ctx。
        // parse 后 ctx.fields() 为 Some（持有实例 __dict__ 的引用），不再 take。
        // 验证：实例属性正确 + ctx.fields() 为 Some + ctx dict 与实例 __dict__ 同一对象。
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node()), rw_field(py, "b", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            // 统一用 placeholder（生产路径 _parse_raw 也用 placeholder）
            let mut ctx = Context::placeholder(py);
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
            // inject 后 ctx.fields() 为 Some（不再 take_fields）
            let ctx_dict = ctx
                .fields()
                .expect("ctx should hold instance dict after inject_fields");
            assert_eq!(ctx_dict.len(), 2);
            // 验证 ctx 的 dict 就是实例的 __dict__（同一对象，借用语义）
            let inst_dict = inst.getattr("__dict__").unwrap();
            assert!(
                ctx_dict.as_ptr() == inst_dict.as_ptr(),
                "ctx dict should be the instance's __dict__ (same object)"
            );
        });
    }

    #[test]
    fn has_expressions_true_with_wo_field_skips_context_write() {
        // has_expressions=true + WO 字段：WO 不写入实例 dict（不 set_field_at）。
        // dict 被 inject 到 ctx，parse 后 ctx.fields() 为 Some（实例 __dict__），
        // 通过实例 __dict__ 验证 WO 字段不存在。
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
            // 实例 __dict__ 只包含 RW 字段（a, b），不含 WO（pad）
            let inst_dict = inst.getattr("__dict__").unwrap();
            let dict = inst_dict.downcast::<PyDict>().unwrap();
            assert_eq!(dict.len(), 2);
            assert!(dict.contains("a").unwrap());
            assert!(dict.contains("b").unwrap());
            assert!(!dict.contains("pad").unwrap());
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
    // RO 字段 build（Tell/Computed）：compute_ro_value 路径
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

    // ======================================================================
    // StopField 捕获
    // ======================================================================

    /// 构造 StopIfNode（常量 Always）的便捷函数。
    fn stop_if_always_node() -> Node {
        Node::StopIf(crate::nodes::stop_if::StopIfNode::new(
            crate::nodes::stop_if::StopIfCondition::Always,
        ))
    }

    /// 构造 StopIfNode（常量 Never）的便捷函数。
    fn stop_if_never_node() -> Node {
        Node::StopIf(crate::nodes::stop_if::StopIfNode::new(
            crate::nodes::stop_if::StopIfCondition::Never,
        ))
    }

    /// 构造 RO 模式的 StopIf StructField（StopIf 作为 RO 字段，不从实例取值）。
    fn stop_field(py: Python<'_>, name: &str, stop_node: Node) -> StructField {
        StructField {
            name: FieldName::new(py, name),
            node: stop_node,
            mode: FieldMode::Ro,
        }
    }

    #[test]
    fn parse_stop_if_always_stops_subsequent_fields_no_expr_path() {
        // Struct 内：StopIf(Always) 触发 → 后续字段不解析
        // has_expressions=false 路径
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                // StopIf(Always) 作为 RO 字段（不从实例取值，build 时检查条件）
                stop_field(py, "stop", stop_if_always_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            // 数据 2 字节，但只应解析 a（b 被跳过）
            let mut stream = ParseStream::new(&[0x10, 0x20]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse should succeed");
            let inst = result.bind(py);
            // a 已解析
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            assert_eq!(a, 0x10);
            // b 不应被解析（属性不存在）
            assert!(
                inst.getattr("b").is_err(),
                "b should not be parsed after StopIf"
            );
            // 流位置应只前进了 1 字节（a 解析，StopIf 不消耗字节）
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_stop_if_always_stops_subsequent_fields_expr_path() {
        // 同上，has_expressions=true 路径
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                stop_field(py, "stop", stop_if_always_node()),
                rw_field(py, "b", u8_node()),
            ];
            // has_expressions=true：触发 init_expr_values 路径
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let mut stream = ParseStream::new(&[0x10, 0x20]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse should succeed");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            assert_eq!(a, 0x10);
            assert!(
                inst.getattr("b").is_err(),
                "b should not be parsed after StopIf"
            );
            assert_eq!(stream.tell(), 1);
        });
    }

    #[test]
    fn parse_stop_if_never_does_not_stop() {
        // StopIf(Never) 不停止，后续字段正常解析
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                stop_field(py, "stop", stop_if_never_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let mut stream = ParseStream::new(&[0x10, 0x20]);
            let mut ctx = Context::placeholder(py);
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
        });
    }

    #[test]
    fn parse_stop_if_at_first_field_stops_immediately() {
        // StopIf 作为第一个字段：实例 dict 为空
        with_py(|py| {
            let fields = vec![
                stop_field(py, "stop", stop_if_always_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let mut stream = ParseStream::new(&[0x99]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            // b 不应存在
            assert!(inst.getattr("b").is_err());
            // 流未消耗
            assert_eq!(stream.tell(), 0);
            // 实例 __dict__ 应为空
            let d = inst.getattr("__dict__").unwrap();
            let d = d.downcast::<PyDict>().unwrap();
            assert_eq!(d.len(), 0);
        });
    }

    #[test]
    fn build_stop_if_always_stops_subsequent_fields() {
        // build 方向：StopIf(Always) 停止后续字段写入
        // StopIf 作为 RO 字段，不从实例 getattr 'stop'
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                stop_field(py, "stop", stop_if_always_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            // 对象只有 a 和 b 属性（不需要 stop——StopIf 是 RO 字段）
            let obj = py
                .eval_bound("type('O', (), {'a': 1, 'b': 2})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build should succeed");
            // 只应写入 a（b 被 StopIf 跳过）
            assert_eq!(stream.as_bytes(), &[1]);
        });
    }

    #[test]
    fn build_stop_if_never_does_not_stop() {
        // build 方向：StopIf(Never) 不停止，后续字段正常 build
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                stop_field(py, "stop", stop_if_never_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let obj = py
                .eval_bound("type('O', (), {'a': 1, 'b': 2})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 2]);
        });
    }

    #[test]
    fn parse_round_trip_with_stop_if_never() {
        // round-trip：StopIf(Never) 不影响数据保存
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                stop_field(py, "stop", stop_if_never_node()),
                rw_field(py, "b", u8_node()),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);

            // build
            let obj = py
                .eval_bound("type('O', (), {'a': 0xAA, 'b': 0xBB})()", None, None)
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();
            assert_eq!(bytes, &[0xAA, 0xBB]);

            // parse 回来
            let mut pstream = ParseStream::new(&bytes);
            let mut pctx = Context::placeholder(py);
            let mut ppath = Path::new();
            let result = node
                .parse(py, &mut pstream, &mut pctx, &mut ppath)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xAA);
            assert_eq!(b, 0xBB);
        });
    }

    #[test]
    fn parse_other_errors_still_propagate() {
        // 非 StopField 错误仍应正常向上传播（不被 StopField 捕获分支吞掉）
        with_py(|py| {
            let fields = vec![
                rw_field(py, "a", u8_node()),
                // 第二字段需要 4 字节但只有 1 字节 → Stream 错误
                rw_field(
                    py,
                    "b",
                    Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt32Big)),
                ),
            ];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let mut stream = ParseStream::new(&[0x10]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail with Stream error");
            // 必须是 Stream 错误，不是被吞掉
            match err {
                ConstructError::Stream { .. } => {}
                other => panic!("expected Stream error, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // FieldName cached_hash + GenericGetDict / KnownHash 路径
    // ======================================================================

    #[test]
    fn fieldname_cached_hash_is_correct() {
        // FieldName::new 缓存的 hash 应等于 Python PyObject_Hash 的返回值。
        with_py(|py| {
            let name = FieldName::new(py, "my_field");
            // 直接调 PyObject_Hash 重新计算（独立验证）。
            let expected = unsafe { pyo3::ffi::PyObject_Hash(name.py_name().as_ptr()) };
            assert_ne!(expected, -1, "PyObject_Hash should not fail");
            assert_eq!(
                name.cached_hash(),
                expected,
                "cached_hash should equal fresh hash"
            );
        });
    }

    #[test]
    fn fieldname_cached_hash_stable_across_instances() {
        // 同名 FieldName 的 cached_hash 应相同（interned string 共享 hash）。
        with_py(|py| {
            let a = FieldName::new(py, "alpha");
            let b = FieldName::new(py, "alpha");
            assert_eq!(a.cached_hash(), b.cached_hash());
        });
    }

    #[test]
    fn fieldname_cached_hash_distinct_for_distinct_names() {
        with_py(|py| {
            let a = FieldName::new(py, "alpha");
            let b = FieldName::new(py, "beta");
            assert_ne!(a.cached_hash(), b.cached_hash());
        });
    }

    #[test]
    fn fieldname_cached_hash_unicode_works() {
        // Unicode 字段名（含中日韩字符 + emoji）也能正确计算 hash。
        with_py(|py| {
            let name = FieldName::new(py, "字段αβγ✨");
            let expected = unsafe { pyo3::ffi::PyObject_Hash(name.py_name().as_ptr()) };
            assert_ne!(expected, -1, "Unicode hash should not fail");
            assert_eq!(name.cached_hash(), expected);
        });
    }

    /// KnownHash 写入路径验证：parse 后实例 dict 中的 (key, value) 通过
    /// Python attribute access 正常读取（写入正确性）。
    #[test]
    fn parse_o1b_knownhash_writes_dict_correctly_no_expr_path() {
        with_py(|py| {
            let fields = vec![rw_field(py, "x", u8_node()), rw_field(py, "y", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let mut stream = ParseStream::new(&[0xAA, 0xBB]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let x: i64 = inst.getattr("x").unwrap().extract().unwrap();
            let y: i64 = inst.getattr("y").unwrap().extract().unwrap();
            assert_eq!(x, 0xAA);
            assert_eq!(y, 0xBB);
        });
    }

    /// KnownHash 写入路径验证：has_expressions 路径（set_field_at_knownhash）。
    #[test]
    fn parse_o1b_knownhash_writes_dict_correctly_expr_path() {
        with_py(|py| {
            let fields = vec![rw_field(py, "a", u8_node()), rw_field(py, "b", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, true);
            let mut stream = ParseStream::new(&[0x11, 0x22]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            let a: i64 = inst.getattr("a").unwrap().extract().unwrap();
            let b: i64 = inst.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0x11);
            assert_eq!(b, 0x22);
        });
    }

    /// GenericGetDict 路径验证：parse 后通过 getattr("__dict__") 得到的 dict
    /// 与 GenericGetDict 路径返回的是同一对象（identity）。
    #[test]
    fn parse_o1a_generic_getdict_dict_is_instance_dict() {
        with_py(|py| {
            let fields = vec![rw_field(py, "x", u8_node())];
            let node = StructNode::new(py, fields, mock_cls(py), false, false);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let inst = result.bind(py);
            // 验证 x 字段已写入
            let x: i64 = inst.getattr("x").unwrap().extract().unwrap();
            assert_eq!(x, 0x42);
            // 验证 dict 是 instance 的 __dict__（identity 比较）
            let dict_via_getattr = inst.getattr("__dict__").unwrap();
            let dict_via_ggd = crate::instance::dict_via_generic_getdict(py, &inst).expect("ggd");
            assert!(
                dict_via_ggd.as_ptr() == dict_via_getattr.as_ptr(),
                "GenericGetDict and getattr should return the same dict object"
            );
        });
    }

    /// GenericGetDict + KnownHash 联合验证：slots class 仍走 fallback getattr 路径并返回明确错误。
    #[test]
    fn parse_o1a_slots_class_falls_back_to_getattr_error() {
        with_py(|py| {
            let code = "class WithSlots:\n    __slots__ = ('x',)\n";
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(code, Some(&globals), None).expect("define");
            let cls = globals
                .get_item("WithSlots")
                .expect("get_item ok")
                .expect("class exists")
                .extract::<Py<PyType>>()
                .expect("extract");
            let fields = vec![rw_field(py, "x", u8_node())];
            let node = StructNode::new(py, fields, cls, false, false);
            let mut stream = ParseStream::new(&[0x42]);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("slots class should fail");
            match err {
                ConstructError::Generic { message, .. } => {
                    assert!(
                        message.contains("__dict__") || message.contains("slots"),
                        "error should mention __dict__/slots: {}",
                        message
                    );
                }
                other => panic!("expected Generic error for slots class, got {:?}", other),
            }
        });
    }
}
