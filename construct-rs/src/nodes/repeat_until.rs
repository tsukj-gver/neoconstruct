//! RepeatUntilNode：终止表达式数组读写（v5 重写占位）。
//!
//! **阶段 1 临时占位**：v5 实施过程中此文件是临时桩——删除了 v4 的 PyCallable
//! 路径后，`RepeatPredicate` 枚举不再存在。完整 v5 实现将在阶段 5/6 引入
//! `RepeatUntilNode { inner, terminator, element_field_idx, element_field_name, discard }`。
//!
//! 详见 `docs/模块设计-Array.md` §4.3 / §2.5。

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::ExprProgram;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;

#[allow(unused_imports)]
use super::Construct;

// ---------------------------------------------------------------------------
// RepeatUntilNode（v5 阶段 6 完全重写前的临时占位）
// ---------------------------------------------------------------------------

/// 终止表达式数组节点（v5 阶段 6 完全重写前的临时占位）。
///
/// 对应 Python construct `RepeatUntil(terminator, subcon, discard)`。
/// 阶段 6 将完全重写数据结构与 parse/build 实现。
#[derive(Debug)]
#[allow(dead_code)]
pub struct RepeatUntilNode {
    /// 元素子树（递归 Box）。
    inner: Box<crate::nodes::Node>,
    /// 占位：v5 阶段 6 替换为 `terminator: ExprProgram`。
    _placeholder: ExprProgram,
}

impl RepeatUntilNode {
    /// 创建 RepeatUntilNode（临时占位构造函数）。
    ///
    /// **阶段 1 临时**：调用方应不调用此构造函数——阶段 2 删除 compile.rs 中
    /// 的 `build_repeat_until_node` 调用（用 `unimplemented!()` 占位）。
    pub fn new(_inner: crate::nodes::Node, _placeholder: ExprProgram) -> Self {
        Self {
            inner: Box::new(_inner),
            _placeholder,
        }
    }

    /// has_expressions 判断（设计 §6.1.1）。
    ///
    /// **v5：始终返回 true**（终止表达式始终引用 Element 字段）。
    pub fn has_expressions(&self) -> bool {
        true
    }
}

impl super::Construct for RepeatUntilNode {
    fn parse<'py>(
        &self,
        _py: Python<'py>,
        _stream: &mut ParseStream<'_>,
        _ctx: &mut Context<'py>,
        _path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 阶段 1 临时占位：阶段 6 完全重写 parse 实现。
        unimplemented!("RepeatUntilNode.parse: v5 阶段 6 完全重写");
    }

    fn build(
        &self,
        _py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        _stream: &mut BuildStream,
        _ctx: &mut Context<'_>,
        _path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 阶段 1 临时占位：阶段 6 完全重写 build 实现。
        unimplemented!("RepeatUntilNode.build: v5 阶段 6 完全重写");
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python `RepeatUntil._sizeof` L2703-2704：永远 SizeofError。
        Err(ConstructError::Generic {
            message: "RepeatUntil size is undefined".to_string(),
            path: String::new(),
        })
    }
}
