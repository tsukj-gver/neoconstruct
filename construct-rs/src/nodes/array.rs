//! ArrayNode：固定次数数组读写。
//!
//! 设计依据：`docs/模块设计-Array.md` §4.1。
//! Python 参考：`construct/construct/core.py` `Array`（L2493-2567）。
//!
//! ## 概述
//!
//! `Array(count, subcon, discard)` 解析 `count` 个 `subcon` 元素到 Python list，
//! 或从 list 构建 `count` 个元素的字节序列。
//!
//! - `count` 可以是编译期常量（[`CountSource::Const`]）或表达式程序
//!   （[`CountSource::Expr`]`，如 `Array(length, Byte)`）。
//! - `discard=True` 时仍消耗流但不收集结果（返回空 list）。
//! - 嵌套数组（Array 内 Array）：内层 `_index` 覆盖外层，循环结束后恢复。
//!
//! ## 性能要点
//!
//! - **4.7 lazy path 迁移**：成功路径不调 `path.push_index/pop`（P0-3 模式推广），
//!   子节点返回 Err 时通过 `ConstructError::push_path_index(i)` 重建索引段。
//! - **4.7 PyList Vec 中转**：用 `Vec<Py<PyAny>>` 收集元素后一次性 `PyList::new_bound`，
//!   避免 `PyList::append` 慢路径（capacity 检查 + realloc）。
//! - `ctx.set_index` 是栈字段写入（~1ns），不走 PyDict。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyList;

// ---------------------------------------------------------------------------
// CountSource
// ---------------------------------------------------------------------------

/// Array 的元素数量来源。
///
/// 设计依据：`docs/模块设计-Array.md` §2.2 决策 A2。
#[derive(Debug, Clone)]
pub enum CountSource {
    /// 编译期常量（如 `Array(5, Byte)`）。
    Const(usize),
    /// 表达式程序（如 `Array(length, Byte)`，复用现有 ExprProgram）。
    Expr(ExprProgram),
}

impl CountSource {
    /// 是否为表达式 count。
    pub fn is_expr(&self) -> bool {
        matches!(self, CountSource::Expr(_))
    }
}

// ---------------------------------------------------------------------------
// ArrayNode
// ---------------------------------------------------------------------------

/// 固定次数数组节点。
///
/// 对应 Python construct `Array(count, subcon, discard)`（core.py L2493）。
///
/// # parse 行为
///
/// 1. 求值 count（常量或表达式）
/// 2. 创建 PyList（预分配容量）
/// 3. `for i in 0..count`：设置 `ctx._index = i`，inner.parse，append 到 list
/// 4. 循环结束恢复 `ctx._index` 到入口值
///
/// # build 行为
///
/// 1. 求值 count
/// 2. 校验 obj 是 list/tuple，且长度等于 count（否则 `Range` 错误）
/// 3. 遍历元素：设置 `ctx._index = i`，inner.build
///
/// # sizeof 行为
///
/// - `Const(n)` × `inner.sizeof()` 的乘积
/// - `Expr` count：在 GIL 持有时求值（设计 §4.1.4 / §12.6 决策点 6 选项 A）
#[derive(Debug)]
pub struct ArrayNode {
    /// 元素子树（递归 Box）。
    inner: Box<crate::nodes::Node>,
    /// 元素数量来源（静态 / 表达式）。
    count: CountSource,
    /// 是否丢弃解析结果（仍消耗流）。
    discard: bool,
}

impl ArrayNode {
    /// 创建 ArrayNode。
    pub fn new(inner: crate::nodes::Node, count: CountSource, discard: bool) -> Self {
        Self {
            inner: Box::new(inner),
            count,
            discard,
        }
    }

    /// 返回 inner 子树引用。
    pub fn inner(&self) -> &crate::nodes::Node {
        &self.inner
    }

    /// 返回 count 来源引用。
    pub fn count(&self) -> &CountSource {
        &self.count
    }

    /// 是否丢弃解析结果。
    pub fn discard(&self) -> bool {
        self.discard
    }

    /// 求值元素数量。负数（i64 < 0）返回 `Range` 错误（对齐 core.py L2527-2528）。
    fn eval_count(&self, ctx: &Context<'_>, py: Python<'_>) -> Result<usize, ConstructError> {
        let count_i64 = match &self.count {
            CountSource::Const(n) => *n as i64,
            CountSource::Expr(prog) => {
                eval_expr_int(prog, ctx, py).map_err(|e| ConstructError::Generic {
                    message: format!(
                        "Array count expression evaluation failed: {}",
                        e.full_message()
                    ),
                    path: String::new(),
                })?
            }
        };
        if count_i64 < 0 {
            return Err(ConstructError::Range {
                message: format!("invalid count {}", count_i64),
                path: String::new(),
            });
        }
        Ok(count_i64 as usize)
    }

    /// has_expressions 判断（设计 §6.1.1）。
    pub fn has_expressions(&self) -> bool {
        self.inner.has_expressions() || self.count.is_expr()
    }
}

// ---------------------------------------------------------------------------
// Construct impl
// ---------------------------------------------------------------------------

impl super::Construct for ArrayNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let count = self.eval_count(ctx, py)?;

        // 4.7 PyList Vec 中转：用 Vec 收集元素后一次性创建 PyList，
        // 避免 PyList::append 慢路径（CPython list 的 capacity 检查 + 可能 realloc）。
        // Vec::with_capacity 一次性分配，push 是纯 Rust 操作（~1-2ns/elem）。
        let mut elems: Vec<Py<PyAny>> = Vec::with_capacity(count);

        // 保存外层 _index（嵌套数组支持，设计 §3.2.2）。
        let old_index = ctx.index();

        for i in 0..count {
            ctx.set_index(i);
            // 4.7 lazy path：成功路径不调 path.push_index/pop。
            // 子节点返回 Err 时通过 push_path_index 重建索引段（P0-3 模式推广）。
            let elem = match self.inner.parse(py, stream, ctx, path) {
                Ok(v) => v,
                Err(mut e) => {
                    e.push_path_index(i);
                    ctx.restore_index(old_index);
                    return Err(e);
                }
            };
            if !self.discard {
                elems.push(elem);
            } else {
                drop(elem);
            }
        }

        // 一次性创建 PyList（pyo3 内部用 PyList_New + PyList_SET_ITEM）。
        let list = PyList::new_bound(py, elems);

        ctx.restore_index(old_index);
        Ok(list.into_any().unbind())
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let count = self.eval_count(ctx, py)?;

        // 4.7 P1-1 整合：build 路径统一调 collect_obj_to_vec 收集 obj 到 Vec。
        // ArrayNode 的 obj_len 需求通过 vec.len() 获取（O(1)）。
        let items = super::common::collect_obj_to_vec(obj, "Array", path)?;
        let obj_len = items.len();

        // AR-3: 长度校验
        if obj_len != count {
            return Err(ConstructError::Range {
                message: format!("expected {} elements, found {}", count, obj_len),
                path: path.to_string(),
            });
        }

        let old_index = ctx.index();

        for (i, elem) in items.into_iter().enumerate() {
            ctx.set_index(i);
            // 4.7 lazy path：成功路径不调 path.push_index/pop。
            let elem_bound = elem.bind(py);
            if let Err(mut e) = self.inner.build(py, elem_bound, stream, ctx, path) {
                e.push_path_index(i);
                ctx.restore_index(old_index);
                return Err(e);
            }
        }

        ctx.restore_index(old_index);
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        let count = match &self.count {
            CountSource::Const(n) => *n,
            CountSource::Expr(prog) => {
                // 表达式 count：需在 GIL 持有条件下求值。
                // 设计 §4.1.4 + §12.6 决策点 6 选项 A：用 with_gil（sizeof 在 GIL 持有
                // 线程调用，with_gil 在已持 GIL 时 O(1)）。正确写法（避免借用逃逸）：
                // 所有需要 py 的逻辑放进闭包内。
                Python::with_gil(|py| -> Result<usize, ConstructError> {
                    let v = eval_expr_int(prog, ctx, py).map_err(|e| ConstructError::Generic {
                        message: format!(
                            "Array count expression evaluation failed in sizeof: {}",
                            e.full_message()
                        ),
                        path: String::new(),
                    })?;
                    if v < 0 {
                        return Err(ConstructError::Range {
                            message: format!("sizeof array count {} is negative", v),
                            path: String::new(),
                        });
                    }
                    Ok(v as usize)
                })?
            }
        };
        let elem_size = self.inner.sizeof(ctx)?;
        count
            .checked_mul(elem_size)
            .ok_or_else(|| ConstructError::Generic {
                message: format!("array size overflow: {} * {}", count, elem_size),
                path: String::new(),
            })
    }
}

// 恢复 ctx._index 已提升为 `Context::restore_index` 方法（P0-1 整合）。
// 保留此注释作为 ArrayNode 模式采用声明参考。
// 设计 §3.2.2 嵌套数组语义：调用 `ctx.restore_index(old_index)` 即可。
//
// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprOp;
    use crate::nodes::format_field::{FormatFieldNode, PythonFormat};
    use crate::nodes::Construct;
    use pyo3::types::PyString;

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

    /// 在 ctx 中设置字段值 + expr_values，模拟 StructNode parse 环境。
    fn setup_ctx_for_expr<'py>(py: Python<'py>, field_values: &[(&str, i64)]) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("ctx");
        ctx.init_expr_values(field_values.len());
        for (idx, (name, value)) in field_values.iter().enumerate() {
            let key = PyString::new_bound(py, name).unbind();
            let val = (*value).into_py(py);
            ctx.set_field_at(idx, &key, val.bind(py), py)
                .expect("set_field_at");
        }
        ctx
    }

    /// 构造一个 Byte（Int8ub）节点，用于 Array 内部。
    fn byte_node() -> crate::nodes::Node {
        crate::nodes::Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big))
    }

    // ======================================================================
    // ArrayNode 构造器 & CountSource
    // ======================================================================

    #[test]
    fn new_const_creates_array_with_const_count() {
        let node = ArrayNode::new(byte_node(), CountSource::Const(5), false);
        assert!(matches!(node.count(), CountSource::Const(5)));
        assert!(!node.discard());
    }

    #[test]
    fn new_const_zero_count() {
        let node = ArrayNode::new(byte_node(), CountSource::Const(0), false);
        assert!(matches!(node.count(), CountSource::Const(0)));
    }

    #[test]
    fn new_discard_flag() {
        let node = ArrayNode::new(byte_node(), CountSource::Const(3), true);
        assert!(node.discard());
    }

    #[test]
    fn new_expr_count() {
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
        let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
        assert!(node.count().is_expr());
    }

    #[test]
    fn has_expressions_const_returns_inner_value() {
        // 内部无表达式 + 常量 count → false
        let node = ArrayNode::new(byte_node(), CountSource::Const(5), false);
        assert!(!node.has_expressions());
    }

    #[test]
    fn has_expressions_expr_count_returns_true() {
        let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
        let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
        assert!(node.has_expressions());
    }

    // ======================================================================
    // parse：Const 路径
    // ======================================================================

    #[test]
    fn parse_const_reads_n_elements() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(3), false);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 验证返回的是 list
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 3);
            let v0: i64 = list.get_item(0).unwrap().extract().unwrap();
            let v1: i64 = list.get_item(1).unwrap().extract().unwrap();
            let v2: i64 = list.get_item(2).unwrap().extract().unwrap();
            assert_eq!(v0, 1);
            assert_eq!(v1, 2);
            assert_eq!(v2, 3);
            assert_eq!(stream.tell(), 3);
        });
    }

    #[test]
    fn parse_const_zero_count_returns_empty_list() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(0), false);
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 0);
            // count=0 不消耗字节
            assert_eq!(stream.tell(), 0);
        });
    }

    #[test]
    fn parse_const_insufficient_returns_stream_error() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(5), false);
            let mut stream = ParseStream::new(&[0x01, 0x02]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            // AR-5: 子构造器失败时错误向上传播
            match err {
                ConstructError::Stream { message, path: p } => {
                    assert!(message.contains("expected 1"), "got: {}", message);
                    // O1 修复（4.6）：path 应为 "root[2]"——inner.parse 在
                    // path 栈含 Index(2) 时已经把 path 写成 "root[2]"，
                    // ArrayNode 不再补充。验证不出现 "root.[2][2]" 双重标记。
                    assert!(
                        p == "root[2]",
                        "O1 path format: expected 'root[2]', got '{}'",
                        p
                    );
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_const_error_path_no_double_index_marker() {
        // O1 专项测试（4.6）：嵌套场景验证 path 格式无双重 [i] 标记。
        // 场景：Array(3, Array(2, Byte)) 解析到不完整流时，
        // 内层 Array 在 i=1 时失败，错误 path 应为 "root[1][1]"。
        with_py(|py| {
            let inner = ArrayNode::new(byte_node(), CountSource::Const(2), false);
            let outer = ArrayNode::new(
                crate::nodes::Node::Array(inner),
                CountSource::Const(3),
                false,
            );
            // 仅提供 3 字节：外层 [0] 完整（2B），外层 [1] 内层 [0] 完整，
            // 外层 [1] 内层 [1] EOF 失败。
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = outer
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail on incomplete inner array");
            match err {
                ConstructError::Stream { path: p, .. } => {
                    // O1 修复：正确 path 应为 "root[1][1]"，不出现 "root.[1][1].[1][1]" 等。
                    assert!(
                        p == "root[1][1]",
                        "nested O1 path format: expected 'root[1][1]', got '{}'",
                        p
                    );
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_const_discard_returns_empty_but_consumes() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(3), true);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            // discard=True 仍消耗流，返回空 list
            assert_eq!(list.len(), 0);
            assert_eq!(stream.tell(), 3);
        });
    }

    // ======================================================================
    // 4.7 lazy path 嵌套组合测试（设计 §6.2）
    // ======================================================================

    #[test]
    fn parse_returns_native_list_type() {
        // 4.7：验证 Vec 中转后返回的仍是原生 list 类型。
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(3), false);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let result_bound = result.bind(py);
            // isinstance(result, list)
            let is_list: bool = result_bound.is_instance_of::<PyList>();
            assert!(is_list, "Vec 中转后应返回原生 list");
        });
    }

    #[test]
    fn parse_discard_returns_empty_list_with_vec() {
        // 4.7：discard=True 时 Vec 不收集元素，但返回空 list（非 None）。
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(5), true);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03, 0x04, 0x05]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 0);
            // 仍消耗所有字节
            assert_eq!(stream.tell(), 5);
        });
    }

    #[test]
    fn parse_struct_inside_array_error_path() {
        // 4.7 设计 §6.2：Array(3, Struct{x: Byte})，Array[1].x EOF → path = "root[1].x"
        // 重建顺序（从叶到根）：
        //   1. leaf FormatField error path = "root"
        //   2. Struct.push_segment("x"): "root" → "root.x"
        //   3. Array.push_path_index(1): "root.x" → "root[1].x"
        with_py(|py| {
            use crate::nodes::struct_node::{FieldMode, FieldName, StructField, StructNode};
            use pyo3::types::PyType;
            let inner_struct_fields = vec![StructField {
                name: FieldName::new(py, "x"),
                node: byte_node(),
                mode: FieldMode::Rw,
            }];
            let cls = py
                .eval_bound("type('Item', (), {})", None, None)
                .expect("cls")
                .extract::<Py<PyType>>()
                .expect("PyType");
            let inner = crate::nodes::Node::Struct(StructNode::new(
                py,
                inner_struct_fields,
                cls,
                false,
                false,
            ));
            let outer = ArrayNode::new(inner, CountSource::Const(3), false);
            // 仅提供 1 字节：Array[0].x 成功，Array[1].x EOF 失败
            let mut stream = ParseStream::new(&[0x10]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = outer
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail on Array[1].x EOF");
            match err {
                ConstructError::Stream { path: p, .. } => {
                    assert!(p == "root[1].x", "expected 'root[1].x', got '{}'", p);
                }
                other => panic!("expected Stream, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_struct_inside_array_error_path() {
        // 4.7 设计 §6.2：build 方向同上，path = "root[1].x"
        // 重建顺序（从叶到根）：
        //   1. leaf FormatField build error path = "root"
        //   2. Struct.push_segment("x"): "root" → "root.x"
        //   3. Array.push_path_index(1): "root.x" → "root[1].x"
        with_py(|py| {
            use crate::nodes::struct_node::{FieldMode, FieldName, StructField, StructNode};
            use pyo3::types::PyType;
            let inner_struct_fields = vec![StructField {
                name: FieldName::new(py, "x"),
                node: byte_node(),
                mode: FieldMode::Rw,
            }];
            let cls = py
                .eval_bound("type('Item', (), {})", None, None)
                .expect("cls")
                .extract::<Py<PyType>>()
                .expect("PyType");
            let inner = crate::nodes::Node::Struct(StructNode::new(
                py,
                inner_struct_fields,
                cls,
                false,
                false,
            ));
            let outer = ArrayNode::new(inner, CountSource::Const(2), false);
            // 传入 2 个元素，第 2 个元素的 x 是 bytes（Int8ub.build 失败）
            let obj = py
                .eval_bound(
                    "[type('I', (), {'x': 1})(), type('I', (), {'x': b'bad'})()]",
                    None,
                    None,
                )
                .expect("obj");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = outer
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail on Array[1].x build");
            let p = err.path().unwrap_or("");
            // 期望 path 含 "root[1].x"（FormatField 错误的 path 重建后）
            assert!(
                p == "root[1].x" || p.ends_with("[1].x"),
                "expected 'root[1].x' or ending with '[1].x', got '{}'",
                p
            );
        });
    }

    #[test]
    fn parse_const_sets_index_during_iteration() {
        // 验证 _index 在迭代中被正确设置（通过 IndexNode 等节点观察）。
        // FormatField 无法读 ctx._index，因此改为：parse 后检查 ctx._index 恢复 None。
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(3), false);
            let mut stream = ParseStream::new(&[0x01, 0x02, 0x03]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let _ = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 循环结束后 _index 应被恢复为 None（new_root 入口时为 None）
            assert!(ctx.index().is_none(), "index should be restored to None");
        });
    }

    #[test]
    fn parse_const_nested_array_restores_outer_index() {
        // Array(2, Array(3, Byte))：外层 _index 在内层结束后恢复
        with_py(|py| {
            let inner = ArrayNode::new(byte_node(), CountSource::Const(3), false);
            let outer_node = crate::nodes::Node::Array(ArrayNode::new(
                crate::nodes::Node::Array(inner),
                CountSource::Const(2),
                false,
            ));
            let mut stream = ParseStream::new(&[
                0x01, 0x02, 0x03, // outer[0]
                0x04, 0x05, 0x06, // outer[1]
            ]);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let result = outer_node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 验证外层是 list of list
            let outer_list = result.bind(py).downcast::<PyList>().expect("outer list");
            assert_eq!(outer_list.len(), 2);
            let inner0 = outer_list.get_item(0).unwrap();
            let inner0_list = inner0.downcast::<PyList>().expect("inner0 list");
            assert_eq!(inner0_list.len(), 3);
            let v: i64 = inner0_list.get_item(2).unwrap().extract().unwrap();
            assert_eq!(v, 3);
            let inner1 = outer_list.get_item(1).unwrap();
            let inner1_list = inner1.downcast::<PyList>().expect("inner1 list");
            let v: i64 = inner1_list.get_item(0).unwrap().extract().unwrap();
            assert_eq!(v, 4);
            // 外层 _index 恢复 None
            assert!(ctx.index().is_none());
        });
    }

    // ======================================================================
    // parse：Expr 路径
    // ======================================================================

    #[test]
    fn parse_expr_count_from_field() {
        // Array(count, Byte) → count=3 → 读 3 字节
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
            let mut stream = ParseStream::new(&[0x0A, 0x0B, 0x0C, 0x0D]);
            let mut ctx = setup_ctx_for_expr(py, &[("count", 3)]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 3);
            let v0: i64 = list.get_item(0).unwrap().extract().unwrap();
            assert_eq!(v0, 10);
            assert_eq!(stream.tell(), 3);
        });
    }

    #[test]
    fn parse_expr_count_negative_returns_range_error() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(-1)]);
            let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
            let mut stream = ParseStream::new(&[]);
            let mut ctx = setup_ctx_for_expr(py, &[]);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Range { message, .. } => {
                    assert!(message.contains("-1"), "got: {}", message);
                }
                other => panic!("expected Range, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_expr_count_zero_returns_empty_list() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
            let mut stream = ParseStream::new(&[0xFF]);
            let mut ctx = setup_ctx_for_expr(py, &[]);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("is list");
            assert_eq!(list.len(), 0);
        });
    }

    // ======================================================================
    // build
    // ======================================================================

    #[test]
    fn build_writes_list_elements() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(3), false);
            let obj = py.eval_bound("[10, 20, 30]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[10, 20, 30]);
        });
    }

    #[test]
    fn build_writes_tuple_elements() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(3), false);
            let obj = py.eval_bound("(1, 2, 3)", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[1, 2, 3]);
        });
    }

    #[test]
    fn build_length_mismatch_returns_range_error() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(3), false);
            let obj = py.eval_bound("[1, 2]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            let err = node
                .build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Range { message, .. } => {
                    assert!(message.contains("expected 3"), "got: {}", message);
                    assert!(message.contains("found 2"), "got: {}", message);
                }
                other => panic!("expected Range, got {:?}", other),
            }
        });
    }

    #[test]
    fn build_empty_list_zero_count_succeeds() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(0), false);
            let obj = py.eval_bound("[]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert!(stream.as_bytes().is_empty());
        });
    }

    #[test]
    fn build_expr_count_validates_length() {
        with_py(|py| {
            let prog = ExprProgram::new(vec![ExprOp::Const(3)]);
            let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
            let obj = py.eval_bound("[10, 20, 30]", None, None).expect("eval");
            let mut stream = BuildStream::new();
            let mut ctx = setup_ctx_for_expr(py, &[]);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            assert_eq!(stream.as_bytes(), &[10, 20, 30]);
        });
    }

    // ======================================================================
    // parse ↔ build 往返
    // ======================================================================

    #[test]
    fn round_trip_const_array_preserves_data() {
        with_py(|py| {
            let node = ArrayNode::new(byte_node(), CountSource::Const(5), false);
            let mut ctx = Context::new_root(py).expect("ctx");
            let mut path = Path::new();

            let obj = py
                .eval_bound("[100, 110, 120, 130, 140]", None, None)
                .expect("eval");
            let mut stream = BuildStream::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");
            let bytes = stream.into_bytes();

            let mut pstream = ParseStream::new(&bytes);
            let result = node
                .parse(py, &mut pstream, &mut ctx, &mut path)
                .expect("parse");
            let list = result.bind(py).downcast::<PyList>().expect("list");
            assert_eq!(list.len(), 5);
            let values: Vec<i64> = list.iter().map(|b| b.extract().unwrap()).collect();
            assert_eq!(values, vec![100, 110, 120, 130, 140]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_const_returns_count_times_inner_size() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = ArrayNode::new(byte_node(), CountSource::Const(10), false);
            assert_eq!(node.sizeof(&ctx).unwrap(), 10);
        });
    }

    #[test]
    fn sizeof_const_zero() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let node = ArrayNode::new(byte_node(), CountSource::Const(0), false);
            assert_eq!(node.sizeof(&ctx).unwrap(), 0);
        });
    }

    #[test]
    fn sizeof_expr_count_evaluates() {
        with_py(|py| {
            // 表达式 count 需要 ctx 持有 expr_values
            let ctx = setup_ctx_for_expr(py, &[("count", 4)]);
            let prog = ExprProgram::new(vec![ExprOp::GetInt(0)]);
            let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
            assert_eq!(node.sizeof(&ctx).unwrap(), 4);
        });
    }

    #[test]
    fn sizeof_expr_count_negative_returns_range_error() {
        with_py(|py| {
            let ctx = Context::new_root(py).expect("ctx");
            let prog = ExprProgram::new(vec![ExprOp::Const(-1)]);
            let node = ArrayNode::new(byte_node(), CountSource::Expr(prog), false);
            let err = node.sizeof(&ctx).expect_err("should fail");
            assert!(matches!(err, ConstructError::Range { .. }));
        });
    }
}
