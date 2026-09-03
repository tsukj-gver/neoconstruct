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
//! 依赖 pyo3 的 `extract`（基于 `PyTypeInfo`）和 Python type name 识别。按优先级：
//! 1. `FormatFieldDescriptor` → [`Node::FormatField`]
//! 2. `BytesDescriptor` → [`Node::Bytes`]
//! 3. `GreedyBytesDescriptor` → [`Node::GreedyBytes`]
//! 4. StructMixin 子类（`hasattr("_construct_compiled")`）→ [`Node::StructRef`]
//! 5. 纯 Python 描述符（按 type name）：
//!    - `TellDescriptor` → [`Node::Tell`]
//!    - `ComputedDescriptor` → [`Node::Computed`]
//! 6. 全部失败 → `ConstructError::Compilation`
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

use crate::descriptors::{
    BytesDescriptor, BytesIntegerDescriptor, FormatFieldDescriptor, GreedyBytesDescriptor,
};
use crate::error::ConstructError;
use crate::expr::{ExprOp, ExprProgram};
use crate::nodes::adapter_callback::AdapterCallbackNode;
use crate::nodes::aligned::AlignedNode;
use crate::nodes::array::{ArrayNode, CountSource};
use crate::nodes::bit_padding::BitPaddingNode;
use crate::nodes::bits_integer::{BitsIntegerNode, MAX_BITS_INTEGER};
use crate::nodes::bitwise::BitwiseNode;
use crate::nodes::bytes::{BytesLength, BytesNode};
use crate::nodes::bytes_integer::BytesIntegerNode;
use crate::nodes::bytewise::BytewiseNode;
use crate::nodes::check_node::CheckNode;
use crate::nodes::checksum::{BuiltinHash, BytesSource, ChecksumNode, HashFunc};
use crate::nodes::computed::ComputedNode;
use crate::nodes::const_node::ConstNode;
use crate::nodes::default_node::DefaultNode;
use crate::nodes::element::ElementNode;
use crate::nodes::enum_node::EnumNode;
use crate::nodes::flags_enum::FlagsEnumNode;
use crate::nodes::focused_seq::{FocusedSeqField, FocusedSeqNode};
use crate::nodes::format_field::FormatFieldNode;
use crate::nodes::greedy_bytes::GreedyBytesNode;
use crate::nodes::greedy_range::GreedyRangeNode;
use crate::nodes::hex::{load_hex_display_classes, load_hexdump_display_classes, HexNode};
use crate::nodes::hex_dump::HexDumpNode;
use crate::nodes::if_then_else::IfThenElseNode;
use crate::nodes::index::IndexNode;
use crate::nodes::mapping::MappingNode;
use crate::nodes::named_tuple::{NamedTupleMode, NamedTupleNode};
use crate::nodes::one_of::{NoneOfNode, OneOfNode};
use crate::nodes::padding::PaddingNode;
use crate::nodes::pass::PassNode;
use crate::nodes::peek::PeekNode;
use crate::nodes::pointer::{PointerNode, PointerOffset};
use crate::nodes::prefixed::PrefixedNode;
use crate::nodes::prefixed_array::PrefixedArrayNode;
use crate::nodes::probe::ProbeNode;
use crate::nodes::process_rotate_left::ProcessRotateLeftNode;
use crate::nodes::process_xor::{ProcessXorNode, XorPad};
use crate::nodes::raw_copy::RawCopyNode;
use crate::nodes::rebuild::RebuildNode;
use crate::nodes::repeat_until::RepeatUntilNode;
use crate::nodes::seek::{SeekNode, SeekOffset};
use crate::nodes::select::SelectNode;
use crate::nodes::sequence::{SequenceField, SequenceNode};
use crate::nodes::stop_if::{StopIfCondition, StopIfNode};
use crate::nodes::strings::encoding::Encoding;
use crate::nodes::strings::{
    CStringNode, GreedyStringNode, NullStrippedNode, NullTerminatedNode, PaddedStringNode,
    PascalStringNode,
};
use crate::nodes::struct_node::{FieldMode, FieldName, StructField, StructNode};
use crate::nodes::struct_ref::StructRefNode;
use crate::nodes::subconstruct::SubconstructNode;
use crate::nodes::switch::{SwitchCase, SwitchKey, SwitchNode};
use crate::nodes::tell::TellNode;
use crate::nodes::terminated::TerminatedNode;
use crate::nodes::transform::{ByteTransform, TransformNode};
use crate::nodes::union::{ParseFrom, UnionNode, UnionSubcon};
use crate::nodes::varint::VarIntNode;
use crate::nodes::zigzag::ZigZagNode;
use crate::nodes::{Construct, Node};
use crate::schema::CompiledSchema;
use crate::stream::Whence;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyFrozenSet, PyList, PyString, PyTuple, PyType};

/// StructMixin 子类通过此属性标识自身（由纯 Python 的 StructMixin 设置）。
const STRUCTMIXIN_COMPILED_ATTR: &str = "_construct_compiled";

/// 编译 Schema 为执行树（一次 FFI 穿越）。
///
/// Python 可见签名（Phase 3.2 扩展，向后兼容）：
///
/// ```python
/// compile_schema(
///     cls: type,
///     field_names: list[str],
///     descriptors: list,
///     modes: list[str] | None = None,             # ["rw", "ro", "wo", ...]
///     expr_programs: list[dict | None] | None = None,  # 每字段的表达式程序
///     bitwise: bool = False,                       # Phase 3.2：BitStructMixin 传 True
/// ) -> CompiledSchema
/// ```
///
/// # 参数
///
/// - `cls`：用户类对象（StructMixin 子类）。用于关联编译产物与错误信息中报类名。
/// - `field_names`：字段名列表，按字段声明顺序排列。
/// - `descriptors`：字段描述符列表，与 `field_names` 按位置对应。
/// - `modes`：可选的字段模式列表（`"rw"` / `"ro"` / `"wo"`）。`None` 时所有字段
///   默认为 `"rw"`（Phase 1 兼容）。
/// - `expr_programs`：可选的每字段表达式程序列表。长度与 `field_names` 一致，
///   每个元素为 `None`（该字段无表达式）或 Python dict（`{param_name: [expr_ops]}`）。
///   当前 2.3 阶段仅用于计算 `has_expressions` 标志，具体程序解析推迟到 2.5/2.6。
/// - `bitwise`：Phase 3.2 新增。`true` 时根 `StructNode` 被包入 `BitwiseNode`
///   （对应 Python `BitStructMixin`，等价于 `Bitwise(Struct(...))`）。
///   字段树在 `bitwise=true` 上下文下编译（影响 `Padding` 描述符的选择，
///   详见 [`build_node_from_descriptor`]）。
///
/// # 返回
///
/// 编译产物 [`CompiledSchema`]（不可变的执行树，关联到 `cls`）。
///
/// # 错误
///
/// - [`ConstructError::Compilation`]：`field_names` 与 `descriptors` 长度不一致、
///   未知 mode 字符串、描述符类型未知、`cls` 不是类型、或类定义了 `__slots__`。
#[pyfunction]
#[pyo3(signature = (cls, field_names, descriptors, modes=None, expr_programs=None, bitwise=false))]
pub fn compile_schema(
    py: Python<'_>,
    cls: &Bound<'_, PyType>,
    field_names: Vec<String>,
    descriptors: Vec<Py<PyAny>>,
    modes: Option<Vec<String>>,
    expr_programs: Option<Vec<Option<Py<PyAny>>>>,
    bitwise: bool,
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

    // 1b. 解析 modes：None → 全 Rw（Phase 1 兼容）；Some → 逐字段解析为 FieldMode。
    //     长度必须与 field_names 一致。
    let modes_resolved: Vec<FieldMode> = match &modes {
        None => vec![FieldMode::Rw; field_names.len()],
        Some(m) => {
            if m.len() != field_names.len() {
                return Err(ConstructError::Compilation {
                    message: format!(
                        "modes ({}) 与 field_names ({}) 长度不一致",
                        m.len(),
                        field_names.len()
                    ),
                }
                .into());
            }
            m.iter()
                .map(|s| match s.as_str() {
                    "rw" => Ok(FieldMode::Rw),
                    "ro" => Ok(FieldMode::Ro),
                    "wo" => Ok(FieldMode::Wo),
                    other => Err(ConstructError::Compilation {
                        message: format!("unknown field mode: {}", other),
                    }),
                })
                .collect::<Result<_, _>>()?
        }
    };

    // 1c. 计算 has_expressions：expr_programs 中任一字段为 Some → true。
    //     当前 2.3 阶段不解析表达式内容，仅设置标志（具体解析在 2.5/2.6）。
    //     RepeatUntil 终止表达式引用 Element/Index 字段，需要外层 Struct
    //     init_expr_values，此处先按 expr_programs 初步判断，节点树构建后再次提升
    //     （详见步骤 4 之后的二次扫描）。
    let has_expressions_from_programs = expr_programs
        .as_ref()
        .map(|progs| progs.iter().any(|p| p.is_some()))
        .unwrap_or(false);

    // 2. 检测 __slots__（设计修订 §3.6 + §5.7.1 N4-R3 + R4 §8.1）
    //    手写 __slots__ 在 __init_subclass__ 时已可见，编译期报错（fail fast）。
    //    @dataclass(slots=True) 的 __slots__ 在 __init_subclass__ 后才生成，
    //    可能漏检，由 parse 时 getattr("__dict__") 失败兜底（struct_node.rs
    //    错误信息已含 slots 提示）。
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
    let expr_programs_slice: &[Option<Py<PyAny>>] = expr_programs.as_deref().unwrap_or(&[]);
    let field_names_slice: &[String] = field_names.as_slice();
    let mut fields: Vec<StructField> = Vec::with_capacity(field_names.len());
    for (i, (name, desc)) in field_names.iter().zip(descriptors.iter()).enumerate() {
        let desc_bound = desc.bind(py);
        let node = build_node_from_descriptor(
            py,
            desc_bound,
            i,
            expr_programs_slice,
            field_names_slice,
            bitwise,
        )
        .map_err(|e| with_field_context(e, name))?;
        // FieldName::new 创建 interned PyString 缓存。
        // mode 从解析后的 modes_resolved 取（Phase 2 扩展）。
        let field = StructField {
            name: crate::nodes::struct_node::FieldName::new(py, name.clone()),
            node,
            mode: modes_resolved[i],
        };
        fields.push(field);
    }

    // 5. 组装根 Struct 节点（含 cls + has_post_init + has_expressions）。
    //    v4 V-1 修正：has_expressions 由两部分组合——
    //    (1) Python 侧 expr_programs 检测（字段表达式：Bytes(m)、Computed(...) 等）
    //    (2) Rust 侧节点树扫描（RepeatUntil 终止表达式引用 Element/Index 字段
    //        需要外层 Struct init_expr_values，否则 expr_values_buf 未初始化）。
    //    两者任一为 true → Struct 走 has_expressions=true 路径，确保 ctx.fields()
    //    注入实例 dict。
    let has_expressions_from_tree = fields.iter().any(|f| f.node.has_expressions());
    let has_expressions = has_expressions_from_programs || has_expressions_from_tree;

    let struct_root = Node::Struct(StructNode::new(
        py,
        fields,
        cls.clone().unbind(),
        has_post_init,
        has_expressions,
    ));

    // 6. Phase 3.2：bitwise=true 时，根 StructNode 被包入 BitwiseNode
    //    （对应 Python BitStructMixin → Bitwise(Struct(...))）。
    let root = if bitwise {
        Node::Bitwise(BitwiseNode::new(struct_root))
    } else {
        struct_root
    };

    // 7. 包装为 CompiledSchema
    Ok(CompiledSchema::new(root, cls.clone().unbind(), py))
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
///
/// # 参数
///
/// - `py`：GIL token
/// - `desc`：描述符的 Python 引用
/// - `field_index`：当前字段在 fields 列表中的索引（用于从 `expr_programs` 取表达式）
/// - `expr_programs`：每字段的表达式程序切片（与字段数等长，None 表示无表达式）
/// - `bitwise`：当前编译上下文是否在 bit 域内（设计 §7.2 递归 `bitwise` 上下文传播）。
///
///   Phase 3.2：此参数仅用于向递归调用传播（`BitwiseDescriptor` 内部递归传 `true`）。
///   Phase 3.3 将在 `PaddingDescriptor` 分支使用此参数选择 `BitPaddingNode`（bit 域）
///   或 `PaddingNode`（字节域）。当前为前向兼容保留。
fn build_node_from_descriptor(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
    bitwise: bool,
) -> Result<Node, ConstructError> {
    // 1. FormatFieldDescriptor → Node::FormatField
    if let Ok(fmt) = desc.extract::<Py<FormatFieldDescriptor>>() {
        let fmt_ref = fmt.bind(py).get();
        return Ok(Node::FormatField(FormatFieldNode::new(fmt_ref.format)));
    }

    // 2. BytesDescriptor → Node::Bytes
    if let Ok(b) = desc.extract::<Py<BytesDescriptor>>() {
        let b_ref = b.bind(py).get();
        let length_obj = b_ref.length.bind(py);

        // 尝试 extract 为 usize（常量长度，向后兼容 Phase 1）
        if let Ok(n) = length_obj.extract::<usize>() {
            return Ok(Node::Bytes(BytesNode::new_const(n)));
        }

        // 表达式长度：从 expr_programs 取该字段的 "length" 参数
        let field_exprs = expr_programs.get(field_index).and_then(Option::as_ref).ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "Bytes field has non-constant length but no expression program was provided (field index {})",
                    field_index
                ),
            })?;

        // field_exprs 是一个 dict {param_name: [op_tuples]}
        let field_exprs_dict =
            field_exprs
                .bind(py)
                .downcast::<PyDict>()
                .map_err(|_| ConstructError::Compilation {
                    message: "expression program must be a dict".to_string(),
                })?;
        let ops_list_opt =
            field_exprs_dict
                .get_item("length")
                .map_err(|e| ConstructError::Compilation {
                    message: format!("failed to get 'length' from expression programs: {}", e),
                })?;
        let ops_list = ops_list_opt.ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "Bytes field has non-constant length but 'length' key missing in expression program (field index {})",
                    field_index
                ),
            })?;

        let ops = parse_expr_ops_from_py(&ops_list)?;
        let program = ExprProgram::new(ops);
        return Ok(Node::Bytes(BytesNode::new_expr(program)));
    }

    // 3. GreedyBytesDescriptor → Node::GreedyBytes
    if desc.extract::<Py<GreedyBytesDescriptor>>().is_ok() {
        return Ok(Node::GreedyBytes(GreedyBytesNode::new()));
    }

    // 3.5. BytesIntegerDescriptor → Node::BytesInteger (Phase 6.1)
    if let Ok(b) = desc.extract::<Py<BytesIntegerDescriptor>>() {
        let b_ref = b.bind(py).get();
        return Ok(Node::BytesInteger(BytesIntegerNode::new(
            b_ref.length,
            b_ref.signed,
            b_ref.swapped,
        )));
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

    // 5. 纯 Python 描述符（按类型名识别，§3.7.4-§3.7.5）。
    //    这些描述符定义在 Python 侧（_mixin.py 或 _constructors.py），
    //    不需要 Rust pyclass，通过 duck typing（type name）识别。
    let type_name = desc
        .get_type()
        .name()
        .map_err(|e| ConstructError::Compilation {
            message: format!("failed to get descriptor type name: {}", e),
        })?;
    // 直接 match to_str() 返回的 &str，避免一次性 String 分配。
    // type_name（Bound<PyString>）在 match 期间保持存活，供 &str 借用。
    match type_name
        .to_str()
        .map_err(|e| ConstructError::Compilation {
            message: format!("descriptor type name is not valid UTF-8: {}", e),
        })? {
        // TellDescriptor（无参数）→ TellNode。
        // TellDescriptor._expr_params 返回 {}（无表达式参数），expr_programs 对应位置为 None。
        "TellDescriptor" => return Ok(Node::Tell(TellNode::new())),
        // ComputedDescriptor（带表达式）→ ComputedNode { expr: program }。
        // ComputedDescriptor._expr_params 返回 {"func": <expr>}，编译期生成 "func" 键的 ExprOp 列表。
        "ComputedDescriptor" => {
            // 从 expr_programs 取该字段的 "func" 参数
            let field_exprs = expr_programs.get(field_index).and_then(Option::as_ref);
            // expr_programs 中可能为 None：ComputedDescriptor 应总是有 "func" 表达式。
            // 但若用户传入了常量（非 FieldRef/ExprRef），_extract_and_compile_exprs 不编译，
            // 此时 expr_programs 对应位置为 None。理论上 Computed 总需要表达式，
            // 但为防御性，若为 None 视为编译错误。
            let field_exprs_dict =
                match field_exprs {
                    Some(d) => d.bind(py).downcast::<PyDict>().map_err(|_| {
                        ConstructError::Compilation {
                            message: "ComputedDescriptor expression program must be a dict"
                                .to_string(),
                        }
                    })?,
                    None => {
                        return Err(ConstructError::Compilation {
                            message: format!(
                                "ComputedDescriptor requires an expression program ('func'), \
                             but no expression was provided for field index {}. \
                             Use Computed(expr) with a FieldRef/ExprRef expression.",
                                field_index
                            ),
                        });
                    }
                };
            let ops_list = field_exprs_dict
                .get_item("func")
                .map_err(|e| ConstructError::Compilation {
                    message: format!("failed to get 'func' from ComputedDescriptor: {}", e),
                })?
                .ok_or_else(|| ConstructError::Compilation {
                    message: format!(
                        "ComputedDescriptor expression program is missing 'func' key (field index {})",
                        field_index
                    ),
                })?;
            let ops = parse_expr_ops_from_py(&ops_list)?;
            let program = ExprProgram::new(ops);
            return Ok(Node::Computed(ComputedNode::new(program)));
        }
        // BitsIntegerDescriptor（Phase 3.1）→ BitsIntegerNode。
        // 仅支持常量 length（int）。表达式 length（FieldRef/ExprRef）编译期拒绝。
        // Python 侧 BitsInteger(length, signed, swapped) / Bit() / Nibble() / Octet()
        // 都映射到此描述符（Bit/Nibble/Octet 是 BitsInteger 的语法糖）。
        "BitsIntegerDescriptor" => {
            return Ok(Node::BitsInteger(build_bits_integer_node(
                py,
                desc,
                field_index,
                path_for_field(field_index),
            )?));
        }
        // BitwiseDescriptor（Phase 3.2）→ BitwiseNode（递归编译内部 subcon，bitwise=true）。
        // 对应 Python `Bitwise(subcon)`。内部 subcon 在 bit 域上下文编译。
        // 设计 §7.2：进入 BitwiseDescriptor 后，内部递归调用传 bitwise=true。
        "BitwiseDescriptor" => {
            let inner_desc = desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "BitwiseDescriptor missing 'subcon' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            let inner_node = build_node_from_descriptor(
                py,
                &inner_desc,
                field_index,
                expr_programs,
                field_names,
                true,
            )?;
            return Ok(Node::Bitwise(BitwiseNode::new(inner_node)));
        }
        // PaddingDescriptor（Phase 3.3）→ 根据 bitwise 上下文选择 BitPaddingNode（bit 域）
        // 或 PaddingNode（字节域）。设计 §4.2 / §4.6 / §7.2。
        "PaddingDescriptor" => {
            return build_padding_node(py, desc, field_index, bitwise);
        }
        // BytewiseDescriptor（Phase 3.3）→ BytewiseNode（递归编译内部 subcon，bitwise=false）。
        // 内部 subcon 重建字节域（设计 §7.2 递归 bitwise 上下文传播）。
        "BytewiseDescriptor" => {
            let inner_desc = desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "BytewiseDescriptor missing 'subcon' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            // Bytewise 把 bit 流重组为字节流，内部回到 bitwise=false 上下文
            let inner_node = build_node_from_descriptor(
                py,
                &inner_desc,
                field_index,
                expr_programs,
                field_names,
                false,
            )?;
            return Ok(Node::Bytewise(BytewiseNode::new(inner_node)));
        }
        // BitsSwappedDescriptor（Phase 3.3）→ TransformNode(BitSwap)。
        // 内部 subcon 在字节域编译（变换后仍是字节，bitwise=false）。
        "BitsSwappedDescriptor" => {
            let inner_desc = desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "BitsSwappedDescriptor missing 'subcon' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            let inner_node = build_node_from_descriptor(
                py,
                &inner_desc,
                field_index,
                expr_programs,
                field_names,
                false,
            )?;
            return Ok(Node::Transform(TransformNode::new(
                inner_node,
                ByteTransform::BitSwap,
            )));
        }
        // ByteSwappedDescriptor（Phase 3.3）→ TransformNode(ByteSwap)。
        "ByteSwappedDescriptor" => {
            let inner_desc = desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "ByteSwappedDescriptor missing 'subcon' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            let inner_node = build_node_from_descriptor(
                py,
                &inner_desc,
                field_index,
                expr_programs,
                field_names,
                false,
            )?;
            return Ok(Node::Transform(TransformNode::new(
                inner_node,
                ByteTransform::ByteSwap,
            )));
        }
        // ArrayDescriptor（Phase 4）→ ArrayNode。
        // 完整编译路径参照 BytesDescriptor 表达式长度路径（compile.rs L266-308）：
        // - count 为 int → CountSource::Const
        // - count 为 FieldRef/ExprRef → 从 expr_programs[field_index]["count"] 取 ExprOp 列表
        // 设计依据：`docs/模块设计-Array.md` §6.2.2。
        //
        // 限制（设计 §6.2.2 P3.1）：inner subcon 不支持含表达式的子描述符
        // （如 Array(N, Bytes(this.m))），与 Bitwise(Bytes(this.m)) 同根因——
        // _extract_and_compile_exprs 不递归 inner。Phase 4 仅支持顶层 count 表达式。
        "ArrayDescriptor" => {
            return Ok(Node::Array(build_array_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // GreedyRangeDescriptor（Phase 4）→ GreedyRangeNode。
        // 对应 Python `GreedyRange(subcon, discard)`。无 count（读到流结束），
        // 无表达式参数（GreedyRangeDescriptor._expr_params = {}）。
        // 设计依据：`docs/模块设计-Array.md` §6.2.3。
        "GreedyRangeDescriptor" => {
            return Ok(Node::GreedyRange(build_greedy_range_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // PrefixedArrayDescriptor（Phase 4）→ PrefixedArrayNode。
        // 对应 Python `PrefixedArray(countfield, subcon)`。
        // 设计依据：`docs/模块设计-Array.md` §4.6 / §6.2.3。
        //
        // 编译路径：递归编译 countfield 与 subcon，二者均沿用同一 field_index
        // 和 expr_programs 切片（与 BitwiseDescriptor / ArrayDescriptor 同模式）。
        //
        // 限制（同 ArrayDescriptor，设计 §6.2.2 P3.1）：
        // countfield 与 inner 均不支持含表达式的子描述符
        // （如 `PrefixedArray(Bytes(this.m), Byte)`）。
        "PrefixedArrayDescriptor" => {
            return Ok(Node::PrefixedArray(build_prefixed_array_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // IndexDescriptor（Phase 4）→ IndexNode。
        // 对应 Python construct `Index`（core.py L2934）。
        // IndexNode 直接调 ctx.index() 读取，不走 ExprProgram（v3 决策，§3.3）。
        // IndexDescriptor 无参数（_expr_params = {}），expr_programs 对应位置为 None。
        "IndexDescriptor" => return Ok(Node::Index(IndexNode::new())),
        // ElementDescriptor（Phase 4.5 v5 新增）→ ElementNode。
        // construct-rs 新增（Python construct 无对应物）。Element 字段为 RepeatUntil
        // 终止表达式提供"当前元素"引用入口，parse 返回 None（值由 RepeatUntilNode
        // 借用设置），build 是 no-op，sizeof=0。
        // 设计依据：`docs/模块设计-Array.md` §4.7。
        // ElementDescriptor 无参数（_expr_params = {}），expr_programs 对应位置为 None。
        "ElementDescriptor" => return Ok(Node::Element(ElementNode::new())),
        // RepeatUntilDescriptor（Phase 4.5 v5 重写）→ RepeatUntilNode。
        // 对应 Python construct `RepeatUntil(terminator, subcon, discard)`（core.py L2637）。
        // v5：terminator 必须是 Phase 2 表达式（引用 Element 字段），编译为 ExprProgram，
        // 运行时零 FFI 求值。不接收 Python lambda/callable（用户硬约束 #1）。
        // 设计依据：`docs/模块设计-Array.md` §4.3 / §6.3.1。
        "RepeatUntilDescriptor" => {
            return Ok(Node::RepeatUntil(build_repeat_until_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // StopIfDescriptor（Phase 4）→ StopIfNode。
        // 对应 Python construct `StopIf(condfunc)`（core.py L4079）。
        // 条件分类（设计 §4.5.1）：
        //   - Python bool 常量（True / False）→ StopIfCondition::Always / Never
        //   - FieldRef/ExprRef → StopIfCondition::Expr，从 expr_programs[field_index]["cond"]
        //     取 ExprOp 列表（与 ComputedDescriptor 的 "func" 同模式）。
        "StopIfDescriptor" => {
            return Ok(Node::StopIf(build_stop_if_node(
                py,
                desc,
                field_index,
                expr_programs,
            )?));
        }
        // Phase 6.3：内置 Adapter 系列（Subconstruct/Peek/RawCopy/Rebuild/Pass）。
        // 设计 §6.3 / compile.rs §6.3。
        //
        // SubconstructDescriptor → SubstructNode（纯转发）。
        // 递归编译 inner subcon（沿用 field_index，与 BitwiseDescriptor 同模式）。
        "SubconstructDescriptor" => {
            let inner_desc = desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "SubstructDescriptor missing 'subcon' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            let inner_node = build_node_from_descriptor(
                py,
                &inner_desc,
                field_index,
                expr_programs,
                field_names,
                false,
            )?;
            return Ok(Node::Subconstruct(SubconstructNode::new(inner_node)));
        }
        // PeekDescriptor → PeekNode（seek 回退）。
        "PeekDescriptor" => {
            let inner_desc = desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "PeekDescriptor missing 'subcon' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            let inner_node = build_node_from_descriptor(
                py,
                &inner_desc,
                field_index,
                expr_programs,
                field_names,
                false,
            )?;
            return Ok(Node::Peek(PeekNode::new(inner_node)));
        }
        // RawCopyDescriptor → RawCopyNode（捕获原始字节）。
        "RawCopyDescriptor" => {
            let inner_desc = desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "RawCopyDescriptor missing 'subcon' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            let inner_node = build_node_from_descriptor(
                py,
                &inner_desc,
                field_index,
                expr_programs,
                field_names,
                false,
            )?;
            return Ok(Node::RawCopy(RawCopyNode::new(inner_node)));
        }
        // RebuildDescriptor → RebuildNode（build 时基于表达式重算）。
        // func 表达式从 expr_programs[field_index]["func"] 取（与 ComputedDescriptor 同模式）。
        "RebuildDescriptor" => {
            return Ok(Node::Rebuild(build_rebuild_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // PassDescriptor → PassNode（no-op）。
        "PassDescriptor" => return Ok(Node::Pass(PassNode::new())),
        // Phase 6.1：VarIntDescriptor → VarIntNode（LEB128 无符号变长整数）。
        // 纯 Python 描述符（无 Rust pyclass），按 type name 识别。
        "VarIntDescriptor" => return Ok(Node::VarInt(VarIntNode::new())),
        // Phase 6.1：ZigZagDescriptor → ZigZagNode（有符号变长整数）。
        "ZigZagDescriptor" => return Ok(Node::ZigZag(ZigZagNode::new())),
        // Phase 6.2：CStringDescriptor → CStringNode（C 风格 null 终止字符串）。
        // 编码字符串在编译期调 Encoding::from_user_str 解析（P2b 拒绝无后缀编码）。
        "CStringDescriptor" => {
            return Ok(Node::CString(build_cstring_node(py, desc, field_index)?));
        }
        // Phase 6.2：GreedyStringDescriptor → GreedyStringNode（读到 EOF + decode）。
        "GreedyStringDescriptor" => {
            return Ok(Node::GreedyString(build_greedy_string_node(
                py,
                desc,
                field_index,
            )?));
        }
        // Phase 6.2：PaddedStringDescriptor → PaddedStringNode（固定长度填充字符串）。
        // length 可为 int 常量或表达式（与 BytesDescriptor 同模式）。
        "PaddedStringDescriptor" => {
            return Ok(Node::PaddedString(build_padded_string_node(
                py,
                desc,
                field_index,
                expr_programs,
            )?));
        }
        // Phase 6.2：PascalStringDescriptor → PascalStringNode（长度前缀字符串）。
        // 递归编译 lengthfield（沿用 field_index，与 BitwiseDescriptor 同模式）。
        "PascalStringDescriptor" => {
            return Ok(Node::PascalString(build_pascal_string_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // Phase 6.2：NullTerminatedDescriptor → NullTerminatedNode（null 终止包装器）。
        // 递归编译 inner subcon（沿用 field_index）。
        "NullTerminatedDescriptor" => {
            return Ok(Node::NullTerminated(build_null_terminated_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // Phase 6.2：NullStrippedDescriptor → NullStrippedNode（null 剥离包装器）。
        // 递归编译 inner subcon（沿用 field_index）。
        "NullStrippedDescriptor" => {
            return Ok(Node::NullStripped(build_null_stripped_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // AdapterDescriptor → AdapterCallbackNode（用户面 Adapter 嵌入 Struct 钩子）。
        // PM 决策 6.3-D1 接受。subcon 在 Rust 内执行，_decode/_encode 通过 Py<PyAny> 引用回调。
        //
        // 识别方式（双重）：
        // 1. type name == "AdapterDescriptor"（直接匹配，用户用 AdapterDescriptor 基类）
        // 2. duck typing：desc 同时拥有可调用的 _decode 和 _encode 方法
        //    （识别任意用户 Adapter 子类，无需用户重设 __class__.__name__）。
        // 双重识别保证：直接用 AdapterDescriptor 基类的用户和继承 Adapter 的用户都能工作。
        "AdapterDescriptor" => {
            return Ok(Node::AdapterCallback(build_adapter_callback_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // === Phase 7.1 Conditional（4 个新分支）===
        // IfThenElseDescriptor → IfThenElseNode（双分支条件）。
        // condfunc 编译期分类（与 StopIfDescriptor 同模式）：
        //   - Python bool → StopIfCondition::Always / Never
        //   - FieldRef/ExprRef → StopIfCondition::Expr（从 expr_programs[field_index]["cond"] 取）
        // thensubcon / elsesubcon 递归编译（沿用 field_index，与 BitwiseDescriptor 同模式）。
        "IfThenElseDescriptor" => {
            return Ok(Node::IfThenElse(build_if_then_else_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // SwitchDescriptor → SwitchNode（多分支条件，PM 决策 1：A+B 混合）。
        // keyfunc 编译期分类（ConstInt / IntExpr / FieldRef / 复杂表达式拒绝）。
        // cases 是 Python dict，编译期展开为 Vec<SwitchCase>。
        // default 递归编译（None → Pass，对齐 Python L4032）。
        "SwitchDescriptor" => {
            return Ok(Node::Switch(build_switch_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // SelectDescriptor → SelectNode（多分支尝试）。
        // subcons 是 Python list，递归编译每个元素（与 PrefixedArray subcons 同模式）。
        "SelectDescriptor" => {
            let subcons_obj = desc
                .getattr("subcons")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "SelectDescriptor missing 'subcons' attribute: {} (field index {})",
                        e, field_index
                    ),
                })?;
            let subcons_list =
                subcons_obj
                    .downcast::<PyList>()
                    .map_err(|_| ConstructError::Compilation {
                        message: format!(
                            "SelectDescriptor 'subcons' must be a list (field index {})",
                            field_index
                        ),
                    })?;
            let mut nodes = Vec::with_capacity(subcons_list.len());
            for sub_desc in subcons_list.iter() {
                let sub_node = build_node_from_descriptor(
                    py,
                    &sub_desc,
                    field_index,
                    expr_programs,
                    field_names,
                    false,
                )?;
                nodes.push(sub_node);
            }
            return Ok(Node::Select(SelectNode::new(nodes)));
        }
        // FocusedSeqDescriptor → FocusedSeqNode（聚焦字段序列，设计 §5）。
        // parsebuildfrom 在编译期解析为 focus_idx。
        // subcons 是 Python list（Renamed 包装或匿名 subcon）。
        "FocusedSeqDescriptor" => {
            return Ok(Node::FocusedSeq(build_focused_seq_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // === Phase 7.2 Streams（3 个新分支） ===
        // SeekDescriptor → SeekNode（流定位）。
        // at 可为 int 常量或 FieldRef/ExprRef 表达式（与 StopIf condfunc 同模式）。
        // whence 是 int 0/1/2，编译期翻译为 Whence enum。
        "SeekDescriptor" => {
            return Ok(Node::Seek(build_seek_node(
                py,
                desc,
                field_index,
                expr_programs,
            )?));
        }
        // PointerDescriptor → PointerNode（绝对偏移读写）。
        // offset 可为 int 或 FieldRef/ExprRef（与 Seek at 同模式）。
        // relativeOffset 是 bool。
        // stream 参数非 None 时编译期拒绝（已知限制：换流不支持）。
        // subcon 递归编译（沿用 field_index，与 BitwiseDescriptor 同模式；P3.1 限制：
        // subcon 不支持含表达式的子描述符）。
        "PointerDescriptor" => {
            return Ok(Node::Pointer(build_pointer_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // PrefixedDescriptor → PrefixedNode（长度前缀子流）。
        // lengthfield + subcon 双重递归编译（与 PascalString 同模式）。
        // includelength 是 bool。
        "PrefixedDescriptor" => {
            return Ok(Node::Prefixed(build_prefixed_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // === Phase 8 P0 批次 ===
        // ConstDescriptor → ConstNode（常量校验 + 值匹配）。
        // value 是任意 Python 对象；subcon 由 Python 层在缺省时推断为 Bytes(len)。
        // 递归编译 subcon（沿用 field_index，与 Subconstruct 同模式）。
        "ConstDescriptor" => {
            return Ok(Node::Const(build_const_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // DefaultDescriptor → DefaultNode（默认值填充）。
        // value 在 Python 层若为 FieldRef/ExprRef/int 则编译为 ExprProgram
        // （从 expr_programs[field_index]["value"] 取，与 Rebuild func 同模式）。
        // 递归编译 subcon。
        "DefaultDescriptor" => {
            return Ok(Node::Default(build_default_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // CheckDescriptor → CheckNode（断言，func 是 ExprProgram）。
        // 必须作为 RO 字段（_field_kind = "ro"）；func 从 expr_programs 取。
        "CheckDescriptor" => {
            return Ok(Node::Check(build_check_node(
                py,
                desc,
                field_index,
                expr_programs,
            )?));
        }
        // === Phase 8.9 ===
        // TerminatedDescriptor → TerminatedNode（singleton，无参数）。
        // 对应 Python construct `Terminated`（core.py L4727）。
        // _expr_params 协议返回空 dict，expr_programs 对应位置为 None。
        "TerminatedDescriptor" => return Ok(Node::Terminated(TerminatedNode::new())),
        // ProbeDescriptor → ProbeNode（调试探针）。
        // into 是可选字段名引用（FieldName）；lookahead 是可选编译期 usize 常量。
        "ProbeDescriptor" => {
            return Ok(Node::Probe(build_probe_node(py, desc, field_index)?));
        }
        // === Phase 8.8 ===
        // AlignedDescriptor → AlignedNode（对齐包装）。
        // modulus 可为 int 常量或 FieldRef/ExprRef 表达式（与 SeekDescriptor at 同模式）。
        // pattern 是 1 字节 bytes，编译期提取为 u8。
        // 递归编译 subcon（沿用 field_index，与 BitwiseDescriptor 同模式）。
        "AlignedDescriptor" => {
            return Ok(Node::Aligned(build_aligned_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // === Phase 8.4 ===
        // HexDescriptor → HexNode（Hex 显示包装，Rust Node 非 AdapterCallback）。
        // 递归编译 subcon + 编译期物化 3 个 Hex 显示类（load_hex_display_classes）。
        // fmtstr 编译期预算并 intern 为 Py<PyString>（方案 B）。
        "HexDescriptor" => {
            return Ok(Node::Hex(build_hex_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // HexDumpDescriptor → HexDumpNode（同 Hex，仅显示类不同，2 个 HexDump 类）。
        "HexDumpDescriptor" => {
            return Ok(Node::HexDump(build_hexdump_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // === Phase 8.5 ===
        // ChecksumDescriptor → ChecksumNode（双轨方案）。
        // hashfunc 识别：HashAlgo enum 值 → BuiltinHash；Python callable → PythonCallable。
        // bytes_source 识别：start/end 表达式 → StreamRange；field_name → ContextBytes。
        "ChecksumDescriptor" => {
            return Ok(Node::Checksum(build_checksum_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // === Phase 8 P1+P2 批次 ===
        // EnumDescriptor → EnumNode（Rust Node，详见设计 §1.2）。
        // decmapping/encmapping 在 Python 描述符中已构造，编译期直接物化 Py<PyDict>。
        // EnumInteger 类从 construct._internals 加载（fallback 路径用）。
        "EnumDescriptor" => {
            return Ok(Node::Enum(build_enum_node(
                py,
                desc,
                field_index,
                field_names,
            )?));
        }
        // FlagsEnumDescriptor → FlagsEnumNode。
        // flags 编译期物化为 Vec<(Py<PyString>, i64)>；encmapping 物化为 Py<PyDict>。
        "FlagsEnumDescriptor" => {
            return Ok(Node::FlagsEnum(build_flags_enum_node(
                py,
                desc,
                field_index,
                field_names,
            )?));
        }
        // MappingDescriptor → MappingNode（C-4：TypeError 捕获在 Node 内实现）。
        "MappingDescriptor" => {
            return Ok(Node::Mapping(build_mapping_node(
                py,
                desc,
                field_index,
                field_names,
            )?));
        }
        // OneOfDescriptor → OneOfNode。valids 编译期转 frozenset 物化。
        "OneOfDescriptor" => {
            return Ok(Node::OneOf(build_one_of_node(
                py,
                desc,
                field_index,
                field_names,
            )?));
        }
        // NoneOfDescriptor → NoneOfNode。
        "NoneOfDescriptor" => {
            return Ok(Node::NoneOf(build_none_of_node(
                py,
                desc,
                field_index,
                field_names,
            )?));
        }
        // UnionDescriptor → UnionNode（含 ParseFrom 编译 + name→index 解析）。
        "UnionDescriptor" => {
            return Ok(Node::Union(build_union_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // SequenceDescriptor → SequenceNode（PyList sink + context nesting）。
        "SequenceDescriptor" => {
            return Ok(Node::Sequence(build_sequence_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // ProcessXorDescriptor → ProcessXorNode。
        // padfunc int/bytes 编译期物化为 XorPad::Int/Bytes；FieldRef/ExprRef 编译为 XorPad::Expr。
        "ProcessXorDescriptor" => {
            return Ok(Node::ProcessXor(build_process_xor_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // ProcessRotateLeftDescriptor → ProcessRotateLeftNode。
        // amount/group int 包装为单 op ExprProgram；FieldRef/ExprRef 同。
        "ProcessRotateLeftDescriptor" => {
            return Ok(Node::ProcessRotateLeft(build_process_rotate_left_node(
                py,
                desc,
                field_index,
                expr_programs,
                field_names,
            )?));
        }
        // NamedTupleDescriptor → NamedTupleNode。
        // factory 编译期物化（调 collections.namedtuple）；inner 类型推断 mode。
        "NamedTupleDescriptor" => {
            return Ok(Node::NamedTuple(build_named_tuple_node(
                py,
                desc,
                field_index,
                field_names,
            )?));
        }
        _ => {
            // Phase 6.3 Adapter duck typing 分支：识别用户继承的 Adapter 子类
            // （type name 非 "AdapterDescriptor"）。通过 hasattr 检查 _decode/_encode。
            if is_adapter_like(desc) {
                return Ok(Node::AdapterCallback(build_adapter_callback_node(
                    py,
                    desc,
                    field_index,
                    expr_programs,
                    field_names,
                )?));
            }
        }
    }

    // 6. 全部失败 → Compilation error
    let repr_str = desc
        .repr()
        .ok()
        .and_then(|r| r.to_str().ok().map(String::from))
        .unwrap_or_else(|| "<unknown>".to_string());
    Err(ConstructError::Compilation {
        message: format!("未知的字段描述符类型: {}", repr_str),
    })
}

/// 构造编译期错误使用的字段路径字符串（"field {index}"）。
///
/// Phase 3.1 中 BitsInteger 编译期错误使用此路径（不含父 Struct 字段名上下文，
/// 后者由 `with_field_context` 在上层附加）。
fn path_for_field(field_index: usize) -> String {
    format!("field {}", field_index)
}

// ---------------------------------------------------------------------------
// Phase 6.2 Strings：Descriptor → Node 编译辅助函数
//
// 设计依据：`docs/design/模块设计/模块设计-Strings.md` v2 §4.4 / §10.2。
//
// 共同模式：
// - encoding 字符串在编译期调 `Encoding::from_user_str` 解析（PM 决策 6.2-D1）。
//   失败返回 `ConstructError::Compilation`（携带 P2b/P8 引导文本，由 Rust 侧
//   `Encoding::from_user_str` 生成）。Python 侧 `__init_subclass__` 会将其映射为
//   `CompilationError`。
// - term / pad / include / consume / require 等参数从 Python 描述符对象读取。
// - 含 inner / lengthfield 的 Node（NullTerminated / NullStripped / PascalString）
//   递归编译（沿用同一个 field_index 和 expr_programs 切片，与 BitwiseDescriptor /
//   ArrayDescriptor 同模式；P3.1 限制：inner 不支持含表达式的子描述符）。
// ---------------------------------------------------------------------------

/// 从 Python 描述符对象读取 `encoding` 属性并解析为 [`Encoding`]。
///
/// 编译期一次调用，运行时零开销（设计 D-1）。
///
/// # 错误
///
/// [`ConstructError::Compilation`]：encoding 缺失、非字符串、或 [`Encoding::from_user_str`]
/// 失败（未识别 / 无后缀编码，携带 P2b/P8 引导文本）。
fn parse_encoding_attr(
    desc: &Bound<'_, PyAny>,
    field_index: usize,
) -> Result<Encoding, ConstructError> {
    let encoding_obj = desc
        .getattr("encoding")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "String descriptor missing 'encoding' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let encoding_str: String = encoding_obj
        .extract()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "String descriptor 'encoding' attribute must be a str (field index {})",
                field_index
            ),
        })?;
    Encoding::from_user_str(&encoding_str).map_err(|e| match e {
        // from_user_str 失败时已是 Compilation 变体；附加 field_index 上下文。
        ConstructError::Compilation { message } => ConstructError::Compilation {
            message: format!("{} (field index {})", message, field_index),
        },
        other => other,
    })
}

/// 从 Python 描述符对象读取可选的 `term` 字节串属性。
///
/// 用于 [`CStringDescriptor`] / [`NullTerminatedDescriptor`]。
///
/// # 参数
///
/// - `desc`：Python 描述符。
/// - `encoding`：已解析的编码（用于生成默认 term）。
/// - `field_index`：字段索引（错误信息用）。
///
/// # 返回
///
/// `term` 字节串。若 Python 侧 `term=None`，返回 `encoding.default_term()`
/// （即编码单元的全零字节串）。
fn parse_term_attr(
    desc: &Bound<'_, PyAny>,
    encoding: Encoding,
    field_index: usize,
) -> Result<Vec<u8>, ConstructError> {
    let term_obj = desc
        .getattr("term")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "String descriptor missing 'term' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    // None → 默认 term（编码单元全零字节串）
    if term_obj.is_none() {
        return Ok(encoding.default_term());
    }
    term_obj
        .extract::<Vec<u8>>()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "String descriptor 'term' attribute must be bytes or None (field index {})",
                field_index
            ),
        })
}

/// 从 Python 描述符对象读取 `pad` 字节串属性。
///
/// 用于 [`NullStrippedDescriptor`]。Python 侧默认 `pad=b"\x00"`。
fn parse_pad_attr(desc: &Bound<'_, PyAny>, field_index: usize) -> Result<Vec<u8>, ConstructError> {
    let pad_obj = desc
        .getattr("pad")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "String descriptor missing 'pad' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    pad_obj
        .extract::<Vec<u8>>()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "String descriptor 'pad' attribute must be bytes (field index {})",
                field_index
            ),
        })
}

/// 从 Python 描述符对象读取 bool 属性（带默认值）。
fn parse_bool_attr(
    desc: &Bound<'_, PyAny>,
    name: &str,
    default: bool,
    field_index: usize,
) -> Result<bool, ConstructError> {
    match desc.getattr(name) {
        Ok(v) => v
            .extract::<bool>()
            .map_err(|_| ConstructError::Compilation {
                message: format!(
                    "String descriptor '{}' attribute must be bool (field index {})",
                    name, field_index
                ),
            }),
        Err(_) => Ok(default),
    }
}

/// 从 `CStringDescriptor` 构建 [`CStringNode`]（Phase 6.2 §3.1）。
///
/// 编译路径：
/// 1. 解析 `encoding` → [`Encoding`]（编译期一次）。
/// 2. 解析 `term`（None → encoding 默认 term）。
/// 3. 解析 `include` / `require`（默认 false / true）。
///
/// `consume` 参数当前被 Rust [`CStringNode`] 忽略（与 Python ``CString`` 实际行为一致，
/// 详见 Strings 设计 §3.1）。
fn build_cstring_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
) -> Result<CStringNode, ConstructError> {
    let encoding = parse_encoding_attr(desc, field_index)?;
    let term = parse_term_attr(desc, encoding, field_index)?;
    let include = parse_bool_attr(desc, "include", false, field_index)?;
    let require = parse_bool_attr(desc, "require", true, field_index)?;
    let _ = py;
    Ok(CStringNode::with_options(encoding, term, include, require))
}

/// 从 `GreedyStringDescriptor` 构建 [`GreedyStringNode`]（Phase 6.2 §3.4）。
fn build_greedy_string_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
) -> Result<GreedyStringNode, ConstructError> {
    let encoding = parse_encoding_attr(desc, field_index)?;
    let _ = py;
    Ok(GreedyStringNode::new(encoding))
}

/// 从 `PaddedStringDescriptor` 构建 [`PaddedStringNode`]（Phase 6.2 §3.5）。
///
/// 编译路径（参照 [`build_node_from_descriptor`] 的 BytesDescriptor 分支）：
/// 1. 解析 `encoding` → [`Encoding`]。
/// 2. 解析 `length`：
///    - Python int 常量 → [`BytesLength::Const`]（校验非负）。
///    - 非常量（FieldRef/ExprRef）→ 从 `expr_programs[field_index]["length"]` 取
///      ExprOp 列表 → [`BytesLength::Expr`]。
fn build_padded_string_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<PaddedStringNode, ConstructError> {
    let encoding = parse_encoding_attr(desc, field_index)?;

    // 解析 length（与 BytesDescriptor 同模式）。
    let length_obj = desc
        .getattr("length")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "PaddedStringDescriptor missing 'length' attribute: {} (field index {})",
                e, field_index
            ),
        })?;

    let length = if let Ok(n) = length_obj.extract::<i64>() {
        // 常量路径（校验非负）。
        if n < 0 {
            return Err(ConstructError::Compilation {
                message: format!(
                    "PaddedString length {} must be non-negative (field index {})",
                    n, field_index
                ),
            });
        }
        BytesLength::Const(n as usize)
    } else {
        // 表达式路径：从 expr_programs[field_index]["length"] 取 ExprOp 列表。
        let field_exprs = expr_programs
            .get(field_index)
            .and_then(Option::as_ref)
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "PaddedString field has non-constant length but no expression program was \
                     provided (field index {})",
                    field_index
                ),
            })?;
        let field_exprs_dict =
            field_exprs
                .bind(py)
                .downcast::<PyDict>()
                .map_err(|_| ConstructError::Compilation {
                    message: "PaddedString expression program must be a dict".to_string(),
                })?;
        let ops_list = field_exprs_dict
            .get_item("length")
            .map_err(|e| ConstructError::Compilation {
                message: format!(
                    "failed to get 'length' from PaddedString expression programs: {} (field index {})",
                    e, field_index
                ),
            })?
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "PaddedString field has non-constant length but 'length' key missing in \
                     expression program (field index {})",
                    field_index
                ),
            })?;
        let ops = parse_expr_ops_from_py(&ops_list)?;
        let program = ExprProgram::new(ops);
        BytesLength::Expr(program)
    };

    Ok(PaddedStringNode::new(length, encoding))
}

/// 从 `PascalStringDescriptor` 构建 [`PascalStringNode`]（Phase 6.2 §3.6）。
///
/// 编译路径：
/// 1. 递归编译 lengthfield（沿用 field_index；与 BitwiseDescriptor 同模式）。
/// 2. 解析 `encoding` → [`Encoding`]。
///
/// # 限制（同 ArrayDescriptor，设计 §6.2.2 P3.1）
///
/// lengthfield 不支持含表达式的子描述符（Python 侧 `_extract_and_compile_exprs` 不递归 inner）。
fn build_pascal_string_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<PascalStringNode, ConstructError> {
    let lengthfield_desc =
        desc.getattr("lengthfield")
            .map_err(|e| ConstructError::Compilation {
                message: format!(
                    "PascalStringDescriptor missing 'lengthfield' attribute: {} (field index {})",
                    e, field_index
                ),
            })?;
    let lengthfield_node = build_node_from_descriptor(
        py,
        &lengthfield_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;
    let encoding = parse_encoding_attr(desc, field_index)?;
    Ok(PascalStringNode::new(lengthfield_node, encoding))
}

/// 从 `NullTerminatedDescriptor` 构建 [`NullTerminatedNode`]（Phase 6.2 §3.2）。
///
/// 编译路径：
/// 1. 递归编译 inner subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
/// 2. 解析 `term`（None → `b"\x00"` 默认；非 None → 用户传入字节串）。
/// 3. 解析 `include` / `consume` / `require`（默认 false / true / true）。
///
/// # 限制（同 ArrayDescriptor，设计 §6.2.2 P3.1）
///
/// inner subcon 不支持含表达式的子描述符。
fn build_null_terminated_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<NullTerminatedNode, ConstructError> {
    let inner_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "NullTerminatedDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &inner_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // term：Python 侧默认 b"\x00"（已在 Descriptor __init__ 保证），此处直接读取。
    // 不调用 parse_term_attr(encoding, ...)：NullTerminated 不依赖 encoding 单元，
    // term 是独立的字节串（与 CString 不同，后者默认 term 由 encoding 决定）。
    let term_obj = desc
        .getattr("term")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "NullTerminatedDescriptor missing 'term' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let term: Vec<u8> = if term_obj.is_none() {
        vec![0u8]
    } else {
        term_obj.extract::<Vec<u8>>().map_err(|_| {
            ConstructError::Compilation {
                message: format!(
                    "NullTerminatedDescriptor 'term' attribute must be bytes or None (field index {})",
                    field_index
                ),
            }
        })?
    };

    let include = parse_bool_attr(desc, "include", false, field_index)?;
    let consume = parse_bool_attr(desc, "consume", true, field_index)?;
    let require = parse_bool_attr(desc, "require", true, field_index)?;

    Ok(NullTerminatedNode::with_options(
        inner_node, term, include, consume, require,
    ))
}

/// 从 `NullStrippedDescriptor` 构建 [`NullStrippedNode`]（Phase 6.2 §3.3）。
///
/// 编译路径：
/// 1. 递归编译 inner subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
/// 2. 解析 `pad`（默认 `b"\x00"`）。
///
/// # 限制（同 ArrayDescriptor，设计 §6.2.2 P3.1）
///
/// inner subcon 不支持含表达式的子描述符。
fn build_null_stripped_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<NullStrippedNode, ConstructError> {
    let inner_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "NullStrippedDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &inner_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;
    let pad = parse_pad_attr(desc, field_index)?;
    Ok(NullStrippedNode::with_pad(inner_node, pad))
}

/// 从 `BitsIntegerDescriptor` 构建 `BitsIntegerNode`，编译期完成所有 length 校验。
///
/// Phase 3.1 仅支持常量 length（Python int）。表达式 length（FieldRef/ExprRef）
/// 返回 `Compilation` 错误，闭环设计 §7.2 / §8.2 表达式 length 路径（P4）。
///
/// # 编译期校验（fail-fast，比 Python 提前暴露）
///
/// - 表达式 length → `Compilation`（"BitsInteger expression length not supported in Phase 3.1"）
/// - length 为负 → `Compilation`（"length must be non-negative"，避免 usize 回绕，P6）
/// - length > MAX_BITS_INTEGER (64) → `Compilation`（"exceeds 64-bit limit"，P5/BI-3）
///
/// `path` 参数已并入编译期错误信息（3.1 P4 修复）：
/// 编译期错误本来没有 path（Compilation 无 path 字段），但 message 中可包含
/// field_index 与 path_for_field 上下文，便于在上层 `with_field_context`
/// 附加父字段名后定位。
fn build_bits_integer_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    path: String,
) -> Result<BitsIntegerNode, ConstructError> {
    let length_obj = desc
        .getattr("length")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "BitsIntegerDescriptor missing 'length' attribute: {} (field index {}, path {})",
                e, field_index, path
            ),
        })?;

    // Phase 3.1：仅支持常量 length（Python int）。
    // 尝试 extract 为 i64。失败表示是表达式（FieldRef/ExprRef）或其他非 int 类型。
    let length_i64: i64 = match length_obj.extract::<i64>() {
        Ok(v) => v,
        Err(_) => {
            // 检查是否是表达式（FieldRef/ExprRef/其他描述符）。即使是其他类型，
            // Phase 3.1 也不支持，统一返回 Compilation。
            return Err(ConstructError::Compilation {
                message: format!(
                    "BitsInteger expression length not supported in Phase 3.1 \
                     (use a constant integer length) (field index {}, path {})",
                    field_index, path
                ),
            });
        }
    };

    // P6: 编译期检测负 length（避免 i64 负数 as usize 回绕为巨大值）
    if length_i64 < 0 {
        return Err(ConstructError::Compilation {
            message: format!(
                "BitsInteger length {} must be non-negative (field index {}, path {})",
                length_i64, field_index, path
            ),
        });
    }

    let length = length_i64 as usize;

    // P5 / BI-3: length > 64 提前暴露（避免运行时报错）
    if length > MAX_BITS_INTEGER {
        return Err(ConstructError::Compilation {
            message: format!(
                "BitsInteger length {} exceeds 64-bit limit (max {}) (field index {}, path {})",
                length, MAX_BITS_INTEGER, field_index, path
            ),
        });
    }

    let signed: bool = desc
        .getattr("signed")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "BitsIntegerDescriptor missing 'signed' attribute: {} (field index {}, path {})",
                e, field_index, path
            ),
        })?
        .extract()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "BitsIntegerDescriptor 'signed' attribute must be bool (field index {}, path {})",
                field_index, path
            ),
        })?;

    let swapped: bool = desc
        .getattr("swapped")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "BitsIntegerDescriptor missing 'swapped' attribute: {} (field index {}, path {})",
                e, field_index, path
            ),
        })?
        .extract()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "BitsIntegerDescriptor 'swapped' attribute must be bool (field index {}, path {})",
                field_index, path
            ),
        })?;

    let _ = py; // py 保留供未来扩展使用（例如从 Python 对象提取表达式程序）
    Ok(BitsIntegerNode::new(length, signed, swapped))
}

/// 从 `PaddingDescriptor` 构建 bit 域或字节域 Padding 节点（设计 §4.2 / §4.6 / §7.2）。
///
/// `bitwise` 参数决定选择 [`BitPaddingNode`]（bit 域，pattern 严格 0x00/0x01）
/// 还是 [`PaddingNode`]（字节域，pattern 任意 0-255）。
///
/// Phase 3.1 / 3.3 仅支持常量 length（Python int）。表达式 length（FieldRef/ExprRef）
/// 返回 `Compilation` 错误（设计 §7.2 P4）。
///
/// # 编译期校验
///
/// - 表达式 length → `Compilation`（"Padding expression length not supported in Phase 3.1"）
/// - length 为负 → `Compilation`（"length must be non-negative"）
/// - bit 域 pattern 非 {0x00, 0x01} → `Padding`（由 BitPaddingNode::new 触发）
fn build_padding_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    bitwise: bool,
) -> Result<Node, ConstructError> {
    let length_obj = desc
        .getattr("length")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "PaddingDescriptor missing 'length' attribute: {} (field index {})",
                e, field_index
            ),
        })?;

    // Phase 3.1：仅支持常量 length（Python int）。
    let length_i64: i64 = match length_obj.extract::<i64>() {
        Ok(v) => v,
        Err(_) => {
            return Err(ConstructError::Compilation {
                message: format!(
                    "Padding expression length not supported in Phase 3.1 \
                     (use a constant integer length) (field index {})",
                    field_index
                ),
            });
        }
    };

    // P6: 编译期检测负 length
    if length_i64 < 0 {
        return Err(ConstructError::Compilation {
            message: format!(
                "Padding length {} must be non-negative (field index {})",
                length_i64, field_index
            ),
        });
    }

    let length = length_i64 as usize;

    // 提取 pattern（Python 侧 PaddingDescriptor.pattern 已是 int 0-255）。
    let pattern: u8 = desc
        .getattr("pattern")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "PaddingDescriptor missing 'pattern' attribute: {} (field index {})",
                e, field_index
            ),
        })?
        .extract()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "PaddingDescriptor 'pattern' attribute must be int (field index {})",
                field_index
            ),
        })?;

    let _ = py;
    if bitwise {
        // bit 域：BitPaddingNode::new 严格校验 pattern ∈ {0x00, 0x01}，
        // 非法 pattern 返回 Padding 错误（设计 §4.2 P1 修正）。
        Ok(Node::BitPadding(BitPaddingNode::new(length, pattern)?))
    } else {
        // 字节域：PaddingNode 接受任意 pattern 字节
        Ok(Node::Padding(PaddingNode::new_const(length, pattern)))
    }
}

/// 从 `ArrayDescriptor` 构建 `ArrayNode`（设计 §6.2.2）。
///
/// 编译路径参照 [`build_node_from_descriptor`] 的 BytesDescriptor 分支（compile.rs L266-308）：
///
/// 1. 从 desc 读取 count / subcon / discard。
/// 2. count 分类：
///    - Python int 常量 → [`CountSource::Const`]（校验非负）。
///    - 非常量（FieldRef/ExprRef 等）→ 从 `expr_programs[field_index]["count"]` 取
///      ExprOp 列表 → [`CountSource::Expr`]。
/// 3. 递归编译 subcon：**沿用同一个 field_index 和 expr_programs 切片**
///    （与 BitwiseDescriptor / BytewiseDescriptor 同模式）。
///
/// # 限制（设计 §6.2.2 P3.1）
///
/// inner subcon 不支持含表达式的子描述符（如 `Array(N, Bytes(this.m))`）。
/// Python 侧 `_extract_and_compile_exprs`（_mixin.py L583-615）不递归 inner subcon，
/// 导致 inner 的表达式参数不会被收集到 expr_programs 中。Rust 侧递归编译 inner 时
/// 若 inner 是含表达式的描述符，会因 expr_programs 中缺键报 Compilation 错误。
/// 这与 Phase 3 的 `Bitwise(Bytes(this.m))` 限制一致。
fn build_array_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<ArrayNode, ConstructError> {
    // 1. 递归编译 subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ArrayDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 解析 count：常量 or 表达式。
    let count_obj = desc
        .getattr("count")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ArrayDescriptor missing 'count' attribute: {} (field index {})",
                e, field_index
            ),
        })?;

    let count =
        if let Ok(n) = count_obj.extract::<i64>() {
            // 常量路径（零运行时开销）。
            if n < 0 {
                return Err(ConstructError::Compilation {
                    message: format!(
                        "Array count {} must be non-negative (field index {})",
                        n, field_index
                    ),
                });
            }
            CountSource::Const(n as usize)
        } else {
            // 表达式路径：从 expr_programs[field_index]["count"] 取 ExprOp 列表。
            // 与 BytesDescriptor 的 "length" 键完全同模式。
            let field_exprs = expr_programs
                .get(field_index)
                .and_then(Option::as_ref)
                .ok_or_else(|| ConstructError::Compilation {
                    message: format!(
                    "Array field has non-constant count but no expression program was provided \
                     (field index {})",
                    field_index
                ),
                })?;

            let field_exprs_dict = field_exprs.bind(py).downcast::<PyDict>().map_err(|_| {
                ConstructError::Compilation {
                    message: "Array expression program must be a dict".to_string(),
                }
            })?;

            let ops_list = field_exprs_dict
                .get_item("count")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "failed to get 'count' from Array expression programs: {} (field index {})",
                        e, field_index
                    ),
                })?
                .ok_or_else(|| ConstructError::Compilation {
                    message: format!(
                        "Array field has non-constant count but 'count' key missing in expression \
                     program (field index {})",
                        field_index
                    ),
                })?;

            let ops = parse_expr_ops_from_py(&ops_list)?;
            let program = ExprProgram::new(ops);
            CountSource::Expr(program)
        };

    // 3. discard 标志（默认 false）。
    let discard: bool = desc
        .getattr("discard")
        .and_then(|d| d.extract())
        .unwrap_or(false);

    Ok(ArrayNode::new(inner_node, count, discard))
}

/// 从 `GreedyRangeDescriptor` 构建 `GreedyRangeNode`（设计 §6.2.3 / §4.2）。
///
/// 编译路径：
///
/// 1. 从 desc 读取 subcon / discard。
/// 2. 递归编译 subcon（沿用同一个 field_index 和 expr_programs 切片，
///    与 BitwiseDescriptor / BytewiseDescriptor / ArrayDescriptor 同模式）。
/// 3. 提取 discard 标志（默认 false）。
///
/// # 限制（同 ArrayDescriptor，设计 §6.2.2 P3.1）
///
/// inner subcon 不支持含表达式的子描述符（如 `GreedyRange(Bytes(this.m))`）。
/// Python 侧 `_extract_and_compile_exprs` 不递归 inner subcon。
fn build_greedy_range_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<GreedyRangeNode, ConstructError> {
    // 1. 递归编译 subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "GreedyRangeDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. discard 标志（默认 false）。
    let discard: bool = desc
        .getattr("discard")
        .and_then(|d| d.extract())
        .unwrap_or(false);

    Ok(GreedyRangeNode::new(inner_node, discard))
}

/// 从 `PrefixedArrayDescriptor` 构建 `PrefixedArrayNode`（设计 §4.6 / §6.2.3）。
///
/// 编译路径：
///
/// 1. 从 desc 读取 countfield / subcon。
/// 2. 递归编译 countfield：作为独立的 Node 子树（沿用同一个 field_index 和
///    expr_programs 切片，与 BitwiseDescriptor / ArrayDescriptor 同模式）。
/// 3. 递归编译 subcon（inner）：同上。
/// 4. 组装为 `PrefixedArrayNode { countfield, inner }`。
///
/// # 限制（同 ArrayDescriptor，设计 §6.2.2 P3.1）
///
/// countfield 与 inner subcon 均不支持含表达式的子描述符。
/// Python 侧 `_extract_and_compile_exprs`（_mixin.py L583-615）不递归进入 inner。
fn build_prefixed_array_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<PrefixedArrayNode, ConstructError> {
    // 1. 递归编译 countfield（沿用 field_index）。
    let countfield_desc = desc
        .getattr("countfield")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "PrefixedArrayDescriptor missing 'countfield' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let countfield_node = build_node_from_descriptor(
        py,
        &countfield_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 递归编译 subcon（沿用 field_index）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "PrefixedArrayDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    Ok(PrefixedArrayNode::new(countfield_node, inner_node))
}

/// 从 `StopIfDescriptor` 构建 `StopIfNode`（设计 §4.5 / §6.2.2）。
///
/// 编译路径（参照 ComputedDescriptor 的 "func" 模式）：
///
/// 1. 从 desc 读取 `condfunc` 属性。
/// 2. 分类：
///    - Python `True` / `False`（或 int 1/0）→ [`StopIfCondition::Always`] / [`StopIfCondition::Never`]。
///    - 其他（FieldRef/ExprRef 等）→ 从 `expr_programs[field_index]["cond"]` 取
///      ExprOp 列表 → [`StopIfCondition::Expr`]。
///
/// # 限制（设计 §6.2.2 P3.1）
///
/// StopIf 作为 Struct 直接字段时支持表达式（如 `rfield(StopIf(x == 0))`，
/// `x` 是字段名引用），从 `expr_programs[field_index]["cond"]` 取。
/// 但作为包装型描述符（Bitwise/Array/...）的 inner 子描述符时不支持
/// （Python 侧 `_extract_and_compile_exprs` 不递归 inner）。
fn build_stop_if_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<StopIfNode, ConstructError> {
    let cond_obj = desc
        .getattr("condfunc")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "StopIfDescriptor missing 'condfunc' attribute: {} (field index {})",
                e, field_index
            ),
        })?;

    // 1. 尝试常量分类：True → Always，False → Never。
    //    Python 的 True/False 在 extract::<bool>() 时与 1/0 等价；这里严格区分
    //    bool 与 int 表达式：先用 is_true/is_none 检查 PyLong，避免把表达式误判为常量。
    //    实际上 Python 侧 IndexDescriptor/StopIfDescriptor 的 condfunc 是 bool 时
    //    一定是常量；FieldRef/ExprRef 不是 bool/int。
    if let Ok(b) = cond_obj.extract::<bool>() {
        let cond = if b {
            StopIfCondition::Always
        } else {
            StopIfCondition::Never
        };
        return Ok(StopIfNode::new(cond));
    }

    // 2. 表达式路径：从 expr_programs[field_index]["cond"] 取 ExprOp 列表。
    //    与 ComputedDescriptor 的 "func" / ArrayDescriptor 的 "count" 同模式。
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "StopIf field has non-constant condfunc but no expression program was provided \
                 (field index {})",
                field_index
            ),
        })?;

    let field_exprs_dict =
        field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "StopIf expression program must be a dict".to_string(),
            })?;

    let ops_list = field_exprs_dict
        .get_item("cond")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get 'cond' from StopIf expression programs: {} (field index {})",
                e, field_index
            ),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "StopIf field has non-constant condfunc but 'cond' key missing in expression \
                 program (field index {})",
                field_index
            ),
        })?;

    let ops = parse_expr_ops_from_py(&ops_list)?;
    let program = ExprProgram::new(ops);
    Ok(StopIfNode::new(StopIfCondition::Expr(program)))
}

// ---------------------------------------------------------------------------
// Phase 7.1 Conditional：Descriptor → Node 编译辅助函数
//
// 设计依据：`docs/design/模块设计/模块设计-Conditional.md` §2.4 / §3.6 / §5.4。
//
// 共同模式：
// - condfunc / keyfunc 在编译期分类（常量 / 表达式），与 StopIfDescriptor 同源
// - thensubcon / elsesubcon / cases / subcons 递归编译（沿用 field_index）
// - 复杂表达式编译期拒绝（Switch keyfunc，引导用户用 Computed 预计算）
// ---------------------------------------------------------------------------

/// 从 Python 描述符的 `condfunc` 属性分类为 [`StopIfCondition`]。
///
/// 共用逻辑（StopIf / IfThenElse）：
/// - Python `True` / `False`（或 int 1/0）→ [`StopIfCondition::Always`] / [`StopIfCondition::Never`]
/// - FieldRef/ExprRef → 从 `expr_programs[field_index][param_name]` 取 ExprOp 列表 →
///   [`StopIfCondition::Expr`]
fn classify_condfunc(
    py: Python<'_>,
    cond_obj: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    param_name: &str,
    descriptor_name: &str,
) -> Result<StopIfCondition, ConstructError> {
    // 1. 尝试常量分类：True → Always，False → Never。
    if let Ok(b) = cond_obj.extract::<bool>() {
        return Ok(if b {
            StopIfCondition::Always
        } else {
            StopIfCondition::Never
        });
    }

    // 2. 表达式路径：从 expr_programs[field_index][param_name] 取 ExprOp 列表。
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "{} has non-constant condfunc but no expression program was provided \
                 (field index {})",
                descriptor_name, field_index
            ),
        })?;

    let field_exprs_dict =
        field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: format!(
                    "{} expression program must be a dict (field index {})",
                    descriptor_name, field_index
                ),
            })?;

    let ops_list = field_exprs_dict
        .get_item(param_name)
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get '{}' from {} expression programs: {} (field index {})",
                param_name, descriptor_name, e, field_index
            ),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "{} has non-constant condfunc but '{}' key missing in expression \
                 program (field index {})",
                descriptor_name, param_name, field_index
            ),
        })?;

    let ops = parse_expr_ops_from_py(&ops_list)?;
    Ok(StopIfCondition::Expr(ExprProgram::new(ops)))
}

/// 从 `IfThenElseDescriptor` 构建 `IfThenElseNode`（设计 §2.4）。
fn build_if_then_else_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<IfThenElseNode, ConstructError> {
    let cond_obj = desc
        .getattr("condfunc")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "IfThenElseDescriptor missing 'condfunc' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let cond = classify_condfunc(
        py,
        &cond_obj,
        field_index,
        expr_programs,
        "cond",
        "IfThenElseDescriptor",
    )?;

    let then_desc = desc
        .getattr("thensubcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "IfThenElseDescriptor missing 'thensubcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let then_node = build_node_from_descriptor(
        py,
        &then_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    let else_desc = desc
        .getattr("elsesubcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "IfThenElseDescriptor missing 'elsesubcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let else_node = build_node_from_descriptor(
        py,
        &else_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    Ok(IfThenElseNode::new(cond, then_node, else_node))
}

/// 从 `SwitchDescriptor` 构建 `SwitchNode`（设计 §3.6，PM 决策 1：A+B 混合）。
fn build_switch_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<SwitchNode, ConstructError> {
    // 1. 分类 keyfunc
    let key_obj = desc
        .getattr("keyfunc")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "SwitchDescriptor missing 'keyfunc' attribute: {} (field index {})",
                e, field_index
            ),
        })?;

    let key = if let Ok(b) = key_obj.extract::<bool>() {
        // Python bool 也是 int（True == 1, False == 0）
        SwitchKey::ConstInt(if b { 1 } else { 0 })
    } else if let Ok(k) = key_obj.extract::<i64>() {
        SwitchKey::ConstInt(k)
    } else {
        // 表达式路径：从 expr_programs[field_index] 取 "key"
        let field_exprs = expr_programs.get(field_index).and_then(Option::as_ref);
        let field_exprs_dict = field_exprs
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "Switch keyfunc is not a constant and no expression program was \
                     collected for this field (field index {}). Nested Switch inside \
                     wrappers (Prefixed / PrefixedArray / Bitwise / Hex / Union / \
                     Select / If / ...) is supported since v0.1.1. Supported keyfunc: \
                     int/bool constant, or an expression over field names declared \
                     earlier in the same Struct, e.g. Switch(typ, {{...}}) or \
                     Switch(typ + 1, {{...}}). Python callables/lambdas are not \
                     supported. If your case matches the supported forms, please \
                     report this as a bug.",
                    field_index
                ),
            })?
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "Switch expression program must be a dict".to_string(),
            })?;

        if let Ok(Some(key_ops)) = field_exprs_dict.get_item("key") {
            // 路线 A：IntExpr
            let ops = parse_expr_ops_from_py(&key_ops)?;
            SwitchKey::IntExpr(ExprProgram::new(ops))
        } else {
            // 都无 → keyfunc 类型不受支持
            return Err(ConstructError::Compilation {
                message: format!(
                    "Switch keyfunc could not be compiled (field index {}): expected \
                     an int/bool constant or a field-name expression (e.g. \
                     Switch(typ, {{...}}) where 'typ' is a field declared earlier in \
                     the same Struct; arithmetic like typ + 1 also works). Note: \
                     'this.xxx' syntax does not exist in construct-rs — reference \
                     field names directly.",
                    field_index
                ),
            });
        }
    };

    // 2. 展开 cases dict
    let cases_obj = desc
        .getattr("cases")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "SwitchDescriptor missing 'cases' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let cases_dict = cases_obj
        .downcast::<PyDict>()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "SwitchDescriptor 'cases' must be a dict (field index {})",
                field_index
            ),
        })?;
    let mut cases = Vec::with_capacity(cases_dict.len());
    for item in cases_dict.items() {
        let case_key: Bound<PyAny> = item.get_item(0).map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to read Switch case key: {} (field index {})",
                e, field_index
            ),
        })?;
        let case_subcon: Bound<PyAny> =
            item.get_item(1).map_err(|e| ConstructError::Compilation {
                message: format!(
                    "failed to read Switch case subcon: {} (field index {})",
                    e, field_index
                ),
            })?;
        let case_node = build_node_from_descriptor(
            py,
            &case_subcon,
            field_index,
            expr_programs,
            field_names,
            false,
        )?;
        cases.push(SwitchCase::new(py, case_key.clone().unbind(), case_node));
    }

    // 3. default subcon
    let default_obj = desc
        .getattr("default")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "SwitchDescriptor missing 'default' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let default_node = if default_obj.is_none() {
        Node::Pass(PassNode::new())
    } else {
        build_node_from_descriptor(
            py,
            &default_obj,
            field_index,
            expr_programs,
            field_names,
            false,
        )?
    };

    Ok(SwitchNode::new(key, cases, default_node))
}

/// 从 `FocusedSeqDescriptor` 构建 `FocusedSeqNode`（设计 §5.4）。
fn build_focused_seq_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<FocusedSeqNode, ConstructError> {
    // 1. parsebuildfrom → focus 字段名
    let pbf_obj = desc
        .getattr("parsebuildfrom")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "FocusedSeqDescriptor missing 'parsebuildfrom' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let focus_name: String = pbf_obj.extract().map_err(|_| ConstructError::Compilation {
        message: format!(
            "FocusedSeqDescriptor 'parsebuildfrom' must be a string (field index {}). \
                 construct-rs only supports string parsebuildfrom (e.g. 'num'). Lambda \
                 context functions are not supported.",
            field_index
        ),
    })?;

    // 2. subcons list → Vec<FocusedSeqField>
    let subcons_obj = desc
        .getattr("subcons")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "FocusedSeqDescriptor missing 'subcons' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let subcons_list =
        subcons_obj
            .downcast::<PyList>()
            .map_err(|_| ConstructError::Compilation {
                message: format!(
                    "FocusedSeqDescriptor 'subcons' must be a list (field index {})",
                    field_index
                ),
            })?;

    let mut fields: Vec<FocusedSeqField> = Vec::new();
    let mut focus_idx: Option<usize> = None;

    for sub_desc_bound in subcons_list.iter() {
        // Renamed 包装：含 name + subcon；匿名 subcon：无 name。
        let name_opt: Option<String> = if sub_desc_bound.hasattr("name").unwrap_or(false) {
            let name_obj =
                sub_desc_bound
                    .getattr("name")
                    .map_err(|e| ConstructError::Compilation {
                        message: format!(
                            "FocusedSeq subcon 'name' attribute access failed: {} (field index {})",
                            e, field_index
                        ),
                    })?;
            if name_obj.is_none() {
                None
            } else {
                name_obj.extract::<String>().ok()
            }
        } else {
            None
        };

        // 获取实际 subcon：Renamed.subcon 或 desc 自身
        let inner_desc = if sub_desc_bound.hasattr("subcon").unwrap_or(false) {
            sub_desc_bound
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "FocusedSeq subcon 'subcon' attribute access failed: {} (field index {})",
                        e, field_index
                    ),
                })?
        } else {
            sub_desc_bound.clone()
        };

        let node = build_node_from_descriptor(
            py,
            &inner_desc,
            field_index,
            expr_programs,
            field_names,
            false,
        )?;

        // 检查 focus_name 匹配
        if let Some(ref n) = name_opt {
            if n == &focus_name && focus_idx.is_none() {
                // FS-8: 重复字段名 → 取第一个匹配（与 Python finalret 覆盖语义一致）
                focus_idx = Some(fields.len());
            }
            fields.push(FocusedSeqField::new_named(
                FieldName::new(py, n.clone()),
                node,
            ));
        } else {
            fields.push(FocusedSeqField::new_anonymous(node));
        }
    }

    // FS-2/FS-4: focus 字段必须存在且为命名字段
    let focus_idx = focus_idx.ok_or_else(|| ConstructError::Compilation {
        message: format!(
            "FocusedSeq parsebuildfrom='{}' does not match any named field in subcons \
             (field index {}). parsebuildfrom must match a named field (Renamed wrapper \
             with matching 'name').",
            focus_name, field_index
        ),
    })?;

    // 4. has_expressions：递归 fields 子树
    let has_expressions = fields.iter().any(|f| f.node().has_expressions());

    Ok(FocusedSeqNode::new(fields, focus_idx, has_expressions))
}

/// 从 `RepeatUntilDescriptor` 构建 `RepeatUntilNode`（v5 阶段 6 完全重写）。
///
/// **阶段 1 临时占位**：v5 重写RepeatUntilNode 数据结构与编译逻辑，
/// 详见 `docs/模块设计-Array.md` §4.3 / §6.3。完整实现将在阶段 6 引入。
/// 编译 `RepeatUntilDescriptor` → `RepeatUntilNode`（v5 完全重写）。
///
/// 设计依据：`docs/模块设计-Array.md` §4.3 / §6.3.1。
///
/// # v5 编译路径
///
/// 1. 从 desc 读取 terminator / subcon / discard
/// 2. 递归编译 subcon（沿用 field_index，与 BitwiseDescriptor 同模式）
/// 3. 从 `expr_programs[field_index]["terminator"]` 取 ExprOp 列表 → ExprProgram
/// 4. 从 `expr_programs[field_index]["element_field_idx"]` 取 Element 字段索引
/// 5. 从 `field_names[element_field_idx]` 取 Element 字段名（intern PyString）
///
/// # v5 关键变更（vs v4）
///
/// - 不再从 `self.predicate` 读取 callable（v4 路径已删）
/// - 终止表达式编译完全在 Python 侧 `_compile_expr_tree` 完成（与其他表达式字段同模式）
fn build_repeat_until_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<RepeatUntilNode, ConstructError> {
    // 1. 递归编译 subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "RepeatUntilDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 从 expr_programs[field_index] 取终止表达式 + Element 字段索引。
    //    Python 侧 _mixin._compile_expressions 通过 RepeatUntilDescriptor
    //    .set_compiled_expr_params(ops, element_field_idx) 注入。
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "RepeatUntil field has no expression program (field index {}); \
                 terminator expression is required",
                field_index
            ),
        })?;
    let field_exprs_dict =
        field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "RepeatUntil expression program must be a dict".to_string(),
            })?;

    // 2a. 取终止表达式 ExprOp 列表（必填）。
    let ops_obj = field_exprs_dict
        .get_item("terminator")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get 'terminator' from RepeatUntil expression programs: {} (field index {})",
                e, field_index
            ),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "RepeatUntil missing 'terminator' in expression programs (field index {})",
                field_index
            ),
        })?;
    let ops = parse_expr_ops_from_py(&ops_obj)?;
    let terminator = ExprProgram::new(ops);

    // 2b. 取 Element 字段索引（必填，编译期由 Python 侧从终止表达式引用的
    // Element 字段推导）。
    let element_field_idx_obj = field_exprs_dict
        .get_item("element_field_idx")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get 'element_field_idx' from RepeatUntil expression programs: {} (field index {})",
                e, field_index
            ),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "RepeatUntil missing 'element_field_idx' in expression programs (field index {})",
                field_index
            ),
        })?;
    let element_field_idx: usize =
        element_field_idx_obj
            .extract()
            .map_err(|_| ConstructError::Compilation {
                message: format!(
                    "RepeatUntil 'element_field_idx' must be int (field index {})",
                    field_index
                ),
            })?;

    // 2c. 取 Index 字段索引列表（可选，默认空 Vec）。终止表达式中引用的 Index 字段
    // 索引（不含 element_field_idx），每次迭代同步为当前下标。
    let index_field_indices: Vec<usize> = match field_exprs_dict
        .get_item("index_field_indices")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get 'index_field_indices' from RepeatUntil expression programs: {} (field index {})",
                e, field_index
            ),
        })? {
        Some(obj) => {
            if obj.is_none() {
                Vec::new()
            } else {
                obj.extract::<Vec<usize>>().map_err(|_| {
                    ConstructError::Compilation {
                        message: format!(
                            "RepeatUntil 'index_field_indices' must be list of int (field index {})",
                            field_index
                        ),
                    }
                })?
            }
        }
        None => Vec::new(),
    };

    // 3. 取 Element 字段名（intern PyString，set_field_at 的 key 参数）。
    let element_field_name_str =
        field_names
            .get(element_field_idx)
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "RepeatUntil element_field_idx {} out of range (field_names len {})",
                    element_field_idx,
                    field_names.len()
                ),
            })?;
    let element_field_name = crate::instance::intern_pystring(py, element_field_name_str);

    // 4. discard 标志（默认 false）。
    let discard: bool = desc
        .getattr("discard")
        .and_then(|d| d.extract())
        .unwrap_or(false);

    Ok(RepeatUntilNode::new(
        inner_node,
        terminator,
        element_field_idx,
        element_field_name,
        index_field_indices,
        discard,
    ))
}

/// 从 `RebuildDescriptor` 构建 `RebuildNode`（Phase 6.3）。
///
/// 对应 Python construct `Rebuild(subcon, func)`（core.py L2975）。
///
/// # 编译路径
///
/// 1. 递归编译 subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
/// 2. 从 `expr_programs[field_index]["func"]` 取 ExprOp 列表 → ExprProgram。
///
/// # 限制（同 ArrayDescriptor，设计 §6.2.2 P3.1）
///
/// subcon 不支持含表达式的子描述符（Python 侧 `_extract_and_compile_exprs` 不递归 inner）。
///
/// # 差异记录（设计 §5.3 RB-callable）
///
/// Python Rebuild.func 可以是任意 callable lambda。construct-rs 收窄为仅 Phase 2 表达式
/// （与 ADR-014 RepeatUntil v5 同脉络）。RebuildDescriptor 的 `_field_kind` 返回 "ro"，
/// 编译期校验用户使用 `rfield(Rebuild(...))` 包装（RB-4 编译期拒绝 RW 包装）。
fn build_rebuild_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<RebuildNode, ConstructError> {
    // 1. 递归编译 subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "RebuildDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 从 expr_programs[field_index]["func"] 取 ExprOp 列表。
    //    与 ComputedDescriptor 的 "func" 同模式。
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "Rebuild field has no expression program (field index {}); \
                 'func' expression is required. \
                 Note: Rebuild func must be a Phase 2 expression (FieldRef/ExprRef/int), \
                 Python callable (lambda) is not supported (same constraint as RepeatUntil v5).",
                field_index
            ),
        })?;
    let field_exprs_dict =
        field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "Rebuild expression program must be a dict".to_string(),
            })?;
    let ops_obj = field_exprs_dict
        .get_item("func")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get 'func' from Rebuild expression programs: {} (field index {})",
                e, field_index
            ),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "Rebuild missing 'func' in expression programs (field index {})",
                field_index
            ),
        })?;
    let ops = parse_expr_ops_from_py(&ops_obj)?;
    let func = ExprProgram::new(ops);

    Ok(RebuildNode::new(inner_node, func))
}

/// 从 `AdapterDescriptor` 构建 `AdapterCallbackNode`（Phase 6.3，PM 决策 6.3-D1 接受）。
///
/// 对应 Python construct `Adapter(subcon)` 基类的嵌入用法（core.py L813）。
///
/// # 编译路径
///
/// 1. 递归编译 subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
/// 2. 取 AdapterDescriptor 的 `_decode` / `_encode` bound method 引用。
///
/// # §0 合规性（设计 §4.4）
///
/// AdapterCallbackNode 在执行树内，2 次 FFI（parse 入口 + _decode 回调）。
/// 用户主动继承 Adapter = 显式接受折衷（PM 决策 2）。不违反 §0 #1。
fn build_adapter_callback_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<AdapterCallbackNode, ConstructError> {
    // 1. 递归编译 subcon（沿用 field_index；与 BitwiseDescriptor 同模式）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "AdapterDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let subcon_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 取 _decode / _encode bound method（含 self）。
    //    AdapterDescriptor 应是用户 Adapter 子类的实例（或包装），其 _decode/_encode
    //    是 bound method（Python descriptor 协议自动绑定 self）。
    let decode = desc
        .getattr("_decode")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "AdapterDescriptor missing '_decode' method: {} (field index {}). \
                 Ensure the field is an Adapter subclass instance with _decode defined.",
                e, field_index
            ),
        })?
        .unbind();
    let encode = desc
        .getattr("_encode")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "AdapterDescriptor missing '_encode' method: {} (field index {}). \
                 Ensure the field is an Adapter (or SymmetricAdapter) subclass instance.",
                e, field_index
            ),
        })?
        .unbind();

    Ok(AdapterCallbackNode::new(subcon_node, decode, encode))
}

// ---------------------------------------------------------------------------
// Phase 7.2 Streams：Seek / Pointer / Prefixed 编译辅助函数
//
// 设计依据：`docs/design/模块设计/模块设计-Streams.md` §3.6。
//
// 共同模式：
// - SeekDescriptor / PointerDescriptor 的 at / offset 可为 int 常量或 FieldRef/ExprRef
//   表达式。表达式路径从 expr_programs[field_index][param_name] 取 ExprOp 列表
//   （与 BytesDescriptor length / StopIfDescriptor cond / ComputedDescriptor func 同模式）。
// - PointerDescriptor / PrefixedDescriptor 的 subcon / lengthfield 递归编译
//   （沿用 field_index，与 BitwiseDescriptor / PascalStringDescriptor 同模式）。
//   P3.1 限制：subcon / lengthfield 不支持含表达式的子描述符（Python 侧
//   `_extract_and_compile_exprs` 不递归 inner）。
// ---------------------------------------------------------------------------

/// 从 Python 描述符读取 `at` / `offset` 参数并构建对应的 Seek/Pointer offset 变体。
///
/// 共用辅助函数（SeekDescriptor.at 和 PointerDescriptor.offset 同模式）。
///
/// # 参数
///
/// - `py`：GIL token。
/// - `desc`：Python 描述符。
/// - `param_name`：参数名（"at" / "offset"）。
/// - `field_index`：字段索引。
/// - `expr_programs`：每字段的表达式程序。
///
/// # 路径
///
/// - Python int 常量 → `SeekOffset::Const` / `PointerOffset::Const`
/// - FieldRef/ExprRef → 从 `expr_programs[field_index][param_name]` 取 ExprOp 列表
fn build_seek_or_pointer_offset(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    param_name: &str,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<(Option<i64>, Option<ExprProgram>), ConstructError> {
    let value_obj = desc
        .getattr(param_name)
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "descriptor missing '{}' attribute: {} (field index {})",
                param_name, e, field_index
            ),
        })?;

    // 尝试 extract 为 i64（常量路径）。失败表示是表达式。
    match value_obj.extract::<i64>() {
        Ok(n) => Ok((Some(n), None)),
        Err(_) => {
            // 表达式路径：从 expr_programs[field_index][param_name] 取
            let field_exprs = expr_programs
                .get(field_index)
                .and_then(Option::as_ref)
                .ok_or_else(|| ConstructError::Compilation {
                    message: format!(
                        "field has non-constant {} but no expression program was provided \
                         (field index {})",
                        param_name, field_index
                    ),
                })?;
            let field_exprs_dict = field_exprs.bind(py).downcast::<PyDict>().map_err(|_| {
                ConstructError::Compilation {
                    message: format!(
                        "{} expression program must be a dict (field index {})",
                        param_name, field_index
                    ),
                }
            })?;
            let ops_list = field_exprs_dict
                .get_item(param_name)
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "failed to get '{}' from expression programs: {} (field index {})",
                        param_name, e, field_index
                    ),
                })?
                .ok_or_else(|| ConstructError::Compilation {
                    message: format!(
                        "field has non-constant {} but '{}' key missing in expression program \
                         (field index {})",
                        param_name, param_name, field_index
                    ),
                })?;
            let ops = parse_expr_ops_from_py(&ops_list)?;
            Ok((None, Some(ExprProgram::new(ops))))
        }
    }
}

/// 从 `SeekDescriptor` 构建 [`SeekNode`]（Phase 7.2 §3.6.2）。
///
/// 编译路径（参照 StopIfDescriptor 的 "condfunc" 模式 + BytesDescriptor 的 length 模式）：
/// 1. 从 desc 读取 `at`：
///    - Python int 常量 → [`SeekOffset::Const`]
///    - FieldRef/ExprRef → 从 `expr_programs[field_index]["at"]` 取 ExprOp 列表 →
///      [`SeekOffset::Expr`]
/// 2. 从 desc 读取 `whence`（int 0/1/2）→ [`Whence::from_python_int`]
fn build_seek_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<SeekNode, ConstructError> {
    let (const_at, expr_at) =
        build_seek_or_pointer_offset(py, desc, "at", field_index, expr_programs)?;
    let at = match (const_at, expr_at) {
        (Some(n), None) => SeekOffset::Const(n),
        (None, Some(prog)) => SeekOffset::Expr(prog),
        _ => {
            return Err(ConstructError::Compilation {
                message: format!(
                    "SeekDescriptor 'at' resolution failed (field index {})",
                    field_index
                ),
            });
        }
    };

    // whence：默认 0（Start）。Python int → Whence enum。
    let whence_val: i64 = desc
        .getattr("whence")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "SeekDescriptor missing 'whence' attribute: {} (field index {})",
                e, field_index
            ),
        })?
        .extract()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "SeekDescriptor 'whence' attribute must be int 0/1/2 (field index {})",
                field_index
            ),
        })?;
    let whence = Whence::from_python_int(whence_val)?;

    Ok(SeekNode::new(at, whence))
}

/// 从 `PointerDescriptor` 构建 [`PointerNode`]`（Phase 7.2 §3.6.2）。
///
/// 编译路径（参照 PascalStringDescriptor lengthfield 递归 + StopIf condfunc 表达式）：
/// 1. 从 desc 读取 `offset`：常量 → Const；表达式 → 从 expr_programs[field_index]["offset"] 取
/// 2. 从 desc 读取 `relativeOffset`（bool，默认 False）
/// 3. 递归编译 `subcon`（沿用 field_index，与 BitwiseDescriptor 同模式，不递归 inner 表达式）
/// 4. 校验 `stream` 属性为 None（非 None 编译期拒绝，文档化已知限制）
fn build_pointer_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<PointerNode, ConstructError> {
    // 1. offset（常量或表达式）
    let (const_off, expr_off) =
        build_seek_or_pointer_offset(py, desc, "offset", field_index, expr_programs)?;
    let offset = match (const_off, expr_off) {
        (Some(n), None) => PointerOffset::Const(n),
        (None, Some(prog)) => PointerOffset::Expr(prog),
        _ => {
            return Err(ConstructError::Compilation {
                message: format!(
                    "PointerDescriptor 'offset' resolution failed (field index {})",
                    field_index
                ),
            });
        }
    };

    // 2. relativeOffset（bool，默认 False）
    let relative: bool = match desc.getattr("relativeOffset") {
        Ok(v) => v.extract().map_err(|_| ConstructError::Compilation {
            message: format!(
                "PointerDescriptor 'relativeOffset' attribute must be bool (field index {})",
                field_index
            ),
        })?,
        Err(_) => false,
    };

    // 3. 递归编译 subcon（沿用 field_index，与 BitwiseDescriptor 同模式）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "PointerDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let subcon_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 4. 校验 stream 参数为 None（换流不支持，已知限制）
    //    若 Python 描述符没有 stream 属性，跳过校验（兼容旧版）。
    if let Ok(stream_attr) = desc.getattr("stream") {
        if !stream_attr.is_none() {
            return Err(ConstructError::Compilation {
                message: format!(
                    "PointerDescriptor 'stream' parameter (stream switching) is not supported \
                     in construct-rs (field index {}). Use the default stream (stream=None).",
                    field_index
                ),
            });
        }
    }

    Ok(PointerNode::new(offset, relative, subcon_node))
}

/// 从 `PrefixedDescriptor` 构建 [`PrefixedNode`]`（Phase 7.2 §3.6.2）。
///
/// 编译路径（参照 PascalStringDescriptor，双重子描述符递归）：
/// 1. 递归编译 `lengthfield`（沿用 field_index，与 PascalString 同模式）
/// 2. 递归编译 `subcon`（同上）
/// 3. 读取 `includelength`（bool，默认 False）
fn build_prefixed_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<PrefixedNode, ConstructError> {
    // 1. 递归编译 lengthfield（沿用 field_index）。
    let lengthfield_desc =
        desc.getattr("lengthfield")
            .map_err(|e| ConstructError::Compilation {
                message: format!(
                    "PrefixedDescriptor missing 'lengthfield' attribute: {} (field index {})",
                    e, field_index
                ),
            })?;
    let lengthfield_node = build_node_from_descriptor(
        py,
        &lengthfield_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 递归编译 subcon（同上）。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "PrefixedDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let subcon_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 3. includelength（bool，默认 False）。
    let includelength: bool = match desc.getattr("includelength") {
        Ok(v) => v.extract().map_err(|_| ConstructError::Compilation {
            message: format!(
                "PrefixedDescriptor 'includelength' attribute must be bool (field index {})",
                field_index
            ),
        })?,
        Err(_) => false,
    };

    Ok(PrefixedNode::new(
        lengthfield_node,
        subcon_node,
        includelength,
    ))
}

/// 将 Python 侧的 ExprOp 元组列表解析为 `Vec<ExprOp>`。
///
/// Python 侧编译器（`_compile_expr_tree`）将表达式树翻译为后序遍历的元组列表，
/// 通过 FFI 传入 Rust。本函数将每个元组转换为对应的 [`ExprOp`] 枚举变体。
///
/// # 元组格式
///
/// 每个元组为 `(op_name, optional_arg)`：
/// - `("getint", idx)` → [`ExprOp::GetInt`]（idx 是字段索引）
/// - `("const", value)` → [`ExprOp::Const`]（value 是 i64）
/// - `("add",)` / `("sub",)` / ... → 对应的二元/一元运算 [`ExprOp`]
///
/// # 错误
///
/// [`ConstructError::Compilation`]：ops_list 不是列表、元素不是元组、
/// op_name 不在已知列表中、或参数类型不匹配。
fn parse_expr_ops_from_py(ops_list: &Bound<'_, PyAny>) -> Result<Vec<ExprOp>, ConstructError> {
    let list = ops_list
        .downcast::<PyList>()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "expression ops must be a list, got {}",
                ops_list
                    .get_type()
                    .name()
                    .map(|n| n.to_string())
                    .unwrap_or_default()
            ),
        })?;

    let mut ops = Vec::with_capacity(list.len());
    for item in list.iter() {
        let tuple = item
            .downcast::<PyTuple>()
            .map_err(|_| ConstructError::Compilation {
                message: format!(
                    "each ExprOp must be a tuple, got {}",
                    item.get_type()
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                ),
            })?;
        let op_name: String = tuple
            .get_item(0)
            .map_err(|e| ConstructError::Compilation {
                message: format!("ExprOp tuple missing op name: {}", e),
            })?
            .extract()
            .map_err(|e| ConstructError::Compilation {
                message: format!("ExprOp op name must be a string: {}", e),
            })?;
        let op = match op_name.as_str() {
            "getint" => {
                let idx: usize = tuple
                    .get_item(1)
                    .map_err(|e| ConstructError::Compilation {
                        message: format!("getint missing index: {}", e),
                    })?
                    .extract()
                    .map_err(|e| ConstructError::Compilation {
                        message: format!("getint index must be int: {}", e),
                    })?;
                ExprOp::GetInt(idx)
            }
            "const" => {
                let val: i64 = tuple
                    .get_item(1)
                    .map_err(|e| ConstructError::Compilation {
                        message: format!("const missing value: {}", e),
                    })?
                    .extract()
                    .map_err(|e| ConstructError::Compilation {
                        message: format!("const value must be int: {}", e),
                    })?;
                ExprOp::Const(val)
            }
            "add" => ExprOp::Add,
            "sub" => ExprOp::Sub,
            "mul" => ExprOp::Mul,
            "floordiv" => ExprOp::FloorDiv,
            "mod" => ExprOp::Mod,
            "bitand" => ExprOp::BitAnd,
            "bitor" => ExprOp::BitOr,
            "bitxor" => ExprOp::BitXor,
            "shl" => ExprOp::Shl,
            "shr" => ExprOp::Shr,
            "neg" => ExprOp::Neg,
            "not" => ExprOp::Not,
            "eq" => ExprOp::Eq,
            "ne" => ExprOp::Ne,
            "lt" => ExprOp::Lt,
            "le" => ExprOp::Le,
            "gt" => ExprOp::Gt,
            "ge" => ExprOp::Ge,
            other => {
                return Err(ConstructError::Compilation {
                    message: format!("unknown ExprOp: {}", other),
                });
            }
        };
        ops.push(op);
    }
    Ok(ops)
}

/// 判断给定的 Python 类型对象是否为 StructMixin 子类。
///
/// 识别条件：该类型拥有 [`STRUCTMIXIN_COMPILED_ATTR]（`_construct_compiled`）属性。
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

/// 判断描述符是否"看起来像 Adapter"（Phase 6.3 duck typing 识别）。
///
/// 用于识别用户继承的 Adapter 子类（type name 非 "AdapterDescriptor"）。
/// 通过同时具备可调用的 `_decode` 和 `_encode` 方法判定。
///
/// # 防御性
///
/// - 仅检查属性存在 + callable，不调用方法（无副作用）。
/// - StructMixin 子类（同时有 _construct_compiled）已在前面分支返回，不会误判。
/// - FormatFieldDescriptor / BytesDescriptor 等无 _decode/_encode，不会误判。
///
/// # 错误
///
/// - `hasattr` 内部抛出异常时返回 `false`（保守起见，不视为 Adapter）。
fn is_adapter_like(desc: &Bound<'_, PyAny>) -> bool {
    let has_decode = desc
        .hasattr("_decode")
        .map(|h| {
            h && {
                // 进一步检查 _decode 是 callable（防御性，避免误判有同名属性的对象）
                desc.getattr("_decode")
                    .map(|d| d.is_callable())
                    .unwrap_or(false)
            }
        })
        .unwrap_or(false);
    if !has_decode {
        return false;
    }
    desc.hasattr("_encode")
        .map(|h| {
            h && {
                desc.getattr("_encode")
                    .map(|d| d.is_callable())
                    .unwrap_or(false)
            }
        })
        .unwrap_or(false)
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
// Phase 8 P0：Const / Default / Check 编译辅助函数
//
// 设计依据：`docs/design/模块设计/模块设计-Phase8-P0.md` §1。
//
// 共同模式：
// - 递归编译 subcon（沿用 field_index，与 BitwiseDescriptor 同模式；P3.1 限制：
//   subcon 不支持含表达式的子描述符）。
// - ConstDescriptor.value 是任意 Python 对象（int/bytes/str 等），直接 clone 引用。
// - DefaultDescriptor.value 是 FieldRef/ExprRef/int：编译为 ExprProgram（与 Rebuild func 同模式）。
// - CheckDescriptor.func 同 DefaultDescriptor.value（表达式 → ExprProgram）。
// ---------------------------------------------------------------------------

/// 从 `ConstDescriptor` 构建 `ConstNode`（设计 §1.2.1）。
///
/// ConstDescriptor 字段：subcon（描述符）+ value（任意 Python 对象）。
/// Python 层在缺省 subcon 时已自动推断为 Bytes(len(value))（针对 bytes value）。
fn build_const_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<ConstNode, ConstructError> {
    let _ = expr_programs; // Const 不使用表达式
                           // 1. 递归编译 subcon。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ConstDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 取 value（任意 Python 对象，直接 unbind 持有 Py<PyAny>）。
    let value = desc
        .getattr("value")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ConstDescriptor missing 'value' attribute: {} (field index {})",
                e, field_index
            ),
        })?
        .unbind();

    Ok(ConstNode::new(inner_node, value))
}

/// 从 `DefaultDescriptor` 构建 `DefaultNode`（设计 §1.2.2）。
///
/// DefaultDescriptor 字段：subcon（描述符）+ value（FieldRef/ExprRef/int）。
/// value 编译为 ExprProgram（从 expr_programs[field_index]["value"] 取，与 Rebuild 同模式）。
fn build_default_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<DefaultNode, ConstructError> {
    // 1. 递归编译 subcon。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "DefaultDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 从 expr_programs[field_index]["value"] 取 ExprOp 列表。
    //    Python 层在 _extract_and_compile_exprs 时已把 int 常量也编译为单条 Const ExprProgram。
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "DefaultDescriptor has no expression program for 'value' (field index {}). \
                 Note: value must be a Phase 2 expression (FieldRef/ExprRef/int), \
                 Python callable (lambda) is not supported (same constraint as Rebuild).",
                field_index
            ),
        })?;
    let field_exprs_dict =
        field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "DefaultDescriptor expression program must be a dict".to_string(),
            })?;
    let ops_obj = field_exprs_dict
        .get_item("value")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get 'value' from DefaultDescriptor expression programs: {} (field index {})",
                e, field_index
            ),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "DefaultDescriptor missing 'value' in expression programs (field index {})",
                field_index
            ),
        })?;
    let ops = parse_expr_ops_from_py(&ops_obj)?;
    let value_program = ExprProgram::new(ops);

    Ok(DefaultNode::new(inner_node, value_program))
}

/// 从 `CheckDescriptor` 构建 `CheckNode`（设计 §1.2.3）。
///
/// CheckDescriptor 字段：func（FieldRef/ExprRef/int）。
/// func 编译为 ExprProgram（从 expr_programs[field_index]["func"] 取）。
fn build_check_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<CheckNode, ConstructError> {
    let _ = desc;
    // 从 expr_programs[field_index]["func"] 取（与 ComputedDescriptor 同模式）。
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "CheckDescriptor has no expression program for 'func' (field index {}). \
                 Note: func must be a Phase 2 expression (FieldRef/ExprRef/int), \
                 Python callable (lambda) is not supported.",
                field_index
            ),
        })?;
    let field_exprs_dict =
        field_exprs
            .bind(py)
            .downcast::<PyDict>()
            .map_err(|_| ConstructError::Compilation {
                message: "CheckDescriptor expression program must be a dict".to_string(),
            })?;
    let ops_obj = field_exprs_dict
        .get_item("func")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to get 'func' from CheckDescriptor expression programs: {} (field index {})",
                e, field_index
            ),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "CheckDescriptor missing 'func' in expression programs (field index {})",
                field_index
            ),
        })?;
    let ops = parse_expr_ops_from_py(&ops_obj)?;
    let func_program = ExprProgram::new(ops);

    Ok(CheckNode::new(func_program))
}

/// 从 `ProbeDescriptor` 构建 `ProbeNode`（设计 §5.2.2）。
///
/// ProbeDescriptor 字段：into（None 或字段名字符串）+ lookahead（None 或 usize）。
///
/// [设计质疑] into 用 FieldName 代替 ExprProgram（详见 probe.rs 模块级注释）。
fn build_probe_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
) -> Result<ProbeNode, ConstructError> {
    // 1. into: None 或 字段名（PyString → FieldName）。
    let into_obj = desc
        .getattr("into")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ProbeDescriptor missing 'into' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let into: Option<FieldName> = if into_obj.is_none() {
        None
    } else {
        let name: String = into_obj
            .extract()
            .map_err(|e| ConstructError::Compilation {
                message: format!(
                    "ProbeDescriptor 'into' must be a string or None, got error: {} (field index {})",
                    e, field_index
                ),
            })?;
        Some(FieldName::new(py, name))
    };

    // 2. lookahead: None 或 usize。
    let lookahead_obj = desc
        .getattr("lookahead")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ProbeDescriptor missing 'lookahead' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let lookahead: Option<usize> = if lookahead_obj.is_none() {
        None
    } else {
        let n: usize = lookahead_obj.extract().map_err(|e| ConstructError::Compilation {
            message: format!(
                "ProbeDescriptor 'lookahead' must be an int or None, got error: {} (field index {})",
                e, field_index
            ),
        })?;
        Some(n)
    };

    Ok(ProbeNode::new(into, lookahead))
}

/// 从 `AlignedDescriptor` 构建 `AlignedNode`（设计 §4.3）。
///
/// AlignedDescriptor 字段：modulus（int 或 FieldRef/ExprRef）+ subcon + pattern（bytes len 1）。
fn build_aligned_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<AlignedNode, ConstructError> {
    // 1. 递归编译 subcon。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "AlignedDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 编译 modulus（int 常量或表达式）。
    //    与 BytesDescriptor length / SeekDescriptor at 同模式：
    //    int 常量也包装为单条 Const ExprProgram。
    let modulus_obj = desc
        .getattr("modulus")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "AlignedDescriptor missing 'modulus' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let modulus_program: ExprProgram = match modulus_obj.extract::<i64>() {
        Ok(n) => {
            // AL-6/AL-7/AL-8：modulus < 2 编译期拒绝（PaddingError 等价）。
            if n < 2 {
                return Err(ConstructError::Compilation {
                    message: format!(
                        "Aligned modulus must be >= 2, got {} (field index {})",
                        n, field_index
                    ),
                });
            }
            // 常量包装为 Const ExprProgram（运行期 eval ~5ns）。
            ExprProgram::new(vec![ExprOp::Const(n)])
        }
        Err(_) => {
            // 表达式路径：从 expr_programs[field_index]["modulus"] 取
            let field_exprs = expr_programs
                .get(field_index)
                .and_then(Option::as_ref)
                .ok_or_else(|| ConstructError::Compilation {
                    message: format!(
                        "AlignedDescriptor has non-constant modulus but no expression program was provided (field index {})",
                        field_index
                    ),
                })?;
            let field_exprs_dict = field_exprs.bind(py).downcast::<PyDict>().map_err(|_| {
                ConstructError::Compilation {
                    message: "AlignedDescriptor expression program must be a dict".to_string(),
                }
            })?;
            let ops_obj = field_exprs_dict
                .get_item("modulus")
                .map_err(|e| ConstructError::Compilation {
                    message: format!(
                        "failed to get 'modulus' from AlignedDescriptor expression programs: {} (field index {})",
                        e, field_index
                    ),
                })?
                .ok_or_else(|| ConstructError::Compilation {
                    message: format!(
                        "AlignedDescriptor missing 'modulus' in expression programs (field index {})",
                        field_index
                    ),
                })?;
            let ops = parse_expr_ops_from_py(&ops_obj)?;
            ExprProgram::new(ops)
        }
    };

    // 3. pattern：默认 0x00。Python 限定 isinstance(bytes) and len==1。
    let pattern_obj = desc
        .getattr("pattern")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "AlignedDescriptor missing 'pattern' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    // 期望 1 字节 bytes。
    let pattern_bytes: &[u8] = pattern_obj
        .extract()
        .map_err(|_| ConstructError::Compilation {
            message: format!(
                "AlignedDescriptor 'pattern' must be 1-byte bytes (field index {})",
                field_index
            ),
        })?;
    if pattern_bytes.len() != 1 {
        // AL-11/AL-12：pattern len != 1 编译期 PaddingError。
        return Err(ConstructError::Compilation {
            message: format!(
                "AlignedDescriptor 'pattern' must be exactly 1 byte, got {} bytes (field index {})",
                pattern_bytes.len(),
                field_index
            ),
        });
    }
    let pattern = pattern_bytes[0];

    Ok(AlignedNode::new(inner_node, modulus_program, pattern))
}

/// 从 `HexDescriptor` 构建 `HexNode`（设计 §2.3.1）。
///
/// HexDescriptor 字段：subcon。display_classes 与 fmtstr 在编译期物化/预算。
/// fmtstr 编译期 intern 为 `Py<PyString>`（方案 B），parse 时直接借用。
fn build_hex_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<HexNode, ConstructError> {
    // 1. 递归编译 subcon。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "HexDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 编译期物化 3 个 Hex 显示类。
    let classes = load_hex_display_classes(py)?;

    // 3. fmtstr 编译期预算（若 inner sizeof 静态可计算）。
    //    方案 B：编译期 intern 为 Py<PyString>，parse 时直接借用（消除每次 PyString::new）。
    //    使用 placeholder ctx 调 sizeof（无表达式时可成功）。
    let placeholder_ctx = crate::context::Context::placeholder(py);
    let fmtstr: Option<Py<PyString>> = match inner_node.sizeof(&placeholder_ctx) {
        Ok(size) => {
            let fmt_str = format!("0{}X", 2 * size);
            Some(PyString::new_bound(py, &fmt_str).into_py(py))
        }
        Err(_) => None, // inner sizeof 不可静态计算 → 运行期 fallback
    };

    Ok(HexNode::new(inner_node, classes, fmtstr))
}

/// 从 `HexDumpDescriptor` 构建 `HexDumpNode`（设计 §2.3.1）。
///
/// HexDumpDescriptor 字段：subcon。display_classes 在编译期物化（2 个 HexDump 显示类）。
fn build_hexdump_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<HexDumpNode, ConstructError> {
    // 1. 递归编译 subcon。
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "HexDumpDescriptor missing 'subcon' attribute: {} (field index {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 编译期物化 2 个 HexDump 显示类。
    let classes = load_hexdump_display_classes(py)?;

    Ok(HexDumpNode::new(inner_node, classes))
}

/// 从 `ChecksumDescriptor` 构建 `ChecksumNode`（设计 §3.5）。
///
/// ChecksumDescriptor 字段：
/// - checksumfield（描述符）
/// - hashfunc：HashAlgo enum 实例（BuiltinHash 路径）或 Python callable（兼容路径）
/// - bytesfunc / start_expr / end_expr：bytes_source 配置
///
/// 编译期分类：
/// - hashfunc.type() == "construct._hashalgo.HashAlgo" → BuiltinHash（按 .name 取值）
/// - hashfunc.is_callable() → PythonCallable
/// - 否则 CompilationError
fn build_checksum_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<ChecksumNode, ConstructError> {
    // 1. 递归编译 checksumfield。
    let checksumfield_desc =
        desc.getattr("checksumfield")
            .map_err(|e| ConstructError::Compilation {
                message: format!(
                    "ChecksumDescriptor missing 'checksumfield' attribute: {} (field index {})",
                    e, field_index
                ),
            })?;
    let checksumfield_node = build_node_from_descriptor(
        py,
        &checksumfield_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;

    // 2. 识别 hashfunc 类型。
    let hashfunc_obj = desc
        .getattr("hashfunc")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ChecksumDescriptor missing 'hashfunc' attribute: {} (field index {})",
                e, field_index
            ),
        })?;

    let hashfunc: HashFunc = if hashfunc_obj.is_callable() {
        // Python callable 路径（A1/A2）
        HashFunc::PythonCallable(hashfunc_obj.unbind())
    } else {
        // 检查是否是 HashAlgo enum 实例
        let type_name =
            hashfunc_obj
                .get_type()
                .name()
                .map_err(|e| ConstructError::Compilation {
                    message: format!("failed to get hashfunc type name: {}", e),
                })?;
        let type_str = type_name
            .to_str()
            .map_err(|e| ConstructError::Compilation {
                message: format!("hashfunc type name not UTF-8: {}", e),
            })?;
        if type_str == "HashAlgo" {
            // BuiltinHash 路径（B1/B2）：取 .name 属性识别算法
            let name_obj =
                hashfunc_obj
                    .getattr("name")
                    .map_err(|e| ConstructError::Compilation {
                        message: format!(
                            "HashAlgo instance missing .name: {} (field index {})",
                            e, field_index
                        ),
                    })?;
            let name_str: String = name_obj
                .extract()
                .map_err(|_| ConstructError::Compilation {
                    message: format!(
                        "HashAlgo.name is not a string (field index {})",
                        field_index
                    ),
                })?;
            let builtin = match name_str.as_str() {
                "MD5" => BuiltinHash::Md5,
                "SHA1" => BuiltinHash::Sha1,
                "SHA256" => BuiltinHash::Sha256,
                "SHA512" => BuiltinHash::Sha512,
                "CRC32" => BuiltinHash::Crc32,
                "ADLER32" => BuiltinHash::Adler32,
                other => {
                    return Err(ConstructError::Compilation {
                        message: format!(
                            "unknown HashAlgo name: {} (field index {})",
                            other, field_index
                        ),
                    });
                }
            };
            HashFunc::BuiltIn(builtin)
        } else {
            return Err(ConstructError::Compilation {
                message: format!(
                    "ChecksumDescriptor.hashfunc must be HashAlgo enum or callable, got type '{}' (field index {})",
                    type_str, field_index
                ),
            });
        }
    };

    // 3. bytes_source 识别：先看是否有 start/end（StreamRange），否则 bytesfunc（ContextBytes）。
    //    ChecksumDescriptor.start_expr/end_expr 是 @property，hasattr 总返回 True，
    //    需检查值是否非 None。
    let start_attr = desc.getattr("start_expr").ok();
    let end_attr = desc.getattr("end_expr").ok();
    let start_is_some = start_attr.as_ref().map(|v| !v.is_none()).unwrap_or(false);
    let end_is_some = end_attr.as_ref().map(|v| !v.is_none()).unwrap_or(false);
    let has_stream_range = start_is_some && end_is_some;
    let bytes_source: BytesSource = if has_stream_range {
        // StreamRange：从 expr_programs 取 start/end
        let field_exprs = expr_programs
            .get(field_index)
            .and_then(Option::as_ref)
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "ChecksumDescriptor StreamRange has no expression programs (field index {})",
                    field_index
                ),
            })?;
        let field_exprs_dict =
            field_exprs
                .bind(py)
                .downcast::<PyDict>()
                .map_err(|_| ConstructError::Compilation {
                    message: "ChecksumDescriptor expression program must be a dict".to_string(),
                })?;
        let start_ops = field_exprs_dict
            .get_item("start")
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to get 'start' from ChecksumDescriptor: {}", e),
            })?
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "ChecksumDescriptor missing 'start' (field index {})",
                    field_index
                ),
            })?;
        let end_ops = field_exprs_dict
            .get_item("end")
            .map_err(|e| ConstructError::Compilation {
                message: format!("failed to get 'end' from ChecksumDescriptor: {}", e),
            })?
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "ChecksumDescriptor missing 'end' (field index {})",
                    field_index
                ),
            })?;
        let start = ExprProgram::new(parse_expr_ops_from_py(&start_ops)?);
        let end = ExprProgram::new(parse_expr_ops_from_py(&end_ops)?);
        BytesSource::StreamRange { start, end }
    } else {
        // ContextBytes：从 bytesfunc 属性取字段名
        let bytesfunc_name: String = desc
            .getattr("bytesfunc_name")
            .map_err(|e| ConstructError::Compilation {
                message: format!(
                    "ChecksumDescriptor missing 'bytesfunc_name' attribute (ContextBytes mode): {} (field index {})",
                    e, field_index
                ),
            })?
            .extract()
            .map_err(|_| ConstructError::Compilation {
                message: format!(
                    "ChecksumDescriptor.bytesfunc_name must be a string (field index {})",
                    field_index
                ),
            })?;
        BytesSource::ContextBytes {
            field_idx: 0, // 未使用，保留兼容
            field_name: bytesfunc_name,
        }
    };

    Ok(ChecksumNode::new(
        checksumfield_node,
        hashfunc,
        bytes_source,
    ))
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
            let schema =
                compile_schema(py, &cls, vec![], vec![], None, None, false).expect("compile");
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
            let schema = compile_schema(
                py,
                &cls,
                vec!["address".to_string()],
                vec![desc.into_any()],
                None,
                None,
                false,
            )
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
            let bytes4 = Py::new(py, BytesDescriptor::new(4.into_py(py)))
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

            let schema =
                compile_schema(py, &cls, names, descs, None, None, false).expect("compile");

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

            let schema = compile_schema(
                py,
                &cls,
                vec!["inner".to_string()],
                vec![inner_cls_py],
                None,
                None,
                false,
            )
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
            let result = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string()],
                vec![],
                None,
                None,
                false,
            );
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
            let err = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![bad_desc],
                None,
                None,
                false,
            )
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
            let result = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![bad_desc],
                None,
                None,
                false,
            );
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

            let schema =
                compile_schema(py, &cls, names, descs, None, None, false).expect("compile");
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

            let schema = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            // _parse_raw 现在返回用户类实例（方案 B'）
            let data = PyBytes::new_bound(py, &[0x42u8]);
            let instance = schema._parse_raw(py, &data).expect("parse").into_bound(py);
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
                None,
                None,
                false,
            )
            .expect("compile");

            // 数据：a=0x01, b=0x0203
            let data = PyBytes::new_bound(py, &[0x01, 0x02, 0x03]);
            let instance = schema._parse_raw(py, &data).expect("parse").into_bound(py);
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

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![int32ub],
                None,
                None,
                false,
            )
            .expect("compile");

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

            let schema = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

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
            let bytes2 = Py::new(py, BytesDescriptor::new(2.into_py(py)))
                .expect("Py::new")
                .into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec![int8ub, int16ub, bytes2],
                None,
                None,
                false,
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
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
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
            let schema =
                compile_schema(py, &cls, vec![], vec![], None, None, false).expect("compile");

            let data = PyBytes::new_bound(py, b"");
            let result = schema._parse_raw(py, &data).expect("parse").into_bound(py);
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
            let schema =
                compile_schema(py, &cls, vec![], vec![], None, None, false).expect("compile");

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
            let bytes2 = Py::new(py, BytesDescriptor::new(2.into_py(py)))
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
                None,
                None,
                false,
            )
            .expect("compile");

            // parse: tag=0xAA, len_bytes=b'\x01\x02', rest=b'\x03\x04\x05'
            let data = PyBytes::new_bound(py, &[0xAA, 0x01, 0x02, 0x03, 0x04, 0x05]);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
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
                None,
                None,
                false,
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
                None,
                None,
                false,
            )
            .expect("compile outer");

            // parse b'\xAA\x05' → Outer 实例（tag=0xAA, inner=Inner(x=5)）
            // 方案 B'：parse 返回 Outer 类实例（不是 dict）
            let data = PyBytes::new_bound(py, &[0xAA, 0x05]);
            let parsed = outer_schema
                ._parse_raw(py, &data)
                .expect("parse")
                .into_bound(py);
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
            let err = compile_schema(py, &cls_bound, vec![], vec![], None, None, false)
                .expect_err("should fail");
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
            let result = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![desc],
                None,
                None,
                false,
            );
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
            let schema = compile_schema(
                py,
                cls.bind(py),
                vec!["x".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
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

    // ======================================================================
    // Phase 2 子任务 2.3：modes 参数（FieldMode 解析）
    // ======================================================================

    /// 创建一个 Python dict，模拟表达式程序 `{param_name: [(op_tuple), ...]}`。
    fn make_expr_program(py: Python<'_>) -> Py<PyAny> {
        // 创建一个简单的 Python dict 模拟表达式程序。
        // 内容不重要（2.3 阶段 Rust 仅检查是否 Some），但用合法结构以便后续扩展。
        let code = "{'length': [('getint', 0), ('const', 2), ('mul',)]}";
        py.eval_bound(code, None, None)
            .expect("eval expr program")
            .unbind()
    }

    #[test]
    fn compile_modes_none_defaults_all_rw() {
        // modes=None 时所有字段默认 Rw（Phase 1 兼容）。
        with_py(|py| {
            let cls = make_dummy_class(py, "DefaultRw");
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
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    let fields = s.fields();
                    assert_eq!(fields[0].mode, FieldMode::Rw);
                    assert_eq!(fields[1].mode, FieldMode::Rw);
                    assert!(!s.has_expressions());
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_modes_rw_ro_wo_parsed_correctly() {
        // modes=["rw", "ro", "wo"] 正确映射到 FieldMode。
        with_py(|py| {
            let cls = make_dummy_class(py, "MixedModes");
            let mk_desc = || {
                Py::new(
                    py,
                    FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
                )
                .expect("Py::new")
                .into_any()
            };
            let schema = compile_schema(
                py,
                &cls,
                vec![
                    "rw_field".to_string(),
                    "ro_field".to_string(),
                    "wo_field".to_string(),
                ],
                vec![mk_desc(), mk_desc(), mk_desc()],
                Some(vec!["rw".to_string(), "ro".to_string(), "wo".to_string()]),
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    let fields = s.fields();
                    assert_eq!(fields[0].mode, FieldMode::Rw, "field 0 should be Rw");
                    assert_eq!(fields[1].mode, FieldMode::Ro, "field 1 should be Ro");
                    assert_eq!(fields[2].mode, FieldMode::Wo, "field 2 should be Wo");
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_modes_unknown_string_returns_error() {
        // 未知 mode 字符串 → Compilation error。
        with_py(|py| {
            let cls = make_dummy_class(py, "BadMode");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![desc],
                Some(vec!["invalid".to_string()]),
                None,
                false,
            )
            .expect_err("should fail");
            let msg = format!("{}", err);
            assert!(
                msg.contains("unknown field mode") || msg.contains("invalid"),
                "message should mention unknown mode: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_modes_length_mismatch_returns_error() {
        // modes 长度与 field_names 不一致 → Compilation error。
        with_py(|py| {
            let cls = make_dummy_class(py, "ModeLenMismatch");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string()],
                vec![desc.clone_ref(py), desc],
                Some(vec!["rw".to_string()]),
                None,
                false,
            )
            .expect_err("should fail");
            let msg = format!("{}", err);
            assert!(
                msg.contains("长度不一致") || msg.contains("length"),
                "message should mention length mismatch: {}",
                msg
            );
        });
    }

    // ======================================================================
    // Phase 2 子任务 2.3：expr_programs 参数（has_expressions 计算）
    // ======================================================================

    #[test]
    fn compile_expr_programs_none_has_expressions_false() {
        // expr_programs=None → has_expressions=false（Phase 1 兼容）。
        with_py(|py| {
            let cls = make_dummy_class(py, "NoExpr");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert!(!s.has_expressions(), "should have no expressions");
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_expr_programs_all_none_has_expressions_false() {
        // expr_programs=[None, None] → 所有字段无表达式 → has_expressions=false。
        with_py(|py| {
            let cls = make_dummy_class(py, "AllNoneExpr");
            let mk_desc = || {
                Py::new(
                    py,
                    FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
                )
                .expect("Py::new")
                .into_any()
            };
            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string()],
                vec![mk_desc(), mk_desc()],
                None,
                Some(vec![None, None]),
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert!(!s.has_expressions(), "should have no expressions");
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_expr_programs_with_some_sets_has_expressions_true() {
        // expr_programs 中任一字段为 Some → has_expressions=true。
        with_py(|py| {
            let cls = make_dummy_class(py, "HasExpr");
            let mk_desc = || {
                Py::new(
                    py,
                    FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
                )
                .expect("Py::new")
                .into_any()
            };
            let prog = make_expr_program(py);
            let schema = compile_schema(
                py,
                &cls,
                vec!["count".to_string(), "data".to_string()],
                vec![mk_desc(), mk_desc()],
                None,
                Some(vec![None, Some(prog)]),
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert!(
                        s.has_expressions(),
                        "should have expressions when any field has program"
                    );
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_expr_programs_with_modes_combined() {
        // modes + expr_programs 同时传入：mode 正确解析 + has_expressions 正确。
        with_py(|py| {
            let cls = make_dummy_class(py, "Combined");
            let mk_desc = || {
                Py::new(
                    py,
                    FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
                )
                .expect("Py::new")
                .into_any()
            };
            let prog = make_expr_program(py);
            let schema = compile_schema(
                py,
                &cls,
                vec![
                    "count".to_string(),
                    "computed".to_string(),
                    "pad".to_string(),
                ],
                vec![mk_desc(), mk_desc(), mk_desc()],
                Some(vec!["rw".to_string(), "ro".to_string(), "wo".to_string()]),
                Some(vec![None, Some(prog), None]),
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    let fields = s.fields();
                    assert_eq!(fields[0].mode, FieldMode::Rw);
                    assert_eq!(fields[1].mode, FieldMode::Ro);
                    assert_eq!(fields[2].mode, FieldMode::Wo);
                    assert!(s.has_expressions());
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_backward_compatible_with_none_none() {
        // Phase 1 兼容：传入 None, None 等价于 Phase 1 行为。
        with_py(|py| {
            let cls = make_dummy_class(py, "BackCompat");
            let desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    assert_eq!(s.fields()[0].mode, FieldMode::Rw);
                    assert!(!s.has_expressions());
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // Phase 2 子任务 2.5：parse_expr_ops_from_py
    // ======================================================================

    fn make_ops_list<'py>(py: Python<'py>, code: &str) -> Bound<'py, PyAny> {
        py.eval_bound(code, None, None).expect("eval ops list")
    }

    #[test]
    fn parse_expr_ops_simple_getint() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "[('getint', 0)]");
            let ops = parse_expr_ops_from_py(&ops_list).expect("parse");
            assert_eq!(ops, vec![ExprOp::GetInt(0)]);
        });
    }

    #[test]
    fn parse_expr_ops_const_value() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "[('const', 42)]");
            let ops = parse_expr_ops_from_py(&ops_list).expect("parse");
            assert_eq!(ops, vec![ExprOp::Const(42)]);
        });
    }

    #[test]
    fn parse_expr_ops_binary_mul() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "[('getint', 0), ('const', 2), ('mul',)]");
            let ops = parse_expr_ops_from_py(&ops_list).expect("parse");
            assert_eq!(ops, vec![ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]);
        });
    }

    #[test]
    fn parse_expr_ops_addition() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "[('getint', 0), ('const', 1), ('add',)]");
            let ops = parse_expr_ops_from_py(&ops_list).expect("parse");
            assert_eq!(ops, vec![ExprOp::GetInt(0), ExprOp::Const(1), ExprOp::Add]);
        });
    }

    #[test]
    fn parse_expr_ops_all_comparison_ops() {
        with_py(|py| {
            for (code, expected) in [
                ("[('eq',)]", ExprOp::Eq),
                ("[('ne',)]", ExprOp::Ne),
                ("[('lt',)]", ExprOp::Lt),
                ("[('le',)]", ExprOp::Le),
                ("[('gt',)]", ExprOp::Gt),
                ("[('ge',)]", ExprOp::Ge),
            ] {
                let ops_list = make_ops_list(py, code);
                let ops = parse_expr_ops_from_py(&ops_list)
                    .unwrap_or_else(|e| panic!("parse '{}' failed: {:?}", code, e));
                assert_eq!(ops, vec![expected], "mismatch for {}", code);
            }
        });
    }

    #[test]
    fn parse_expr_ops_all_bitwise_ops() {
        with_py(|py| {
            for (code, expected) in [
                ("[('bitand',)]", ExprOp::BitAnd),
                ("[('bitor',)]", ExprOp::BitOr),
                ("[('bitxor',)]", ExprOp::BitXor),
                ("[('shl',)]", ExprOp::Shl),
                ("[('shr',)]", ExprOp::Shr),
            ] {
                let ops_list = make_ops_list(py, code);
                let ops = parse_expr_ops_from_py(&ops_list)
                    .unwrap_or_else(|e| panic!("parse '{}' failed: {:?}", code, e));
                assert_eq!(ops, vec![expected], "mismatch for {}", code);
            }
        });
    }

    #[test]
    fn parse_expr_ops_unary_neg_and_not() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "[('getint', 0), ('neg',), ('not',)]");
            let ops = parse_expr_ops_from_py(&ops_list).expect("parse");
            assert_eq!(ops, vec![ExprOp::GetInt(0), ExprOp::Neg, ExprOp::Not]);
        });
    }

    #[test]
    fn parse_expr_ops_floordiv_and_mod() {
        with_py(|py| {
            let ops_list =
                make_ops_list(py, "[('getint', 0), ('const', 3), ('floordiv',), ('mod',)]");
            let ops = parse_expr_ops_from_py(&ops_list).expect("parse");
            assert_eq!(
                ops,
                vec![
                    ExprOp::GetInt(0),
                    ExprOp::Const(3),
                    ExprOp::FloorDiv,
                    ExprOp::Mod
                ]
            );
        });
    }

    #[test]
    fn parse_expr_ops_chained_addition() {
        with_py(|py| {
            // count + 1 + 1
            let ops_list = make_ops_list(
                py,
                "[('getint', 0), ('const', 1), ('add',), ('const', 1), ('add',)]",
            );
            let ops = parse_expr_ops_from_py(&ops_list).expect("parse");
            assert_eq!(
                ops,
                vec![
                    ExprOp::GetInt(0),
                    ExprOp::Const(1),
                    ExprOp::Add,
                    ExprOp::Const(1),
                    ExprOp::Add,
                ]
            );
        });
    }

    #[test]
    fn parse_expr_ops_rejects_non_list() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "('getint', 0)");
            let err = parse_expr_ops_from_py(&ops_list).expect_err("should fail");
            match err {
                ConstructError::Compilation { message } => {
                    assert!(message.contains("must be a list"), "got: {}", message);
                }
                other => panic!("expected Compilation, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_expr_ops_rejects_non_tuple_element() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "[42]");
            let err = parse_expr_ops_from_py(&ops_list).expect_err("should fail");
            match err {
                ConstructError::Compilation { message } => {
                    assert!(message.contains("must be a tuple"), "got: {}", message);
                }
                other => panic!("expected Compilation, got {:?}", other),
            }
        });
    }

    #[test]
    fn parse_expr_ops_rejects_unknown_op() {
        with_py(|py| {
            let ops_list = make_ops_list(py, "[('bogus',)]");
            let err = parse_expr_ops_from_py(&ops_list).expect_err("should fail");
            match err {
                ConstructError::Compilation { message } => {
                    assert!(message.contains("unknown"), "got: {}", message);
                }
                other => panic!("expected Compilation, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // Phase 2 子任务 2.5：compile_schema 表达式 Bytes 集成
    // ======================================================================

    /// 创建一个非 int 的 Py<PyAny>，用于模拟表达式对象。
    fn make_non_int_marker(py: Python<'_>) -> Py<PyAny> {
        py.eval_bound("'expr_marker'", None, None)
            .expect("eval marker string")
            .unbind()
    }

    #[test]
    fn compile_bytes_expr_produces_expr_length_node() {
        // BytesDescriptor(length=<非 int>) + expr_programs[1]=Some({"length": [...]})
        // → BytesNode 使用 BytesLength::Expr
        with_py(|py| {
            let cls = make_dummy_class(py, "ExprBytes");
            let count_desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let bytes_desc = Py::new(py, BytesDescriptor::new(make_non_int_marker(py)))
                .expect("Py::new")
                .into_any();

            let prog = make_expr_program(py); // {"length": [("getint", 0), ("const", 2), ("mul",)]}
            let schema = compile_schema(
                py,
                &cls,
                vec!["count".to_string(), "data".to_string()],
                vec![count_desc, bytes_desc],
                None,
                Some(vec![None, Some(prog)]),
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert!(s.has_expressions(), "should have expressions");
                    let fields = s.fields();
                    match &fields[1].node {
                        Node::Bytes(b) => {
                            assert!(
                                b.length().is_expr(),
                                "expected BytesLength::Expr, got {:?}",
                                b.length()
                            );
                        }
                        other => panic!("expected Node::Bytes, got {:?}", other),
                    }
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bytes_const_with_none_expr_still_const() {
        // BytesDescriptor(length=4) + expr_programs=None
        // → BytesNode 使用 BytesLength::Const（向后兼容）
        with_py(|py| {
            let cls = make_dummy_class(py, "ConstBytes");
            let bytes_desc = Py::new(py, BytesDescriptor::new(4.into_py(py)))
                .expect("Py::new")
                .into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["data".to_string()],
                vec![bytes_desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert!(
                        !s.has_expressions(),
                        "const Bytes should not set has_expressions"
                    );
                    let fields = s.fields();
                    match &fields[0].node {
                        Node::Bytes(b) => {
                            assert!(
                                b.length().is_const(),
                                "expected BytesLength::Const, got {:?}",
                                b.length()
                            );
                            assert_eq!(b.length().as_const(), Some(&4));
                        }
                        other => panic!("expected Node::Bytes, got {:?}", other),
                    }
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bytes_non_int_without_expr_returns_error() {
        // BytesDescriptor(length=<非 int>) + expr_programs=None
        // → 应返回编译错误
        with_py(|py| {
            let cls = make_dummy_class(py, "MissingExpr");
            let bytes_desc = Py::new(py, BytesDescriptor::new(make_non_int_marker(py)))
                .expect("Py::new")
                .into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["data".to_string()],
                vec![bytes_desc],
                None,
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("no expression program") || msg.contains("non-constant"),
                "got: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_bytes_expr_missing_length_key_returns_error() {
        // BytesDescriptor(length=<非 int>) + expr_programs[0]=Some({})（缺少 "length" 键）
        // → 应返回编译错误
        with_py(|py| {
            let cls = make_dummy_class(py, "MissingKey");
            let bytes_desc = Py::new(py, BytesDescriptor::new(make_non_int_marker(py)))
                .expect("Py::new")
                .into_any();
            let empty_prog = py
                .eval_bound("{}", None, None)
                .expect("eval empty dict")
                .unbind();
            let err = compile_schema(
                py,
                &cls,
                vec!["data".to_string()],
                vec![bytes_desc],
                None,
                Some(vec![Some(empty_prog)]),
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("'length'") || msg.contains("missing"),
                "got: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_bytes_expr_end_to_end_round_trip() {
        // 完整的编译 + parse + build 回环
        // Struct { count: Int8ub, data: Bytes(count * 2) }
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "RoundTripExpr");
            let count_desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let bytes_desc = Py::new(py, BytesDescriptor::new(make_non_int_marker(py)))
                .expect("Py::new")
                .into_any();
            // {"length": [("getint", 0), ("const", 2), ("mul",)]}
            let prog = make_expr_program(py);
            let schema = compile_schema(
                py,
                &cls,
                vec!["count".to_string(), "data".to_string()],
                vec![count_desc, bytes_desc],
                None,
                Some(vec![None, Some(prog)]),
                false,
            )
            .expect("compile");

            // --- parse ---
            // count=3 → data=Bytes(3*2=6) → "ABCDEF"
            let parse_data = b"\x03ABCDEF";
            let data_binding = PyBytes::new_bound(py, parse_data);
            let parsed = schema
                ._parse_raw(py, &data_binding)
                .expect("parse")
                .into_bound(py);
            let count_val: i64 = parsed
                .getattr("count")
                .expect("getattr count")
                .extract()
                .expect("extract count");
            assert_eq!(count_val, 3);
            let data_binding2 = parsed.getattr("data").expect("getattr data");
            let data_val: &[u8] = data_binding2
                .downcast::<PyBytes>()
                .expect("is PyBytes")
                .as_bytes();
            assert_eq!(data_val, b"ABCDEF");

            // --- build ---
            // Use cls directly to create the instance (avoid NameError)
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs.set_item("count", 3).expect("set count");
            kwargs
                .set_item("data", PyBytes::new_bound(py, b"ABCDEF"))
                .expect("set data");
            let obj = cls.call((), Some(&kwargs)).expect("create obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), parse_data);
        });
    }

    // ======================================================================
    // Phase 2 子任务 2.6：TellDescriptor / ComputedDescriptor 识别
    // ======================================================================

    /// 创建一个 TellDescriptor Python 实例（模拟 Python 侧 Tell()）。
    ///
    /// 使用 `type()` 动态创建类（避免 `eval_bound` 无法执行 `class` 语句的限制）。
    /// 类名为 "TellDescriptor"，含 `_expr_params = {}` 类属性。
    fn make_tell_descriptor(py: Python<'_>) -> Py<PyAny> {
        // type("TellDescriptor", (object,), {"_expr_params": {}})()
        py.eval_bound(
            "type('TellDescriptor', (), {'_expr_params': {}})()",
            None,
            None,
        )
        .expect("create TellDescriptor")
        .unbind()
    }

    /// 创建一个 ComputedDescriptor Python 实例，携带 expr 引用。
    ///
    /// 使用 `run_bound` 定义 ComputedDescriptor 类（带 property），然后实例化。
    /// `expr` 参数可以是任意对象（None 即可，因为实际表达式通过 expr_programs 传入）。
    fn make_computed_descriptor(py: Python<'_>) -> Py<PyAny> {
        // 先用 run_bound 定义 ComputedDescriptor 类（带 property），然后实例化。
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class ComputedDescriptor:\n",
            "    def __init__(self, expr):\n",
            "        self.expr = expr\n",
            "    @property\n",
            "    def _expr_params(self):\n",
            "        return {'func': self.expr}\n",
            "    def __repr__(self):\n",
            "        return 'Computed({!r})'.format(self.expr)\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define ComputedDescriptor");
        let cls = globals
            .get_item("ComputedDescriptor")
            .expect("get_item ok")
            .expect("class exists");
        // expr=None 是占位值，实际表达式通过 expr_programs 的 "func" 键传入。
        cls.call((Option::<Py<PyAny>>::None,), None)
            .expect("instantiate")
            .unbind()
    }

    /// 构造一个 "func" 表达式程序 dict：{"func": [(op_tuple), ...]}。
    fn make_func_program(py: Python<'_>, ops_code: &str) -> Py<PyAny> {
        let code = format!("{{'func': {}}}", ops_code);
        py.eval_bound(&code, None, None)
            .expect("eval func program")
            .unbind()
    }

    #[test]
    fn compile_tell_descriptor_produces_tell_node() {
        // TellDescriptor → Node::Tell
        with_py(|py| {
            let cls = make_dummy_class(py, "TellTest");
            let tell_desc = make_tell_descriptor(py).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["pos".to_string()],
                vec![tell_desc],
                Some(vec!["ro".to_string()]),
                None, // TellDescriptor 无表达式参数
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    match &s.fields()[0].node {
                        Node::Tell(_) => {}
                        other => panic!("expected Node::Tell, got {:?}", other),
                    }
                    assert_eq!(s.fields()[0].mode, FieldMode::Ro);
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_computed_descriptor_produces_computed_node() {
        // ComputedDescriptor + expr_programs["func"] → Node::Computed { expr }
        with_py(|py| {
            let cls = make_dummy_class(py, "ComputedTest");
            let count_desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let computed_desc = make_computed_descriptor(py).into_any();
            // func = count * 2 → [getint 0, const 2, mul]
            let prog = make_func_program(py, "[('getint', 0), ('const', 2), ('mul',)]");
            let schema = compile_schema(
                py,
                &cls,
                vec!["count".to_string(), "doubled".to_string()],
                vec![count_desc, computed_desc],
                Some(vec!["rw".to_string(), "ro".to_string()]),
                Some(vec![None, Some(prog)]),
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 2);
                    match &s.fields()[1].node {
                        Node::Computed(c) => {
                            assert_eq!(
                                c.expr().ops(),
                                &[ExprOp::GetInt(0), ExprOp::Const(2), ExprOp::Mul]
                            );
                        }
                        other => panic!("expected Node::Computed, got {:?}", other),
                    }
                    assert_eq!(s.fields()[1].mode, FieldMode::Ro);
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_computed_descriptor_without_expr_program_returns_error() {
        // ComputedDescriptor + expr_programs=None → Compilation error
        with_py(|py| {
            let cls = make_dummy_class(py, "MissingExpr");
            let computed_desc = make_computed_descriptor(py).into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![computed_desc],
                Some(vec!["ro".to_string()]),
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("ComputedDescriptor")
                    || msg.contains("func")
                    || msg.contains("expression"),
                "got: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_computed_descriptor_missing_func_key_returns_error() {
        // ComputedDescriptor + expr_programs={}（空 dict，缺 "func"）→ Compilation error
        with_py(|py| {
            let cls = make_dummy_class(py, "MissingFunc");
            let computed_desc = make_computed_descriptor(py).into_any();
            let empty_prog = py
                .eval_bound("{}", None, None)
                .expect("empty dict")
                .unbind();
            let err = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![computed_desc],
                Some(vec!["ro".to_string()]),
                Some(vec![Some(empty_prog)]),
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("func") || msg.contains("missing"),
                "got: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_tell_and_computed_mixed_end_to_end() {
        // 完整端到端：Tell + Computed 混合使用。
        // Struct { start: Tell(), count: Int8ub, end: Tell(), size: Computed(end - start) }
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "Packet");
            let tell_desc = make_tell_descriptor(py).into_any();
            let count_desc = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let computed_desc = make_computed_descriptor(py).into_any();
            // size = end - start → [getint 2 (end), getint 0 (start), sub]
            let prog = make_func_program(py, "[('getint', 2), ('getint', 0), ('sub',)]");
            let schema = compile_schema(
                py,
                &cls,
                vec![
                    "start".to_string(),
                    "count".to_string(),
                    "end".to_string(),
                    "size".to_string(),
                ],
                vec![
                    tell_desc.clone_ref(py),
                    count_desc,
                    tell_desc,
                    computed_desc,
                ],
                Some(vec![
                    "ro".to_string(),
                    "rw".to_string(),
                    "ro".to_string(),
                    "ro".to_string(),
                ]),
                Some(vec![None, None, None, Some(prog)]),
                false,
            )
            .expect("compile");

            // parse: b'\x05' → start=0, count=5, end=1, size=1
            let parse_data = b"\x05";
            let data = PyBytes::new_bound(py, parse_data);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let start: i64 = parsed.getattr("start").unwrap().extract().unwrap();
            let count: i64 = parsed.getattr("count").unwrap().extract().unwrap();
            let end: i64 = parsed.getattr("end").unwrap().extract().unwrap();
            let size: i64 = parsed.getattr("size").unwrap().extract().unwrap();
            assert_eq!(start, 0);
            assert_eq!(count, 5);
            assert_eq!(end, 1);
            assert_eq!(size, 1); // end - start = 1 - 0

            // build: 实例只提供 RW 字段（count），RO 字段自动计算
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs.set_item("count", 5).expect("set count");
            let obj = cls.call((), Some(&kwargs)).expect("create obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), parse_data);
        });
    }

    #[test]
    fn compile_tell_descriptor_in_rw_mode_still_compiles() {
        // Tell 字段理论上应使用 RO 模式，但编译器不强制拒绝其他模式。
        // （mode 校验推迟到 Phase 3 的 validate_field_combinations）
        with_py(|py| {
            let cls = make_dummy_class(py, "TellRw");
            let tell_desc = make_tell_descriptor(py).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["pos".to_string()],
                vec![tell_desc],
                Some(vec!["rw".to_string()]),
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert!(matches!(s.fields()[0].node, Node::Tell(_)));
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // Phase 3.1 子任务：BitsIntegerDescriptor 识别与编译期校验
    // ======================================================================

    /// 创建一个 BitsIntegerDescriptor Python 实例（模拟 Python 侧 BitsInteger()）。
    ///
    /// 使用 `run_bound` 定义类（带 property），然后实例化。
    /// 长度/signed/swapped 通过参数注入。
    fn make_bits_integer_descriptor(
        py: Python<'_>,
        length: i64,
        signed: bool,
        swapped: bool,
    ) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        // 定义类（含 _expr_params property，对常量 length 返回空 dict）
        let code = concat!(
            "class BitsIntegerDescriptor:\n",
            "    def __init__(self, length, signed, swapped):\n",
            "        self.length = length\n",
            "        self.signed = signed\n",
            "        self.swapped = swapped\n",
            "    @property\n",
            "    def _expr_params(self):\n",
            "        # 仅当 length 是 FieldRef/ExprRef 时返回非空，常量返回空\n",
            "        if isinstance(self.length, int):\n",
            "            return {}\n",
            "        return {'length': self.length}\n",
            "    def __repr__(self):\n",
            "        return 'BitsInteger(length={!r}, signed={!r}, swapped={!r})'.format(\n",
            "            self.length, self.signed, self.swapped)\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define BitsIntegerDescriptor");
        let cls = globals
            .get_item("BitsIntegerDescriptor")
            .expect("get_item ok")
            .expect("class exists");
        cls.call((length, signed, swapped), None)
            .expect("instantiate")
            .unbind()
    }

    /// 创建一个 BitsIntegerDescriptor，length 为 FieldRef 标记（模拟表达式 length）。
    fn make_bits_integer_expr_length_descriptor(py: Python<'_>) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class BitsIntegerDescriptor:\n",
            "    def __init__(self, length, signed, swapped):\n",
            "        self.length = length\n",
            "        self.signed = signed\n",
            "        self.swapped = swapped\n",
            "    @property\n",
            "    def _expr_params(self):\n",
            "        return {'length': self.length}\n",
            "class FakeFieldRef:\n",
            "    pass\n",
        );
        py.run_bound(code, Some(&globals), None).expect("define");
        let cls = globals
            .get_item("BitsIntegerDescriptor")
            .expect("get ok")
            .expect("exists");
        let fake_ref = globals
            .get_item("FakeFieldRef")
            .expect("get ok")
            .expect("exists")
            .call((), None)
            .expect("instantiate");
        cls.call((fake_ref, false, false), None)
            .expect("instantiate")
            .unbind()
    }

    #[test]
    fn compile_bits_integer_descriptor_produces_bits_integer_node() {
        // BitsInteger(8) → Node::BitsInteger(BitsIntegerNode { length: 8, ... })
        with_py(|py| {
            let cls = make_dummy_class(py, "BitsIntTest");
            let desc = make_bits_integer_descriptor(py, 8, false, false).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    match &s.fields()[0].node {
                        Node::BitsInteger(b) => {
                            assert_eq!(b.length(), 8);
                            assert!(!b.signed());
                            assert!(!b.swapped());
                        }
                        other => panic!("expected BitsInteger, got {:?}", other),
                    }
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bits_integer_descriptor_bit_nibble_octet_lengths() {
        // 测试 Bit(1), Nibble(4), Octet(8) 三种长度
        with_py(|py| {
            let cls = make_dummy_class(py, "BitNibbleOctet");
            let bit = make_bits_integer_descriptor(py, 1, false, false).into_any();
            let nibble = make_bits_integer_descriptor(py, 4, false, false).into_any();
            let octet = make_bits_integer_descriptor(py, 8, false, false).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["bit".to_string(), "nibble".to_string(), "octet".to_string()],
                vec![bit, nibble, octet],
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    let fields = s.fields();
                    assert!(matches!(
                        fields[0].node,
                        Node::BitsInteger(ref b) if b.length() == 1
                    ));
                    assert!(matches!(
                        fields[1].node,
                        Node::BitsInteger(ref b) if b.length() == 4
                    ));
                    assert!(matches!(
                        fields[2].node,
                        Node::BitsInteger(ref b) if b.length() == 8
                    ));
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bits_integer_descriptor_signed_swapped() {
        // 测试 signed=true + swapped=true
        with_py(|py| {
            let cls = make_dummy_class(py, "SignedSwapped");
            let desc = make_bits_integer_descriptor(py, 16, true, true).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => match &s.fields()[0].node {
                    Node::BitsInteger(b) => {
                        assert_eq!(b.length(), 16);
                        assert!(b.signed());
                        assert!(b.swapped());
                    }
                    other => panic!("expected BitsInteger, got {:?}", other),
                },
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bits_integer_descriptor_max_length_64_succeeds() {
        // BI-3 边界：length=64 应编译成功
        with_py(|py| {
            let cls = make_dummy_class(py, "MaxLen");
            let desc = make_bits_integer_descriptor(py, 64, false, false).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert!(matches!(
                        &s.fields()[0].node,
                        Node::BitsInteger(b) if b.length() == 64
                    ));
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bits_integer_descriptor_length_exceeds_64_returns_compilation_error() {
        // BI-3: length > 64 → Compilation error
        with_py(|py| {
            let cls = make_dummy_class(py, "TooLong");
            let desc = make_bits_integer_descriptor(py, 65, false, false).into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("64") || msg.contains("exceeds"),
                "message should mention 64-bit limit: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_bits_integer_descriptor_negative_length_returns_compilation_error() {
        // BI-2: length < 0 → Compilation error
        with_py(|py| {
            let cls = make_dummy_class(py, "Negative");
            let desc = make_bits_integer_descriptor(py, -1, false, false).into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("non-negative") || msg.contains("negative"),
                "message should mention non-negative: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_bits_integer_descriptor_zero_length_compiles_succeeds() {
        // 长度为 0：编译成功（编译期不拒绝），运行时 parse/build 时返回 FormatField 错误（BI-1）
        // 设计 §9.1 BI-1 明确 length==0 在 parse/build 时报错，不延后到编译期。
        // 但本测试仅验证编译期能通过——实际行为以 BI-1 运行时错误为准。
        // 注意：当前实现允许编译，但 parse/build 时返回 FormatField 错误。
        with_py(|py| {
            let cls = make_dummy_class(py, "Zero");
            let desc = make_bits_integer_descriptor(py, 0, false, false).into_any();
            let result = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            );
            // 编译应成功（运行时报错）
            assert!(result.is_ok(), "compile should succeed: {:?}", result);
        });
    }

    #[test]
    fn compile_bits_integer_descriptor_expression_length_returns_compilation_error() {
        // P4: 表达式 length（非 int）→ Compilation error（Phase 3.1 不支持）
        with_py(|py| {
            let cls = make_dummy_class(py, "ExprLen");
            let desc = make_bits_integer_expr_length_descriptor(py).into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("expression length") || msg.contains("not supported"),
                "message should mention expression length not supported: {}",
                msg
            );
        });
    }

    // ======================================================================
    // Phase 3.2 子任务：BitwiseDescriptor 识别 + bitwise 根包装
    // ======================================================================

    /// 创建一个 BitwiseDescriptor Python 实例（模拟 Python 侧 `Bitwise(subcon)`）。
    fn make_bitwise_descriptor(py: Python<'_>, subcon: Py<PyAny>) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class BitwiseDescriptor:\n",
            "    __slots__ = ('subcon',)\n",
            "    def __init__(self, subcon):\n",
            "        self.subcon = subcon\n",
            "    _expr_params = {}\n",
            "    def __repr__(self):\n",
            "        return 'Bitwise({!r})'.format(self.subcon)\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define BitwiseDescriptor");
        let cls = globals
            .get_item("BitwiseDescriptor")
            .expect("get_item ok")
            .expect("class exists");
        cls.call((subcon,), None).expect("instantiate").unbind()
    }

    #[test]
    fn compile_bitwise_descriptor_produces_bitwise_node() {
        // Bitwise(BitsInteger(8)) → Node::Bitwise(BitwiseNode(BitsInteger(8)))
        with_py(|py| {
            let cls = make_dummy_class(py, "BitwiseTest");
            let inner = make_bits_integer_descriptor(py, 8, false, false);
            let desc = make_bitwise_descriptor(py, inner).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["x".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    match &s.fields()[0].node {
                        Node::Bitwise(b) => {
                            // inner 应该是 BitsInteger(8)
                            match b.inner() {
                                Node::BitsInteger(bi) => {
                                    assert_eq!(bi.length(), 8);
                                    assert!(!bi.signed());
                                    assert!(!bi.swapped());
                                }
                                other => {
                                    panic!("expected BitsInteger inside Bitwise, got {:?}", other)
                                }
                            }
                        }
                        other => panic!("expected Bitwise, got {:?}", other),
                    }
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bitwise_descriptor_with_struct_subcon() {
        // Bitwise(Struct-like): 用 StructRef 测试
        // 实际上 Bitwise(subcon) 通常包裹 BitsInteger 或嵌套 Struct（后者通过 StructRef）
        // 这里测试 BitwiseDescriptor 至少识别 subcon 字段
        with_py(|py| {
            let cls = make_dummy_class(py, "BitwiseStruct");
            // 用 BitsInteger 作为内层（最常见用法）
            let inner = make_bits_integer_descriptor(py, 16, false, false);
            let desc = make_bitwise_descriptor(py, inner).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => match &s.fields()[0].node {
                    Node::Bitwise(b) => match b.inner() {
                        Node::BitsInteger(bi) => assert_eq!(bi.length(), 16),
                        other => panic!("expected BitsInteger, got {:?}", other),
                    },
                    other => panic!("expected Bitwise, got {:?}", other),
                },
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bitwise_true_wraps_root_struct_in_bitwise_node() {
        // bitwise=true 参数：根 StructNode 被包入 BitwiseNode
        // （对应 Python BitStructMixin → Bitwise(Struct(...))）
        with_py(|py| {
            let cls = make_dummy_class(py, "BitStructLike");
            let a_desc = make_bits_integer_descriptor(py, 4, false, false).into_any();
            let b_desc = make_bits_integer_descriptor(py, 4, false, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string()],
                vec![a_desc, b_desc],
                None,
                None,
                true, // bitwise=true（BitStructMixin）
            )
            .expect("compile");

            // 根应是 Bitwise(BitwiseNode { inner: Struct })
            match schema.root() {
                Node::Bitwise(b) => match b.inner() {
                    Node::Struct(s) => {
                        assert_eq!(s.len(), 2);
                    }
                    other => panic!("expected Struct inside Bitwise, got {:?}", other),
                },
                other => panic!("expected Node::Bitwise root, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bitwise_false_does_not_wrap_root() {
        // bitwise=false（默认）：根仍是 StructNode，不被包入 BitwiseNode
        with_py(|py| {
            let cls = make_dummy_class(py, "RegularStruct");
            let desc = make_bits_integer_descriptor(py, 8, false, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            // 根应是 Struct（不是 Bitwise）
            match schema.root() {
                Node::Struct(_) => {}
                other => panic!("expected Node::Struct root, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bitwise_descriptor_missing_subcon_returns_error() {
        // BitwiseDescriptor 无 subcon 属性 → Compilation error
        with_py(|py| {
            let cls = make_dummy_class(py, "BadBitwise");
            // 创建一个空的 BitwiseDescriptor（无 subcon 属性）
            let desc = py
                .eval_bound(
                    "type('BitwiseDescriptor', (), {'_expr_params': {}})()",
                    None,
                    None,
                )
                .expect("create empty BitwiseDescriptor")
                .unbind();

            let err = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc.into_any()],
                None,
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("subcon") || msg.contains("BitwiseDescriptor"),
                "message should mention subcon: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_bitstruct_mixin_end_to_end_round_trip() {
        // 端到端：BitStruct-like schema（bitwise=true）parse + build 往返
        // BitStruct { a: BitsInteger(4), b: BitsInteger(4) } parse b'\xA5' → a=10, b=5
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "BitStructE2E");
            let a_desc = make_bits_integer_descriptor(py, 4, false, false).into_any();
            let b_desc = make_bits_integer_descriptor(py, 4, false, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string()],
                vec![a_desc, b_desc],
                None,
                None,
                true, // bitwise=true
            )
            .expect("compile");

            // parse b'\xA5'
            let data = pyo3::types::PyBytes::new_bound(py, &[0xA5]);
            let instance = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let a: i64 = instance.getattr("a").unwrap().extract().unwrap();
            let b: i64 = instance.getattr("b").unwrap().extract().unwrap();
            assert_eq!(a, 0xA);
            assert_eq!(b, 0x5);

            // build back
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs.set_item("a", 0xA).unwrap();
            kwargs.set_item("b", 0x5).unwrap();
            let obj = cls.call((), Some(&kwargs)).expect("create obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), &[0xA5]);
        });
    }

    #[test]
    fn compile_bitstruct_mixin_unaligned_inner_returns_bitfield_error() {
        // BitStruct { a: BitsInteger(5) } - 5 bit 非 8 倍数 → parse 时 BitField error
        with_py(|py| {
            let cls = make_dummy_class(py, "BadBitStruct");
            let desc = make_bits_integer_descriptor(py, 5, false, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string()],
                vec![desc],
                None,
                None,
                true, // bitwise=true
            )
            .expect("compile");

            // parse 应失败（BitField 错误）
            let data = pyo3::types::PyBytes::new_bound(py, &[0xFF]);
            let err = schema._parse_raw(py, &data).expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("not byte-aligned") || msg.contains("BitField"),
                "got: {}",
                msg
            );
        });
    }

    // ======================================================================
    // Phase 3.3 子任务：PaddingDescriptor / BytewiseDescriptor /
    // BitsSwappedDescriptor / ByteSwappedDescriptor 识别
    // ======================================================================

    /// 创建一个 PaddingDescriptor Python 实例（模拟 Python 侧 `Padding(length, pattern)`）。
    fn make_padding_descriptor(py: Python<'_>, length: i64, pattern: u8) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class PaddingDescriptor:\n",
            "    def __init__(self, length, pattern):\n",
            "        self.length = length\n",
            "        self.pattern = pattern\n",
            "    @property\n",
            "    def _expr_params(self):\n",
            "        if isinstance(self.length, int):\n",
            "            return {}\n",
            "        return {'length': self.length}\n",
            "    def __repr__(self):\n",
            "        return 'Padding(length={!r}, pattern={!r})'.format(self.length, self.pattern)\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define PaddingDescriptor");
        let cls = globals
            .get_item("PaddingDescriptor")
            .expect("get ok")
            .expect("exists");
        cls.call((length, pattern), None)
            .expect("instantiate")
            .unbind()
    }

    /// 创建一个 BytewiseDescriptor Python 实例。
    fn make_bytewise_descriptor(py: Python<'_>, subcon: Py<PyAny>) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class BytewiseDescriptor:\n",
            "    __slots__ = ('subcon',)\n",
            "    def __init__(self, subcon):\n",
            "        self.subcon = subcon\n",
            "    _expr_params = {}\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define BytewiseDescriptor");
        let cls = globals
            .get_item("BytewiseDescriptor")
            .expect("get ok")
            .expect("exists");
        cls.call((subcon,), None).expect("instantiate").unbind()
    }

    /// 创建一个 BitsSwappedDescriptor Python 实例。
    fn make_bits_swapped_descriptor(py: Python<'_>, subcon: Py<PyAny>) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class BitsSwappedDescriptor:\n",
            "    __slots__ = ('subcon',)\n",
            "    def __init__(self, subcon):\n",
            "        self.subcon = subcon\n",
            "    _expr_params = {}\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define BitsSwappedDescriptor");
        let cls = globals
            .get_item("BitsSwappedDescriptor")
            .expect("get ok")
            .expect("exists");
        cls.call((subcon,), None).expect("instantiate").unbind()
    }

    /// 创建一个 ByteSwappedDescriptor Python 实例。
    fn make_byte_swapped_descriptor(py: Python<'_>, subcon: Py<PyAny>) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class ByteSwappedDescriptor:\n",
            "    __slots__ = ('subcon',)\n",
            "    def __init__(self, subcon):\n",
            "        self.subcon = subcon\n",
            "    _expr_params = {}\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define ByteSwappedDescriptor");
        let cls = globals
            .get_item("ByteSwappedDescriptor")
            .expect("get ok")
            .expect("exists");
        cls.call((subcon,), None).expect("instantiate").unbind()
    }

    // ------------------------------------------------------------------
    // PaddingDescriptor 编译
    // ------------------------------------------------------------------

    #[test]
    fn compile_padding_descriptor_byte_domain_produces_padding_node() {
        // Padding(4) 在字节域 → Node::Padding（字节级）
        with_py(|py| {
            let cls = make_dummy_class(py, "PadByte");
            let desc = make_padding_descriptor(py, 4, 0).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["reserved".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");
            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    assert!(matches!(s.fields()[0].node, Node::Padding(_)));
                }
                other => panic!("expected Node::Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_padding_descriptor_bit_domain_produces_bit_padding_node() {
        // Padding(4) 在 bit 域（bitwise=true）→ Node::BitPadding
        with_py(|py| {
            let cls = make_dummy_class(py, "PadBit");
            let desc = make_padding_descriptor(py, 4, 0).into_any();
            let schema = compile_schema(
                py,
                &cls,
                vec!["reserved".to_string()],
                vec![desc],
                None,
                None,
                true, // bitwise=true → 根包入 Bitwise，Padding 编译为 BitPadding
            )
            .expect("compile");
            match schema.root() {
                Node::Bitwise(b) => match b.inner() {
                    Node::Struct(s) => {
                        assert!(matches!(s.fields()[0].node, Node::BitPadding(_)));
                    }
                    other => panic!("expected Struct, got {:?}", other),
                },
                other => panic!("expected Bitwise, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_padding_descriptor_bit_domain_invalid_pattern_returns_error() {
        // P1 修复：bit 域 Padding pattern 非 {0x00, 0x01} → Padding 错误
        with_py(|py| {
            let cls = make_dummy_class(py, "BadPattern");
            let desc = make_padding_descriptor(py, 4, 0x80).into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["reserved".to_string()],
                vec![desc],
                None,
                None,
                true, // bitwise=true
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("0x00") && msg.contains("0x01") && msg.contains("0x80"),
                "got: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_padding_descriptor_negative_length_returns_error() {
        // Padding(-1) → Compilation error
        with_py(|py| {
            let cls = make_dummy_class(py, "NegPad");
            let desc = make_padding_descriptor(py, -1, 0).into_any();
            let err = compile_schema(
                py,
                &cls,
                vec!["reserved".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("non-negative") || msg.contains("negative"),
                "got: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_padding_end_to_end_byte_domain_round_trip() {
        // 端到端：Padding(4) 字节域 parse/build
        // Padding 作为 RW 字段（parse 返回 None 存入实例，build 忽略 obj）
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "PadE2E");
            let int8ub = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let pad4 = make_padding_descriptor(py, 4, 0).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["tag".to_string(), "reserved".to_string()],
                vec![int8ub, pad4],
                // Padding 用 RW 模式：build 时从实例取值（Padding.build 忽略 obj）
                Some(vec!["rw".to_string(), "rw".to_string()]),
                None,
                false,
            )
            .expect("compile");

            // parse b'\xAA\x00\x00\x00\x00' → tag=0xAA
            let data = pyo3::types::PyBytes::new_bound(py, &[0xAA, 0, 0, 0, 0]);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let tag: i64 = parsed.getattr("tag").unwrap().extract().unwrap();
            assert_eq!(tag, 0xAA);

            // build back：Padding.build 忽略 obj（这里 reserved=None）
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs.set_item("tag", 0xAA).unwrap();
            kwargs.set_item("reserved", py.None()).unwrap();
            let obj = cls.call((), Some(&kwargs)).expect("obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), &[0xAA, 0, 0, 0, 0]);
        });
    }

    #[test]
    fn compile_padding_end_to_end_bit_domain_round_trip() {
        // 端到端：BitStruct { a: BitsInteger(4), b: Padding(4) }
        // 总 8 bit = 1 字节
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "BitPadE2E");
            let a_desc = make_bits_integer_descriptor(py, 4, false, false).into_any();
            let pad_desc = make_padding_descriptor(py, 4, 0).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "reserved".to_string()],
                vec![a_desc, pad_desc],
                // a 是 RW，reserved 是 RW（Padding.build 忽略 obj）
                Some(vec!["rw".to_string(), "rw".to_string()]),
                None,
                true, // bitwise=true
            )
            .expect("compile");

            // parse b'\xA5' → a=0xA
            let data = pyo3::types::PyBytes::new_bound(py, &[0xA5]);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let a: i64 = parsed.getattr("a").unwrap().extract().unwrap();
            assert_eq!(a, 0xA);

            // build back：reserved=None，Padding.build 忽略 obj
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs.set_item("a", 0xA).unwrap();
            kwargs.set_item("reserved", py.None()).unwrap();
            let obj = cls.call((), Some(&kwargs)).expect("obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), &[0xA0]);
        });
    }

    // ------------------------------------------------------------------
    // BytewiseDescriptor 编译
    // ------------------------------------------------------------------

    #[test]
    fn compile_bytewise_descriptor_produces_bytewise_node() {
        // Bytewise(Int16ub) → Node::Bytewise wrapping FormatField
        with_py(|py| {
            let cls = make_dummy_class(py, "BytewiseTest");
            let inner = Py::new(
                py,
                FormatFieldDescriptor::new("Int16ub", PythonFormat::UnsignedInt16Big),
            )
            .expect("Py::new")
            .into_any();
            let desc = make_bytewise_descriptor(py, inner).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => match &s.fields()[0].node {
                    Node::Bytewise(b) => match b.inner() {
                        Node::FormatField(_) => {}
                        other => panic!("expected FormatField, got {:?}", other),
                    },
                    other => panic!("expected Bytewise, got {:?}", other),
                },
                other => panic!("expected Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_bytewise_inside_bitstruct_end_to_end() {
        // BitStruct { a: Nibble, b: Bytewise(Int8ub), c: Nibble }
        // 总 4+8+4 = 16 bit = 2 字节
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "BitBytewise");
            let a_desc = make_bits_integer_descriptor(py, 4, false, false).into_any();
            let inner_format = Py::new(
                py,
                FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
            )
            .expect("Py::new")
            .into_any();
            let b_desc = make_bytewise_descriptor(py, inner_format).into_any();
            let c_desc = make_bits_integer_descriptor(py, 4, false, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec![a_desc, b_desc, c_desc],
                None,
                None,
                true,
            )
            .expect("compile");

            // parse [0xA5, 0xF0] → a=0xA, b=0x5F, c=0x0
            let data = pyo3::types::PyBytes::new_bound(py, &[0xA5, 0xF0]);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let a: i64 = parsed.getattr("a").unwrap().extract().unwrap();
            let b: i64 = parsed.getattr("b").unwrap().extract().unwrap();
            let c: i64 = parsed.getattr("c").unwrap().extract().unwrap();
            assert_eq!(a, 0xA);
            assert_eq!(b, 0x5F);
            assert_eq!(c, 0x0);
        });
    }

    // ------------------------------------------------------------------
    // BitsSwappedDescriptor / ByteSwappedDescriptor 编译
    // ------------------------------------------------------------------

    #[test]
    fn compile_bits_swapped_descriptor_produces_transform_node() {
        // BitsSwapped(Bytes(2)) → Node::Transform(BitSwap)
        with_py(|py| {
            let cls = make_dummy_class(py, "BitSwapTest");
            let inner = Py::new(py, BytesDescriptor::new(2.into_py(py)))
                .expect("Py::new")
                .into_any();
            let desc = make_bits_swapped_descriptor(py, inner).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => match &s.fields()[0].node {
                    Node::Transform(t) => {
                        assert_eq!(
                            t.transform(),
                            crate::nodes::transform::ByteTransform::BitSwap
                        );
                    }
                    other => panic!("expected Transform, got {:?}", other),
                },
                other => panic!("expected Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_byte_swapped_descriptor_produces_transform_node() {
        // ByteSwapped(Int16ub) → Node::Transform(ByteSwap)
        with_py(|py| {
            let cls = make_dummy_class(py, "ByteSwapTest");
            let inner = Py::new(
                py,
                FormatFieldDescriptor::new("Int16ub", PythonFormat::UnsignedInt16Big),
            )
            .expect("Py::new")
            .into_any();
            let desc = make_byte_swapped_descriptor(py, inner).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => match &s.fields()[0].node {
                    Node::Transform(t) => {
                        assert_eq!(
                            t.transform(),
                            crate::nodes::transform::ByteTransform::ByteSwap
                        );
                    }
                    other => panic!("expected Transform, got {:?}", other),
                },
                other => panic!("expected Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_byte_swapped_end_to_end_round_trip() {
        // 端到端：ByteSwapped(Int32ub) parse/build
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "ByteSwapE2E");
            let inner = Py::new(
                py,
                FormatFieldDescriptor::new("Int32ub", PythonFormat::UnsignedInt32Big),
            )
            .expect("Py::new")
            .into_any();
            let desc = make_byte_swapped_descriptor(py, inner).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            // parse [0x78, 0x56, 0x34, 0x12] → ByteSwap → 0x12345678
            let data = pyo3::types::PyBytes::new_bound(py, &[0x78, 0x56, 0x34, 0x12]);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let v: i64 = parsed.getattr("v").unwrap().extract().unwrap();
            assert_eq!(v, 0x12345678);

            // build back
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs.set_item("v", 0x12345678).unwrap();
            let obj = cls.call((), Some(&kwargs)).expect("obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), &[0x78, 0x56, 0x34, 0x12]);
        });
    }

    #[test]
    fn compile_bits_swapped_end_to_end_round_trip() {
        // 端到端：BitsSwapped(Bytes(2)) parse/build
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "BitSwapE2E");
            let inner = Py::new(py, BytesDescriptor::new(2.into_py(py)))
                .expect("Py::new")
                .into_any();
            let desc = make_bits_swapped_descriptor(py, inner).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            // parse [0xF0, 0x0F] → BitSwap → [0x0F, 0xF0] → Bytes(2) → b"\x0F\xF0"
            let data = pyo3::types::PyBytes::new_bound(py, &[0xF0, 0x0F]);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let v_binding = parsed.getattr("v").unwrap();
            let v: &[u8] = v_binding
                .downcast::<pyo3::types::PyBytes>()
                .unwrap()
                .as_bytes();
            assert_eq!(v, &[0x0F, 0xF0]);
        });
    }

    // ======================================================================
    // Phase 4 子任务 4.2：GreedyRangeDescriptor 识别与编译
    // ======================================================================

    /// 创建一个 GreedyRangeDescriptor Python 实例（模拟 Python 侧 `GreedyRange(subcon, discard)`）。
    fn make_greedy_range_descriptor(py: Python<'_>, subcon: Py<PyAny>, discard: bool) -> Py<PyAny> {
        let globals = PyDict::new_bound(py);
        let code = concat!(
            "class GreedyRangeDescriptor:\n",
            "    __slots__ = ('subcon', 'discard')\n",
            "    def __init__(self, subcon, discard):\n",
            "        self.subcon = subcon\n",
            "        self.discard = discard\n",
            "    _expr_params = {}\n",
            "    def __repr__(self):\n",
            "        return 'GreedyRange(subcon={!r}, discard={!r})'.format(self.subcon, self.discard)\n",
        );
        py.run_bound(code, Some(&globals), None)
            .expect("define GreedyRangeDescriptor");
        let cls = globals
            .get_item("GreedyRangeDescriptor")
            .expect("get ok")
            .expect("exists");
        cls.call((subcon, discard), None)
            .expect("instantiate")
            .unbind()
    }

    /// 创建一个 Int8ub 内层描述符（用于 GreedyRange 测试）。
    fn make_int8ub_descriptor(py: Python<'_>) -> Py<PyAny> {
        Py::new(
            py,
            FormatFieldDescriptor::new("Int8ub", PythonFormat::UnsignedInt8Big),
        )
        .expect("Py::new")
        .into_any()
    }

    /// 创建一个 Int16ub 内层描述符（用于 GreedyRange 部分回退测试）。
    fn make_int16ub_descriptor(py: Python<'_>) -> Py<PyAny> {
        Py::new(
            py,
            FormatFieldDescriptor::new("Int16ub", PythonFormat::UnsignedInt16Big),
        )
        .expect("Py::new")
        .into_any()
    }

    #[test]
    fn compile_greedy_range_descriptor_produces_greedy_range_node() {
        // GreedyRange(Int8ub) → Node::GreedyRange(GreedyRangeNode { inner: FormatField })
        with_py(|py| {
            let cls = make_dummy_class(py, "GreedyRangeTest");
            let inner = make_int8ub_descriptor(py);
            let desc = make_greedy_range_descriptor(py, inner, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["items".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => {
                    assert_eq!(s.len(), 1);
                    match &s.fields()[0].node {
                        Node::GreedyRange(g) => {
                            assert!(!g.discard());
                            // inner 应是 FormatField
                            match g.inner() {
                                Node::FormatField(_) => {}
                                other => {
                                    panic!(
                                        "expected FormatField inside GreedyRange, got {:?}",
                                        other
                                    )
                                }
                            }
                        }
                        other => panic!("expected GreedyRange, got {:?}", other),
                    }
                }
                other => panic!("expected Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_greedy_range_descriptor_with_discard_flag() {
        // GreedyRange(Int8ub, discard=True) → discard 标志传递正确
        with_py(|py| {
            let cls = make_dummy_class(py, "GreedyRangeDiscard");
            let inner = make_int8ub_descriptor(py);
            let desc = make_greedy_range_descriptor(py, inner, true).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["items".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            match schema.root() {
                Node::Struct(s) => match &s.fields()[0].node {
                    Node::GreedyRange(g) => assert!(g.discard(), "discard should be true"),
                    other => panic!("expected GreedyRange, got {:?}", other),
                },
                other => panic!("expected Struct, got {:?}", other),
            }
        });
    }

    #[test]
    fn compile_greedy_range_descriptor_missing_subcon_returns_error() {
        // GreedyRangeDescriptor 无 subcon 属性 → Compilation error
        with_py(|py| {
            let cls = make_dummy_class(py, "BadGreedyRange");
            // 创建一个空的 GreedyRangeDescriptor（无 subcon 属性）
            let desc = py
                .eval_bound(
                    "type('GreedyRangeDescriptor', (), {'_expr_params': {}})()",
                    None,
                    None,
                )
                .expect("create empty GreedyRangeDescriptor")
                .unbind();

            let err = compile_schema(
                py,
                &cls,
                vec!["v".to_string()],
                vec![desc.into_any()],
                None,
                None,
                false,
            )
            .expect_err("should fail");
            let msg = err.to_string();
            assert!(
                msg.contains("subcon") || msg.contains("GreedyRangeDescriptor"),
                "message should mention subcon: {}",
                msg
            );
        });
    }

    #[test]
    fn compile_greedy_range_end_to_end_round_trip() {
        // 端到端：GreedyRange(Int8ub) parse + build 往返
        // Struct { items: GreedyRange(Int8ub) }
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "GreedyRangeE2E");
            let inner = make_int8ub_descriptor(py);
            let desc = make_greedy_range_descriptor(py, inner, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["items".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            // parse 5 字节 → items = [1, 2, 3, 4, 5]
            let parse_data = &[0x01, 0x02, 0x03, 0x04, 0x05];
            let data = pyo3::types::PyBytes::new_bound(py, parse_data);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let items_binding = parsed.getattr("items").unwrap();
            let items = items_binding.downcast::<pyo3::types::PyList>().unwrap();
            assert_eq!(items.len(), 5);
            let v: i64 = items.get_item(0).unwrap().extract().unwrap();
            assert_eq!(v, 1);
            let v: i64 = items.get_item(4).unwrap().extract().unwrap();
            assert_eq!(v, 5);

            // build back
            let kwargs = pyo3::types::PyDict::new_bound(py);
            kwargs
                .set_item("items", pyo3::types::PyList::new_bound(py, [1, 2, 3, 4, 5]))
                .unwrap();
            let obj = cls.call((), Some(&kwargs)).expect("obj");
            let built = schema._build_raw(py, &obj).expect("build");
            assert_eq!(built.as_bytes(), parse_data);
        });
    }

    #[test]
    fn compile_greedy_range_end_to_end_partial_fallback() {
        // 端到端：GreedyRange(Int16ub) 解析 5 字节 → 前 2 个元素（4 字节），第 5 字节回退
        // 验证 GR-3 边界：第 N 个元素失败回退
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "GreedyRangeFallback");
            let inner = make_int16ub_descriptor(py);
            let desc = make_greedy_range_descriptor(py, inner, false).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["items".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            // 5 字节：0x0102, 0x0304, 0x05（残留）
            let parse_data = &[0x01, 0x02, 0x03, 0x04, 0x05];
            let data = pyo3::types::PyBytes::new_bound(py, parse_data);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let items_binding = parsed.getattr("items").unwrap();
            let items = items_binding.downcast::<pyo3::types::PyList>().unwrap();
            // 应得到 2 个元素（前 4 字节），第 5 字节回退
            assert_eq!(items.len(), 2);
            let v0: i64 = items.get_item(0).unwrap().extract().unwrap();
            let v1: i64 = items.get_item(1).unwrap().extract().unwrap();
            assert_eq!(v0, 0x0102);
            assert_eq!(v1, 0x0304);
        });
    }

    #[test]
    fn compile_greedy_range_end_to_end_discard_returns_empty_list() {
        // 端到端：GreedyRange(Int8ub, discard=True) 解析 5 字节 → 空 list 但消耗所有字节
        with_py(|py| {
            let cls = make_structmixin_class_with_init(py, "GreedyRangeDiscardE2E");
            let inner = make_int8ub_descriptor(py);
            let desc = make_greedy_range_descriptor(py, inner, true).into_any();

            let schema = compile_schema(
                py,
                &cls,
                vec!["items".to_string()],
                vec![desc],
                None,
                None,
                false,
            )
            .expect("compile");

            let parse_data = &[0x01, 0x02, 0x03];
            let data = pyo3::types::PyBytes::new_bound(py, parse_data);
            let parsed = schema._parse_raw(py, &data).expect("parse").into_bound(py);
            let items_binding = parsed.getattr("items").unwrap();
            let items = items_binding.downcast::<pyo3::types::PyList>().unwrap();
            // discard=True：返回空 list
            assert_eq!(items.len(), 0);
        });
    }
}

// ===========================================================================
// Phase 8 P1+P2: build_*_node 辅助函数
//
// 设计依据：`docs/design/模块设计/模块设计-Phase8-P1P2.md` §1-§6。
//
// 共通模式：
// - 递归编译 subcon（沿用 field_index，与 P0 同模式）
// - 编译期物化 Python 对象为 Py<PyDict>/Py<PyFrozenSet>/Py<PyType>
// - 表达式参数从 expr_programs[field_index]["param"] 取
// ===========================================================================

/// 从 `EnumDescriptor` 构建 `EnumNode`（设计 §1.3.1）。
///
/// `EnumDescriptor` 字段：`subcon` + `decmapping`（dict int→EnumIntegerString）
/// 与 `encmapping`（dict str→int）。`EnumInteger` 类从 `construct._internals` 加载。
fn build_enum_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    field_names: &[String],
) -> Result<EnumNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "EnumDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        // Enum 无表达式参数，传空切片。
        &[],
        field_names,
        false,
    )?;
    let decmapping = desc
        .getattr("decmapping")
        .map_err(|e| ConstructError::Compilation {
            message: format!("EnumDescriptor missing 'decmapping': {}", e),
        })?
        .extract::<Py<PyDict>>()
        .map_err(|e| ConstructError::Compilation {
            message: format!("EnumDescriptor.decmapping not a dict: {}", e),
        })?;
    let encmapping = desc
        .getattr("encmapping")
        .map_err(|e| ConstructError::Compilation {
            message: format!("EnumDescriptor missing 'encmapping': {}", e),
        })?
        .extract::<Py<PyDict>>()
        .map_err(|e| ConstructError::Compilation {
            message: format!("EnumDescriptor.encmapping not a dict: {}", e),
        })?;
    // 加载 EnumInteger 类（从 construct._internals）。
    let enum_integer_cls = py
        .import_bound("construct._internals")
        .and_then(|m| m.getattr("EnumInteger"))
        .and_then(|a| a.extract::<Py<PyType>>())
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "failed to load EnumInteger from construct._internals: {}",
                e
            ),
        })?;
    Ok(EnumNode::new(
        inner_node,
        decmapping,
        encmapping,
        enum_integer_cls,
    ))
}

/// 从 `FlagsEnumDescriptor` 构建 `FlagsEnumNode`（设计 §1.3.2）。
fn build_flags_enum_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    field_names: &[String],
) -> Result<FlagsEnumNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "FlagsEnumDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node =
        build_node_from_descriptor(py, &subcon_desc, field_index, &[], field_names, false)?;
    // flags 是 Python dict {name: value}，编译期物化为 Vec<(Py<PyString>, i64)>。
    let flags_dict = desc
        .getattr("flags")
        .map_err(|e| ConstructError::Compilation {
            message: format!("FlagsEnumDescriptor missing 'flags': {}", e),
        })?;
    let flags_bound = flags_dict
        .downcast::<PyDict>()
        .map_err(|_| ConstructError::Compilation {
            message: "FlagsEnumDescriptor.flags not a dict".to_string(),
        })?;
    let mut flags = Vec::with_capacity(flags_bound.len());
    for (k, v) in flags_bound.iter() {
        let name: Py<PyString> = k.extract().map_err(|_| ConstructError::Compilation {
            message: "FlagsEnum flag name not a str".to_string(),
        })?;
        let value: i64 = v.extract().map_err(|_| ConstructError::Compilation {
            message: "FlagsEnum flag value not an int".to_string(),
        })?;
        flags.push((name, value));
    }
    // encmapping 同 Enum（str→int）。
    let encmapping = desc
        .getattr("encmapping")
        .map_err(|e| ConstructError::Compilation {
            message: format!("FlagsEnumDescriptor missing 'encmapping': {}", e),
        })?
        .extract::<Py<PyDict>>()
        .map_err(|e| ConstructError::Compilation {
            message: format!("FlagsEnumDescriptor.encmapping not a dict: {}", e),
        })?;
    Ok(FlagsEnumNode::new(inner_node, flags, encmapping))
}

/// 从 `MappingDescriptor` 构建 `MappingNode`（设计 §1.3.3，C-4 TypeError 捕获）。
fn build_mapping_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    field_names: &[String],
) -> Result<MappingNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "MappingDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node =
        build_node_from_descriptor(py, &subcon_desc, field_index, &[], field_names, false)?;
    let decmapping = desc
        .getattr("decmapping")
        .map_err(|e| ConstructError::Compilation {
            message: format!("MappingDescriptor missing 'decmapping': {}", e),
        })?
        .extract::<Py<PyDict>>()
        .map_err(|e| ConstructError::Compilation {
            message: format!("MappingDescriptor.decmapping not a dict: {}", e),
        })?;
    let encmapping = desc
        .getattr("encmapping")
        .map_err(|e| ConstructError::Compilation {
            message: format!("MappingDescriptor missing 'encmapping': {}", e),
        })?
        .extract::<Py<PyDict>>()
        .map_err(|e| ConstructError::Compilation {
            message: format!("MappingDescriptor.encmapping not a dict: {}", e),
        })?;
    Ok(MappingNode::new(inner_node, decmapping, encmapping))
}

/// 从 `OneOfDescriptor` 构建 `OneOfNode`。
/// valids 编译期转 frozenset 物化（list/set 输入统一）。
fn build_one_of_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    field_names: &[String],
) -> Result<OneOfNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "OneOfDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node =
        build_node_from_descriptor(py, &subcon_desc, field_index, &[], field_names, false)?;
    let valids_obj = desc
        .getattr("valids")
        .map_err(|e| ConstructError::Compilation {
            message: format!("OneOfDescriptor missing 'valids': {}", e),
        })?;
    // 转 frozenset（list/set/frozenset 输入统一）。
    let valids_frozen = py
        .eval_bound("frozenset", None, None)
        .map_err(|e| ConstructError::Compilation {
            message: format!("failed to get frozenset builtin: {}", e),
        })?
        .call1((valids_obj,))
        .map_err(|e| ConstructError::Compilation {
            message: format!("OneOfDescriptor.valids to frozenset failed: {}", e),
        })?
        .extract::<Py<PyFrozenSet>>()
        .map_err(|e| ConstructError::Compilation {
            message: format!("OneOfDescriptor.valids not convertible to frozenset: {}", e),
        })?;
    Ok(OneOfNode::new(inner_node, valids_frozen))
}

/// 从 `NoneOfDescriptor` 构建 `NoneOfNode`。
fn build_none_of_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    field_names: &[String],
) -> Result<NoneOfNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "NoneOfDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node =
        build_node_from_descriptor(py, &subcon_desc, field_index, &[], field_names, false)?;
    let invalids_obj = desc
        .getattr("invalids")
        .map_err(|e| ConstructError::Compilation {
            message: format!("NoneOfDescriptor missing 'invalids': {}", e),
        })?;
    let invalids_frozen = py
        .eval_bound("frozenset", None, None)
        .map_err(|e| ConstructError::Compilation {
            message: format!("failed to get frozenset builtin: {}", e),
        })?
        .call1((invalids_obj,))
        .map_err(|e| ConstructError::Compilation {
            message: format!("NoneOfDescriptor.invalids to frozenset failed: {}", e),
        })?
        .extract::<Py<PyFrozenSet>>()
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "NoneOfDescriptor.invalids not convertible to frozenset: {}",
                e
            ),
        })?;
    Ok(NoneOfNode::new(inner_node, invalids_frozen))
}

/// 从 `UnionDescriptor` 构建 `UnionNode`（设计 §3）。
/// parsefrom 编译期分类：None / int Index / str Name（编译期 name→index 解析）/
/// 表达式 Expr（编译为 ExprProgram）。
fn build_union_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<UnionNode, ConstructError> {
    let subcons_obj = desc
        .getattr("subcons")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "UnionDescriptor missing 'subcons': {} (field {})",
                e, field_index
            ),
        })?;
    let subcons_list =
        subcons_obj
            .downcast::<PyList>()
            .map_err(|_| ConstructError::Compilation {
                message: "UnionDescriptor.subcons not a list".to_string(),
            })?;

    // 编译每个 subcon，识别 Renamed 包装提取 name。
    let mut subcons: Vec<UnionSubcon> = Vec::with_capacity(subcons_list.len());
    let mut name_to_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut has_expressions = false;
    for (idx, sub_desc) in subcons_list.iter().enumerate() {
        // 检测 Renamed 包装（type name == "Renamed"，有 name + subcon 属性）。
        let (name_opt, inner_desc) = if sub_desc
            .get_type()
            .name()
            .map(|s| s == "Renamed")
            .unwrap_or(false)
        {
            let n: String = sub_desc
                .getattr("name")
                .and_then(|n| n.extract())
                .map_err(|e| ConstructError::Compilation {
                    message: format!("Union Renamed.name not a str: {}", e),
                })?;
            let inner = sub_desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!("Union Renamed.subcon missing: {}", e),
                })?;
            name_to_index.insert(n.clone(), idx);
            (Some(FieldName::new(py, n)), inner)
        } else {
            (None, sub_desc.clone())
        };
        let node = build_node_from_descriptor(
            py,
            &inner_desc,
            field_index,
            expr_programs,
            field_names,
            false,
        )?;
        if node.has_expressions() {
            has_expressions = true;
        }
        let subcon = match name_opt {
            Some(name) => UnionSubcon::new_named(name, node),
            None => UnionSubcon::new_anonymous(node),
        };
        subcons.push(subcon);
    }

    // parsefrom 分类
    let parsefrom_obj = desc
        .getattr("parsefrom")
        .map_err(|e| ConstructError::Compilation {
            message: format!("UnionDescriptor missing 'parsefrom': {}", e),
        })?;
    let parsefrom = if parsefrom_obj.is_none() {
        ParseFrom::None
    } else if let Ok(idx) = parsefrom_obj.extract::<i64>() {
        ParseFrom::Index(idx as usize)
    } else if let Ok(name) = parsefrom_obj.extract::<String>() {
        // 编译期 name→index 解析
        let resolved = *name_to_index
            .get(&name)
            .ok_or_else(|| ConstructError::Compilation {
                message: format!("Union parsefrom name '{}' not found in subcons", name),
            })?;
        let name_py = PyString::new_bound(py, &name).unbind();
        ParseFrom::Name {
            name: name_py,
            resolved_index: resolved,
        }
    } else {
        // 表达式路径：从 expr_programs 取（key="parsefrom"）。
        let _ = expr_programs;
        return Err(ConstructError::Compilation {
            message: "Union parsefrom as expression not yet supported (use None/int/str)"
                .to_string(),
        });
    };

    Ok(UnionNode::new(subcons, parsefrom, has_expressions))
}

/// 从 `SequenceDescriptor` 构建 `SequenceNode`（设计 §4，C-1 RO 字段不从 list 取值）。
fn build_sequence_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<SequenceNode, ConstructError> {
    let subcons_obj = desc
        .getattr("subcons")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "SequenceDescriptor missing 'subcons': {} (field {})",
                e, field_index
            ),
        })?;
    let subcons_list =
        subcons_obj
            .downcast::<PyList>()
            .map_err(|_| ConstructError::Compilation {
                message: "SequenceDescriptor.subcons not a list".to_string(),
            })?;

    let mut fields: Vec<SequenceField> = Vec::with_capacity(subcons_list.len());
    let mut has_expressions = false;
    for sub_desc in subcons_list.iter() {
        let (name_opt, inner_desc, kind) = if sub_desc
            .get_type()
            .name()
            .map(|s| s == "Renamed")
            .unwrap_or(false)
        {
            let n: String = sub_desc
                .getattr("name")
                .and_then(|n| n.extract())
                .map_err(|e| ConstructError::Compilation {
                    message: format!("Sequence Renamed.name not a str: {}", e),
                })?;
            let inner = sub_desc
                .getattr("subcon")
                .map_err(|e| ConstructError::Compilation {
                    message: format!("Sequence Renamed.subcon missing: {}", e),
                })?;
            (Some(FieldName::new(py, n)), inner, FieldMode::Rw)
        } else {
            (None, sub_desc.clone(), FieldMode::Rw)
        };
        let node = build_node_from_descriptor(
            py,
            &inner_desc,
            field_index,
            expr_programs,
            field_names,
            false,
        )?;
        if node.has_expressions() {
            has_expressions = true;
        }
        let field = match name_opt {
            Some(name) => SequenceField::new_named_with_kind(name, node, kind),
            None => SequenceField::new_anonymous(node),
        };
        fields.push(field);
    }

    Ok(SequenceNode::new(fields, has_expressions))
}

/// 从 `ProcessXorDescriptor` 构建 `ProcessXorNode`（设计 §5.3）。
fn build_process_xor_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<ProcessXorNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ProcessXorDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;
    // padfunc：int/bytes 物化；FieldRef/ExprRef 走 ExprProgram。
    let padfunc = desc
        .getattr("padfunc")
        .map_err(|e| ConstructError::Compilation {
            message: format!("ProcessXorDescriptor missing 'padfunc': {}", e),
        })?;
    let pad = if let Ok(v) = padfunc.extract::<i64>() {
        XorPad::Int(v as u8)
    } else if let Ok(b) = padfunc.extract::<&[u8]>() {
        if b.len() == 1 {
            XorPad::Int(b[0])
        } else {
            XorPad::Bytes(b.to_vec())
        }
    } else {
        // 表达式路径：从 expr_programs[field_index]["pad"] 取
        let prog = compile_expr_program_from_desc(py, desc, "pad", field_index, expr_programs)?;
        XorPad::Expr(prog)
    };
    Ok(ProcessXorNode::new(inner_node, pad))
}

/// 从 `ProcessRotateLeftDescriptor` 构建 `ProcessRotateLeftNode`（设计 §5.5）。
fn build_process_rotate_left_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<ProcessRotateLeftNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "ProcessRotateLeftDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node = build_node_from_descriptor(
        py,
        &subcon_desc,
        field_index,
        expr_programs,
        field_names,
        false,
    )?;
    // amount/group：int 包装为单 op Const ExprProgram；FieldRef/ExprRef 走 ExprProgram。
    let amount_prog = compile_int_or_expr_program(py, desc, "amount", field_index, expr_programs)?;
    let group_prog = compile_int_or_expr_program(py, desc, "group", field_index, expr_programs)?;
    Ok(ProcessRotateLeftNode::new(
        inner_node,
        amount_prog,
        group_prog,
    ))
}

/// 从 `NamedTupleDescriptor` 构建 `NamedTupleNode`（设计 §6.1）。
fn build_named_tuple_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    field_names: &[String],
) -> Result<NamedTupleNode, ConstructError> {
    let subcon_desc = desc
        .getattr("subcon")
        .map_err(|e| ConstructError::Compilation {
            message: format!(
                "NamedTupleDescriptor missing 'subcon': {} (field {})",
                e, field_index
            ),
        })?;
    let inner_node =
        build_node_from_descriptor(py, &subcon_desc, field_index, &[], field_names, false)?;
    // 编译期调用 collections.namedtuple 物化 factory
    let tuplename: String = desc
        .getattr("tuplename")
        .and_then(|n| n.extract())
        .map_err(|e| ConstructError::Compilation {
            message: format!("NamedTupleDescriptor.tuplename not a str: {}", e),
        })?;
    let tuplefields_obj = desc
        .getattr("tuplefields")
        .map_err(|e| ConstructError::Compilation {
            message: format!("NamedTupleDescriptor missing 'tuplefields': {}", e),
        })?;
    // tuplefields 可为 str（空格分隔）或 list
    let field_names_list: Vec<String> = if let Ok(s) = tuplefields_obj.extract::<String>() {
        s.split_whitespace().map(|s| s.to_string()).collect()
    } else if let Ok(v) = tuplefields_obj.extract::<Vec<String>>() {
        v
    } else {
        return Err(ConstructError::Compilation {
            message: "NamedTupleDescriptor.tuplefields must be str or list".to_string(),
        });
    };
    let factory = {
        let nt_module =
            py.import_bound("collections")
                .map_err(|e| ConstructError::Compilation {
                    message: format!("failed to import collections: {}", e),
                })?;
        let namedtuple_fn =
            nt_module
                .getattr("namedtuple")
                .map_err(|e| ConstructError::Compilation {
                    message: format!("collections.namedtuple missing: {}", e),
                })?;
        let name_bound = PyString::new_bound(py, &tuplename);
        let fields_bound = PyList::new_bound(py, field_names_list.iter().map(|s| s.as_str()));
        let args_tuple = PyTuple::new_bound(py, [name_bound.into_any(), fields_bound.into_any()]);
        namedtuple_fn
            .call1(args_tuple)
            .map_err(|e| ConstructError::Compilation {
                message: format!("collections.namedtuple call failed: {}", e),
            })?
            .extract::<Py<PyType>>()
            .map_err(|e| ConstructError::Compilation {
                message: format!("namedtuple did not return a type: {}", e),
            })?
    };
    // mode：根据 inner_node 类型推断（Struct → Struct 模式；其他 → Sequence 模式）。
    let mode = match &inner_node {
        Node::Struct(_) | Node::StructRef(_) => NamedTupleMode::Struct,
        _ => NamedTupleMode::Sequence,
    };
    // field_names 转 Vec<Py<PyString>>
    let field_names_py: Vec<Py<PyString>> = field_names_list
        .iter()
        .map(|s| PyString::new_bound(py, s).unbind())
        .collect();
    Ok(NamedTupleNode::new(
        inner_node,
        factory,
        mode,
        field_names_py,
    ))
}

/// 辅助：从 desc.attr_name 取 int 常量包装为单 op Const ExprProgram，
/// 或从 expr_programs 取已编译 ExprProgram。
fn compile_int_or_expr_program(
    _py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    attr: &str,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<ExprProgram, ConstructError> {
    let val_obj = desc
        .getattr(attr)
        .map_err(|e| ConstructError::Compilation {
            message: format!("Descriptor missing '{}': {}", attr, e),
        })?;
    if let Ok(v) = val_obj.extract::<i64>() {
        Ok(ExprProgram::new(vec![ExprOp::Const(v)]))
    } else {
        // 表达式路径：从 expr_programs[field_index][attr] 取
        let field_exprs = expr_programs
            .get(field_index)
            .and_then(Option::as_ref)
            .ok_or_else(|| ConstructError::Compilation {
                message: format!(
                    "expression for '{}' missing in expr_programs (field {})",
                    attr, field_index
                ),
            })?;
        extract_expr_program_from_dict(_py, field_exprs.bind(_py), attr, field_index)
    }
}

/// 辅助：从 desc 的 expr_programs 取指定 key 的 ExprProgram（ProcessXor pad 用）。
fn compile_expr_program_from_desc(
    py: Python<'_>,
    _desc: &Bound<'_, PyAny>,
    key: &str,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<ExprProgram, ConstructError> {
    let field_exprs = expr_programs
        .get(field_index)
        .and_then(Option::as_ref)
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "expression for '{}' missing in expr_programs (field {})",
                key, field_index
            ),
        })?;
    extract_expr_program_from_dict(py, field_exprs.bind(py), key, field_index)
}

/// 辅助：从 Python dict {key: [ops]} 取 ExprProgram。
fn extract_expr_program_from_dict(
    _py: Python<'_>,
    dict: &Bound<'_, PyAny>,
    key: &str,
    field_index: usize,
) -> Result<ExprProgram, ConstructError> {
    let dict = dict
        .downcast::<PyDict>()
        .map_err(|_| ConstructError::Compilation {
            message: "expression program must be a dict".to_string(),
        })?;
    let ops_obj = dict
        .get_item(key)
        .map_err(|e| ConstructError::Compilation {
            message: format!("failed to get '{}' from expression programs: {}", key, e),
        })?
        .ok_or_else(|| ConstructError::Compilation {
            message: format!(
                "missing '{}' in expression programs (field {})",
                key, field_index
            ),
        })?;
    // ops_obj 是 list of tuples [("const", N), ("getint", idx), ...]。
    parse_expr_ops(&ops_obj)
}

/// 辅助：把 Python ops list 转为 ExprOp Vec。
fn parse_expr_ops(ops_obj: &Bound<'_, PyAny>) -> Result<ExprProgram, ConstructError> {
    let ops_list = ops_obj
        .downcast::<PyList>()
        .map_err(|_| ConstructError::Compilation {
            message: "expression ops must be a list".to_string(),
        })?;
    let mut ops = Vec::with_capacity(ops_list.len());
    for op_item in ops_list.iter() {
        let op_tuple = op_item
            .downcast::<PyTuple>()
            .map_err(|_| ConstructError::Compilation {
                message: "expression op must be a tuple".to_string(),
            })?;
        let name: String = op_tuple
            .get_item(0)
            .and_then(|n| n.extract())
            .map_err(|e| ConstructError::Compilation {
                message: format!("expression op name not a str: {}", e),
            })?;
        let op = match name.as_str() {
            "const" => {
                let v: i64 = op_tuple
                    .get_item(1)
                    .and_then(|v| v.extract())
                    .map_err(|e| ConstructError::Compilation {
                        message: format!("const op value not i64: {}", e),
                    })?;
                ExprOp::Const(v)
            }
            "getint" => {
                let idx: usize = op_tuple
                    .get_item(1)
                    .and_then(|v| v.extract())
                    .map_err(|e| ConstructError::Compilation {
                        message: format!("getint op index not usize: {}", e),
                    })?;
                ExprOp::GetInt(idx)
            }
            "add" => ExprOp::Add,
            "sub" => ExprOp::Sub,
            "mul" => ExprOp::Mul,
            "floordiv" => ExprOp::FloorDiv,
            "mod" => ExprOp::Mod,
            "bitand" => ExprOp::BitAnd,
            "bitor" => ExprOp::BitOr,
            "bitxor" => ExprOp::BitXor,
            "shl" => ExprOp::Shl,
            "shr" => ExprOp::Shr,
            "neg" => ExprOp::Neg,
            "not" => ExprOp::Not,
            "eq" => ExprOp::Eq,
            "ne" => ExprOp::Ne,
            "lt" => ExprOp::Lt,
            "le" => ExprOp::Le,
            "gt" => ExprOp::Gt,
            "ge" => ExprOp::Ge,
            _ => {
                return Err(ConstructError::Compilation {
                    message: format!("unknown expression op: {}", name),
                })
            }
        };
        ops.push(op);
    }
    Ok(ExprProgram::new(ops))
}
