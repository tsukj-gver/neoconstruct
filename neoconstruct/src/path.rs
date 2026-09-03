//! 错误路径追踪栈。
//!
//! ## 用途
//!
//! `Path` 用于在 parse/build 的执行树遍历中追踪当前所处的字段位置。
//! Struct 节点进入子字段时 `push_field(name)`，离开时 `pop()`。
//! 错误发生时 `to_string()` 生成如 `"root.header.flags"` 的路径字符串，
//! 嵌入到 `ConstructError` 中以便定位出错字段。
//!
//! ## 性能（lazy path 模式）
//!
//! - **成功路径**：`Path::new()` 返回 `Path::Root`（零分配），从不 push，从不读。零成本。
//! - **错误路径**：叶节点读 `to_string()`（`Root` 返回 `"root"` 字面量），
//!   父节点通过 `ConstructError::push_path_segment` / `push_path_index` 重建路径。
//!   `Path` 对象本身保持 `Root` 态不变。
//! - **测试代码**：少量测试显式 `push_field` 验证 path 行为，会触发 `Root → Segments` 转换。
//!
//! ## 格式说明
//!
//! 当前实现的 Display 格式为 `"root.field1.field2[N]"`：
//! - 初始 segment 为 `(root)`，输出为 `"root"`（小写无括号）。
//! - 后续 Field 用 `.` 分隔，如 `".header"`。
//! - Index 用 `[N]` 后缀，如 `"[2]"`。
//!
//! ## 与 Python construct 的格式差异
//!
//! Python construct 的实际 path 格式（见 `construct/construct/core.py` `Renamed._parse`）：
//! - 根路径为 `"(parsing)"` / `"(building)"` / `"(sizeof)"`（取决于操作类型）。
//! - 字段用 `" -> "` 分隔，如 `"(parsing) -> header -> flags"`。
//!
//! 本项目选择了 `"root.field1.field2[N]"` 的简化格式。两者不一致。
//! Path 是 Rust 内部错误追踪机制，通过 `error.rs` 的 `full_message()`
//! 最终生成给 Python 用户的错误消息。后续若需要严格对齐 Python construct 的 path 格式，
//! 可调整 `Path::Display` 实现，无需改动调用方。

use std::fmt;

/// 错误路径中的一个段：根标记、结构体字段名或数组索引。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// 根路径段（无 String payload，零分配）。
    ///
    /// 由 [`Path::new`] 预置，Display 输出为 `"root"`。
    Root,
    /// 结构体字段名（如 `"address"`）。
    Field(String),
    /// 数组索引（如 `[2]`）。Array 节点使用。
    Index(usize),
}

/// 错误路径追踪栈。
///
/// 详见模块级文档。
///
/// # 两态 enum（零分配）
///
/// lazy path 模式下，生产代码成功路径从不 push。
/// `Root` 变体覆盖 >99% 场景，零堆分配（`Path::new()` 仅构造 enum 变体）。
///
/// - [`Path::Root`]：仅含根段。`new()` / `default()` / 成功路径的常态。
/// - [`Path::Segments`]：显式 push 后的扩展路径。仅测试代码或（罕见的）eager path
///   节点触发。内部 `Vec` 的首元素始终是 [`PathSegment::Root`]。
#[derive(Debug, Clone)]
pub enum Path {
    /// 仅含根段。零分配。
    ///
    /// 这是 lazy path 模式下的常态：成功路径不 push，叶节点读 `to_string()` 得 `"root"`。
    Root,
    /// 扩展路径（push 后转换）。
    ///
    /// 仅在显式调用 `push_field`/`push_index` 后产生。生产代码不触发（lazy path 模式），
    /// 保留用于测试代码与未来可能的 eager path 节点。
    ///
    /// 内部 `Vec` 的首元素在从 `Root` 转换时被预置为 [`PathSegment::Root`]，
    /// 后续 push 追加到末尾。`pop` 到仅剩 Root 段时不自动转回 `Root` 变体
    /// （功能等价，避免多余的状态转换开销）。
    Segments(Vec<PathSegment>),
}

impl Path {
    /// 创建新的路径栈，初始状态为 `Root` 变体（零分配）。
    ///
    /// 进入执行树时调用一次（parse/build 入口）。
    ///
    /// lazy path 模式：从 `vec![PathSegment::Root]`（1 次 Vec 堆分配 ~20-30ns）
    /// 改为 `Path::Root`（enum 构造 ~0ns）。
    pub fn new() -> Self {
        Path::Root
    }

    /// 在路径末尾追加一个字段段（如进入 `header` 字段时）。
    ///
    /// `Root` 变体转换为 `Segments(vec![Root, Field(name)])`；
    /// `Segments` 变体直接 push。
    pub fn push_field(&mut self, name: &str) {
        match self {
            Path::Root => {
                *self = Path::Segments(vec![
                    PathSegment::Root,
                    PathSegment::Field(name.to_string()),
                ]);
            }
            Path::Segments(segs) => {
                segs.push(PathSegment::Field(name.to_string()));
            }
        }
    }

    /// 在路径末尾追加一个索引段（如进入数组第 2 个元素时）。
    ///
    /// `Root` 变体转换为 `Segments(vec![Root, Index(i)])`；
    /// `Segments` 变体直接 push。
    pub fn push_index(&mut self, i: usize) {
        match self {
            Path::Root => {
                *self = Path::Segments(vec![PathSegment::Root, PathSegment::Index(i)]);
            }
            Path::Segments(segs) => {
                segs.push(PathSegment::Index(i));
            }
        }
    }

    /// 弹出路径末尾段（离开字段/数组元素时）。
    ///
    /// 弹出根 segment 不会发生（调用方负责成对 push/pop），但即使发生也不 panic（静默返回）：
    /// `Root` 变体的 pop 转换为空的 `Segments(Vec::new())`（零分配）。
    pub fn pop(&mut self) {
        match self {
            Path::Root => {
                // 防御性：pop on Root 转换为空 Segments（零分配，Vec::new 不分配）。
                *self = Path::Segments(Vec::new());
            }
            Path::Segments(segs) => {
                segs.pop();
            }
        }
    }

    /// 返回当前路径段数（含根 `(root)`）。
    ///
    /// `Root` 变体返回 1；`Segments` 返回内部 Vec 长度。
    pub fn len(&self) -> usize {
        match self {
            Path::Root => 1,
            Path::Segments(segs) => segs.len(),
        }
    }

    /// 路径是否为空（不含任何 segment）。
    ///
    /// `Root` 变体始终非空（含 1 个 Root 段）。`Segments` 返回内部 Vec 的 is_empty。
    pub fn is_empty(&self) -> bool {
        match self {
            Path::Root => false,
            Path::Segments(segs) => segs.is_empty(),
        }
    }
}

impl Default for Path {
    /// `Path` 的默认值与 [`Path::new`] 一致（`Path::Root`，零分配）。
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Path {
    /// 生成 `"root.field1.field2[0]"` 格式的路径字符串。
    ///
    /// 规则：
    /// - `Root` 变体直接输出 `"root"`（无需遍历，零分配）。
    /// - `Segments` 变体遍历：
    ///   - 初始 `(root)` segment 输出为 `"root"`（剥离括号）。
    ///   - 后续 Field 段用 `.` 分隔。
    ///   - Index 段直接追加 `[N]`（不再加 `.` 分隔，因为索引是前一个对象的成员）。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Path::Root => f.write_str("root"),
            Path::Segments(segs) => {
                for (i, seg) in segs.iter().enumerate() {
                    match seg {
                        PathSegment::Root => {
                            // 根段：输出 "root"（无 payload，零分配）。
                            // 无论位置（i 是否为 0），Root 都输出 "root" 不加前缀点。
                            f.write_str("root")?;
                        }
                        PathSegment::Field(name) => {
                            if i == 0 {
                                // 非根的 Field 段出现在首位（如 pop 后重新 push）原样输出。
                                f.write_str(name)?;
                            } else {
                                f.write_str(".")?;
                                f.write_str(name)?;
                            }
                        }
                        PathSegment::Index(idx) => {
                            write!(f, "[{}]", idx)?;
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_has_root_segment() {
        let p = Path::new();
        assert_eq!(p.len(), 1);
        assert_eq!(p.to_string(), "root");
    }

    #[test]
    fn default_equals_new() {
        let p_default = Path::default();
        let p_new = Path::new();
        assert_eq!(p_default.to_string(), p_new.to_string());
        assert_eq!(p_default.len(), p_new.len());
    }

    #[test]
    fn push_single_field_appends_with_dot() {
        let mut p = Path::new();
        p.push_field("address");
        assert_eq!(p.to_string(), "root.address");
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn push_multiple_fields_chain() {
        let mut p = Path::new();
        p.push_field("header");
        p.push_field("flags");
        p.push_field("bit0");
        assert_eq!(p.to_string(), "root.header.flags.bit0");
    }

    #[test]
    fn push_index_appends_bracket() {
        let mut p = Path::new();
        p.push_field("items");
        p.push_index(0);
        assert_eq!(p.to_string(), "root.items[0]");
    }

    #[test]
    fn push_index_higher_value() {
        let mut p = Path::new();
        p.push_field("matrix");
        p.push_index(255);
        assert_eq!(p.to_string(), "root.matrix[255]");
    }

    #[test]
    fn nested_array_indices() {
        let mut p = Path::new();
        p.push_field("grid");
        p.push_index(3);
        p.push_index(7);
        assert_eq!(p.to_string(), "root.grid[3][7]");
    }

    #[test]
    fn mixed_fields_and_indices() {
        let mut p = Path::new();
        p.push_field("header");
        p.push_field("entries");
        p.push_index(2);
        p.push_field("value");
        assert_eq!(p.to_string(), "root.header.entries[2].value");
    }

    #[test]
    fn pop_removes_last_segment() {
        let mut p = Path::new();
        p.push_field("a");
        p.push_field("b");
        assert_eq!(p.to_string(), "root.a.b");
        p.pop();
        assert_eq!(p.to_string(), "root.a");
        p.pop();
        assert_eq!(p.to_string(), "root");
    }

    #[test]
    fn pop_to_root_stays_at_root() {
        let mut p = Path::new();
        p.pop();
        // 根 segment 被弹出，路径变为空。
        assert_eq!(p.len(), 0);
        assert!(p.is_empty());
        assert_eq!(p.to_string(), "");
    }

    #[test]
    fn pop_after_push_index() {
        let mut p = Path::new();
        p.push_field("items");
        p.push_index(5);
        assert_eq!(p.to_string(), "root.items[5]");
        p.pop();
        assert_eq!(p.to_string(), "root.items");
    }

    #[test]
    fn push_field_with_empty_name() {
        let mut p = Path::new();
        p.push_field("");
        // 空字段名仍输出分隔点。
        assert_eq!(p.to_string(), "root.");
    }

    #[test]
    fn push_field_with_special_chars() {
        let mut p = Path::new();
        p.push_field("weird-name");
        p.push_field("inner.value");
        // 字段名中的特殊字符原样输出，不转义。
        assert_eq!(p.to_string(), "root.weird-name.inner.value");
    }

    #[test]
    fn root_segment_with_custom_name_displays_as_is() {
        // 通过 Default 构造的 Path 与 new() 等价。
        // 直接构造一个不含初始 (root) 的 Path 用以验证非根 Field 段的渲染。
        let mut p = Path::new();
        p.pop(); // 弹出 (root)
        p.push_field("custom_root");
        assert_eq!(p.to_string(), "custom_root");
    }

    #[test]
    fn clone_preserves_segments() {
        let mut p = Path::new();
        p.push_field("a");
        p.push_index(1);
        let cloned = p.clone();
        assert_eq!(p.to_string(), cloned.to_string());
        // 修改原件不影响克隆。
        p.push_field("b");
        assert_ne!(p.to_string(), cloned.to_string());
    }

    #[test]
    fn segment_equality() {
        assert_eq!(
            PathSegment::Field("x".into()),
            PathSegment::Field("x".into())
        );
        assert_ne!(
            PathSegment::Field("x".into()),
            PathSegment::Field("y".into())
        );
        assert_eq!(PathSegment::Index(5), PathSegment::Index(5));
        assert_ne!(PathSegment::Field("x".into()), PathSegment::Index(0));
    }

    #[test]
    fn empty_path_displays_as_empty_string() {
        // Path 是 enum，直接用 Segments(Vec::new()) 构造空路径。
        let p = Path::Segments(Vec::new());
        assert!(p.is_empty());
        assert_eq!(p.to_string(), "");
    }

    #[test]
    fn round_trip_push_pop_returns_to_root() {
        let original = Path::new().to_string();
        let mut p = Path::new();
        p.push_field("a");
        p.push_field("b");
        p.push_index(3);
        p.pop();
        p.pop();
        p.pop();
        assert_eq!(p.to_string(), original);
    }

    // ======================================================================
    // enum 零分配测试
    // ======================================================================

    #[test]
    fn new_returns_root_variant() {
        // Path::new() 应返回 Root 变体（零分配）。
        let p = Path::new();
        assert!(matches!(p, Path::Root));
    }

    #[test]
    fn root_to_string_returns_literal() {
        // Root 变体直接返回 "root" 字面量，无需遍历。
        let p = Path::new();
        assert_eq!(p.to_string(), "root");
    }

    #[test]
    fn push_on_root_transitions_to_segments() {
        // Root 变体 push 后转换为 Segments 变体。
        let mut p = Path::new();
        assert!(matches!(p, Path::Root));
        p.push_field("a");
        assert!(matches!(p, Path::Segments(_)));
        assert_eq!(p.to_string(), "root.a");

        let mut p2 = Path::new();
        p2.push_index(0);
        assert!(matches!(p2, Path::Segments(_)));
        assert_eq!(p2.to_string(), "root[0]");
    }

    #[test]
    fn pop_on_root_transitions_to_empty_segments() {
        // Root 变体 pop 转换为空 Segments（零分配，不 panic）。
        let mut p = Path::new();
        assert!(matches!(p, Path::Root));
        p.pop();
        assert!(matches!(p, Path::Segments(_)));
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
        assert_eq!(p.to_string(), "");
    }

    #[test]
    fn default_returns_root_variant() {
        // default() 与 new() 一致，返回 Root 变体。
        let p = Path::default();
        assert!(matches!(p, Path::Root));
    }
}
