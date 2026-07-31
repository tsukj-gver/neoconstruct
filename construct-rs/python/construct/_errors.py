"""construct-rs Python 异常层次。

设计依据：``docs/design/基础设施/架构设计.md`` §B.8。

所有异常携带 ``message`` 与可选的 ``path`` 属性，``str(e)`` 格式与 Python construct
2.10.70 对齐：path 非 None 时为 ``"Error in path {path}\\n{message}"``。

异常层次与 Rust 侧 ``ConstructError`` 枚举变体一一对应：

============================  ==========================================
Python 异常                    Rust ConstructError 变体
============================  ==========================================
:class:`ConstructError`       （基类，对应所有变体）
:class:`StreamError`          ``ConstructError::Stream``
:class:`FormatFieldError`     ``ConstructError::FormatField``
:class:`FieldLengthError`     ``ConstructError::FieldLength``
:class:`SizeofError`          （sizeof 调用失败时）
:class:`CompilationError`     ``ConstructError::Compilation``
:class:`UnresolvedReferenceError`  ``ConstructError::UnresolvedReference``
:class:`GenericConstructError``ConstructError::Generic``
:class:`IntegerError`         ``ConstructError::Integer`` (Phase 3.3)
:class:`PaddingError`         ``ConstructError::Padding`` (Phase 3.3)
:class:`RangeError`           ``ConstructError::Range`` (Phase 4)
:class:`RepeatError`          ``ConstructError::Repeat`` (Phase 4)
:class:`StopFieldError`       ``ConstructError::StopField`` (Phase 4)
:class:`IndexFieldError`      ``ConstructError::IndexField`` (Phase 4)
:class:`StringError`          ``ConstructError::String`` (Phase 6.2)
:class:`ExplicitError`        ``ConstructError::Explicit`` (Phase 7)
:class:`SelectError`          ``ConstructError::Select`` (Phase 7)
============================  ==========================================

.. note::

    Phase 1 的 Rust ``From<ConstructError> for PyErr`` 实现将所有变体映射为
    ``PyValueError``（携带含 path 的完整消息）。当后续更新 Rust 侧映射为各 Python
    异常子类后，用户可通过 ``except StreamError`` 等精确捕获。当前阶段用户可通过
    ``except ValueError`` 或 ``except Exception`` 捕获 Rust 抛出的错误，错误消息
    中已包含 path 信息。
"""


class ConstructError(Exception):
    """所有 construct-rs 错误的基类。

    :param message: 错误详情。
    :param path: 错误发生的路径（如 ``"root.header.flags"``），编译期错误为 ``None``。
    """

    def __init__(self, message: str = "", path: str = None):
        self.message = message
        self.path = path
        if path is not None:
            full = "Error in path {}\n{}".format(path, message)
        else:
            full = message
        super().__init__(full)

    def __str__(self):
        if self.path is not None:
            return "Error in path {}\n{}".format(self.path, self.message)
        return self.message


class StreamError(ConstructError):
    """流错误：字节不足、流读/写失败等。

    对应 Python construct 的 ``StreamError`` 和 Rust 的
    ``ConstructError::Stream``。
    """


class FormatFieldError(ConstructError):
    """格式字段错误：整数/浮点解析或构建失败。

    对应 Python construct 的 ``FormatFieldError`` 和 Rust 的
    ``ConstructError::FormatField``。
    """


class FieldLengthError(ConstructError):
    """字段长度错误：``Bytes(n)`` 的实际数据长度不匹配等。

    对应 Python construct 的 ``FieldError`` / ``StreamError``（长度不匹配场景）和
    Rust 的 ``ConstructError::FieldLength``。
    """


class SizeofError(ConstructError):
    """大小计算错误：字段大小无法在编译期或当前上下文确定。

    对应 Python construct 的 ``SizeofError``。
    """


class IntegerError(ConstructError):
    """整数错误：``BitsInteger`` 解析/构建失败（非整数类型、超出范围等）。

    对应 Python construct 的 ``IntegerError`` 和 Rust 的
    ``ConstructError::Integer``。

    Phase 3.3 新增：``BitsInteger`` 节点的运行时错误统一映射到此异常类，
    使用户可通过 ``except IntegerError`` 精确捕获（与 Python construct 一致）。
    """


class PaddingError(ConstructError):
    """填充错误：``Padding`` 字段的 pattern 非法（bit 域仅接受 0x00/0x01）等。

    对应 Python construct 的 ``PaddingError`` 和 Rust 的
    ``ConstructError::Padding``。

    Phase 3.3 新增：``BitPaddingNode`` 在编译期对非法 pattern 返回此错误
    （比 Python 在 ``bits2bytes`` 阶段延迟 ``KeyError`` 更早暴露）。
    """


class CompilationError(ConstructError):
    """编译错误：schema 编译失败（未知描述符、字段重复等）。

    对应 Rust 的 ``ConstructError::Compilation``。编译期错误不携带 ``path``。
    """


class UnresolvedReferenceError(ConstructError):
    """未解析的类型引用：前向引用未在运行时解析成功。

    对应 Rust 的 ``ConstructError::UnresolvedReference``。
    通常发生在互相引用的结构体中，被引用类尚未完成延迟编译时。
    """


class GenericConstructError(ConstructError):
    """通用错误：其他无法归类的运行时错误。

    对应 Rust 的 ``ConstructError::Generic``。
    """


class RangeError(ConstructError):
    """范围错误：Array count 无效（负数或与给定列表长度不符）。

    对应 Python construct 的 ``RangeError`` 和 Rust 的
    ``ConstructError::Range``。

    Phase 4 新增：Array 系列节点的 count 校验失败时触发
    （core.py L2528、L2541、L2543）。
    """


class RepeatError(ConstructError):
    """重复错误：RepeatUntil build 时无元素满足终止表达式。

    对应 Python construct 的 ``RepeatError`` 和 Rust 的
    ``ConstructError::Repeat``。

    Phase 4 新增：RepeatUntil build 遍历完列表无元素满足终止表达式时触发
    （core.py L2700）。
    """


class StopFieldError(ConstructError):
    """早停信号：StopIf 条件为真时抛出，被外层 Struct/GreedyRange 捕获。

    对应 Python construct 的 ``StopFieldError`` 和 Rust 的
    ``ConstructError::StopField``。

    Phase 4 新增：作为 Result 哨兵变体（非 panic），正常路径被
    GreedyRangeNode / StructNode 捕获视为正常终止（core.py L4106）。
    """


class IndexFieldError(ConstructError):
    """Index 字段错误：Index 节点读取 _index 时上下文未提供。

    对应 Python construct 的 ``IndexFieldError`` 和 Rust 的
    ``ConstructError::IndexField``。

    Phase 4 新增：当前 IndexNode 在 _index 缺失时返回 Py_None（不触发此错误），
    此异常类保留供未来严格模式使用。
    """


class StringError(ConstructError):
    """字符串错误：编解码失败、非 Unicode 输入、不支持的编码等。

    对应 Python construct 的 ``StringError``（core.py L54）和 Rust 的
    ``ConstructError::String``。

    Phase 6.2 新增：String 系列构造器（CString / GreedyString / PaddedString /
    PascalString）的编解码失败统一映射到此异常类，使用户可通过
    ``except StringError`` 精确捕获（与 Python construct 一致）。

    触发场景：

    - ``Encoding::decode``：非法字节序列（如无效 UTF-8 字节）
    - ``Encoding::encode``：输入非 ``str`` 类型（如 ``bytes``）
    - ASCII 编码遇到非 ASCII 字符（>= 128）
    - 编码名不合法（``Encoding::from_user_str`` 编译期失败，由 Descriptor 转为此异常）
    """


class ExplicitError(ConstructError):
    """显式错误：用户主动抛出（Select / Peek 不吞掉，直接向上传播）。

    对应 Python construct 的 ``ExplicitError``（core.py L89）和 Rust 的
    ``ConstructError::Explicit``。

    Phase 7 新增（PM 决策 2 / ADR-022 PE-3 收尾）。

    触发场景（Python construct 中）：

    - 用户在 Adapter 回调中 ``raise ExplicitError`` → Select / Peek 不吞掉
    - ``Error`` 构造器（core.py 未定义独立类，Phase 7+ 暂不实现）parse/build 时抛出

    construct-rs 已知差异 D1（设计 §1.5 / §12）：当前 Rust 侧
    ``From<PyErr> for ConstructError`` 统一转 ``Generic``，无法保留 Python 侧的
    ``ExplicitError`` 类型信息。Phase 7 范围内此异常类供 Rust 内部主动构造的
    ``ConstructError::Explicit`` 变体映射（Select / Peek 识别 Explicit 时传播），
    用户 Python 路径的完整 parity 待 Phase 8+ 修复 ``From<PyErr>`` 后达成。
    """


class SelectError(ConstructError):
    """Select 错误：所有 subcon 都未成功。

    对应 Python construct 的 ``SelectError``（core.py L109）和 Rust 的
    ``ConstructError::Select``。

    Phase 7 新增：Select 构造器遍历全部 subcons 后无成功者时触发。
    """


# ---------------------------------------------------------------------------
# Phase 8 P0 新增异常类（5 个）
# ---------------------------------------------------------------------------


class ConstError(ConstructError):
    """常量字段错误：parse 时 subcon 结果与期望值不符，或 build 时 obj 非 None/期望值。

    对应 Python construct 的 ``ConstError``（core.py L74）和 Rust 的
    ``ConstructError::Const``。

    Phase 8.1 新增：``Const`` 构造器的 parse/build 校验失败时触发
    （core.py L2832/L2867/L2872）。
    """


class CheckError(ConstructError):
    """断言检查错误：Check 节点的表达式求值为假。

    对应 Python construct 的 ``CheckError``（core.py L84）和 Rust 的
    ``ConstructError::Check``。

    Phase 8.1 新增：``Check`` 构造器的表达式求值为假时触发
    （core.py L3105/L3118/L3122）。
    """


class ChecksumError(ConstructError):
    """校验和错误：parse 时 hash 不匹配。

    对应 Python construct 的 ``ChecksumError``（core.py L144）和 Rust 的
    ``ConstructError::Checksum``。

    Phase 8.5 新增：``Checksum`` 构造器的 hash 比对失败时触发
    （core.py L5574）。
    """


class TerminatedError(ConstructError):
    """终止符错误：Terminated 节点 parse 时 stream 未到 EOF。

    对应 Python construct 的 ``TerminatedError``（core.py L129）和 Rust 的
    ``ConstructError::Terminated``。

    Phase 8.9 新增：``Terminated`` 构造器在 stream 仍有剩余字节时触发
    （core.py L4748）。
    """


class CancelParsing(ConstructError):
    """用户主动取消解析（仅能由用户代码显式 raise）。

    对应 Python construct 的 ``CancelParsing``（core.py L149）和 Rust 的
    ``ConstructError::CancelParsing``。

    Phase 8.10 新增（PM 决策 D-7）：

    - 用户在 Adapter ``_decode`` 内 ``raise CancelParsing()`` → parse 入口捕获，返回 None
    - 用户在 ``__post_init__`` 内 ``raise CancelParsing()`` → 同上
    - schema.rs ``_parse_raw`` 顶层 catch（对齐 Python core.py L416-419）

    构造-rs 已知差异（设计 §6.5 CP-3/CP-4）：

    - CancelParsing 在 Peek 内层抛出会被 Peek 吞掉返回 None（与 Python 同样行为）
    - CancelParsing 在 Select 内层抛出会被 Select 当作 subcon 失败继续尝试下一个

    construct-rs 表达式系统不接 lambda（ADR-006），用户无法在 Computed 表达式
    中 raise CancelParsing——**仅 Python 层用户代码可触发**。
    """


# ---------------------------------------------------------------------------
# Phase 8 P1+P2 新增异常类（5 个）
# ---------------------------------------------------------------------------


class MappingError(ConstructError):
    """映射错误：Enum/FlagsEnum/Mapping 的 label↔value 查找失败。

    对应 Python construct 的 ``MappingError``（core.py L102）和 Rust 的
    ``ConstructError::Mapping``。

    Phase 8.2 新增。触发场景：

    - ``Enum`` build 时 label 不在 encmapping
    - ``FlagsEnum`` build 时未知 label（str/dict 路径）
    - ``Mapping`` parse/build 时 key 不在映射（**包含 TypeError → MappingError 转换**，
      C-4：捕获自定义不可哈希 key 的 TypeError）
    """


class ValidationError(ConstructError):
    """校验错误：OneOf/NoneOf 的值集合校验失败，或 Validator 子类 _validate 返回 False。

    对应 Python construct 的 ``ValidationError``（core.py L125）和 Rust 的
    ``ConstructError::Validation``。

    Phase 8.3 新增。触发场景：

    - ``OneOf`` parse/build 时 obj ∉ valids（含 C-6 bool 边界：``True == 1`` 视为 int 1）
    - ``NoneOf`` parse/build 时 obj ∈ invalids
    - 用户继承 ``Validator`` 实现 ``_validate`` 返回 False
    """


class UnionError(ConstructError):
    """Union 错误：build 时无 subcon 匹配，或 parsefrom 解析失败。

    对应 Python construct 的 ``UnionError``（core.py L130）和 Rust 的
    ``ConstructError::Union``。

    Phase 8.6 新增。
    """


class RotationError(ConstructError):
    """旋转错误：ProcessRotateLeft 的 group/amount 参数非法或数据长度不对齐。

    对应 Python construct 的 ``RotationError``（core.py L138）和 Rust 的
    ``ConstructError::Rotation``。

    Phase 8.12 新增。触发场景：

    - ``group < 1``
    - ``len(data) % group != 0``
    """


class NamedTupleError(ConstructError):
    """NamedTuple 错误：inner 非 Struct/Sequence/Array/GreedyRange，或字段提取失败。

    对应 Python construct 的 ``NamedTupleError``（core.py L120）和 Rust 的
    ``ConstructError::NamedTuple``。

    Phase 8.11 新增。
    """


class TimestampError(ConstructError):
    """Timestamp 错误：参数类型错误（unit/epoch 非法）。

    对应 Python construct 的 ``TimestampError``（core.py L128）。

    Phase 8.11 新增。**仅 Python 层使用**（Timestamp macro 是 Python 实现，
    不进入 Rust ConstructError）。触发场景：

    - ``unit`` 非 int/float/str
    - ``epoch`` 非 int/Arrow/str
    """


# ---------------------------------------------------------------------------
# Phase 6.2: StringEncoded 兼容性别名（设计 §3.7.2 / §3.7.3）
#
# Python construct 原版中 ``StringEncoded(subcon, encoding)`` 是 Adapter 子类，
# 包装任意 bytes-producing Construct 并做 bytes↔str 转换。
#
# 在 PM 决策 1 方案 A 下，construct-rs 把编解码逻辑下沉到 CString / GreedyString /
# PaddedString / PascalString 内部，StringEncoded 失去存在意义。保留名字仅为
# 向后兼容：一调用即抛 ``StringError``，错误信息引导用户改用具体 String Node。
#
# **BREAKING CHANGE**（设计 §3.7.3）：从 Python construct 迁移的用户，如代码用
# ``StringEncoded(Bytes(4), "utf8")``，需改用 ``PaddedString(4, "utf8")`` 等。
# 自定义 bytes 来源 + decode 的场景，请走未来 Phase 6.3+ 的用户面 Adapter
# （继承 ``Adapter`` 写 ``_decode``/``_encode``）。
# ---------------------------------------------------------------------------

def StringEncoded(subcon=None, encoding=None):  # noqa: D401 (函数名故意大写对齐 Python construct)
    """兼容性别名（不推荐使用）—— 一调用即抛 :class:`StringError`。

    Phase 6.2 起，编解码逻辑已下沉到 :class:`CString` / :class:`GreedyString` /
    :class:`PaddedString` / :class:`PascalString` 内部。``StringEncoded`` 在
    Python construct 中是内部 Adapter（docstring 标注 "Used internally"），
    construct-rs 不再需要它。

    :param subcon: 任意 Construct 实例（Python construct 兼容参数，construct-rs 忽略）。
    :param encoding: 编码字符串（Python construct 兼容参数，construct-rs 忽略）。
    :raises StringError: 始终抛出，错误信息引导用户改用具体 String Node。

    迁移指引：

    - ``StringEncoded(Bytes(4), "utf8")`` → ``PaddedString(4, "utf8")``
    - ``StringEncoded(GreedyBytes, "utf8")`` → ``GreedyString("utf8")``
    - 自定义 bytes 来源 + decode → 继承 ``Adapter`` 写 ``_decode``/``_encode``
      （Phase 6.3+ 用户面 Adapter）。
    """
    raise StringError(
        "StringEncoded is deprecated in construct-rs; use CString / GreedyString / "
        "PaddedString / PascalString directly. For custom bytes->str adapters, "
        "inherit from Adapter (Phase 6.3)."
    )


__all__ = [
    "ConstructError",
    "StreamError",
    "FormatFieldError",
    "FieldLengthError",
    "SizeofError",
    "CompilationError",
    "UnresolvedReferenceError",
    "GenericConstructError",
    "IntegerError",
    "PaddingError",
    "RangeError",
    "RepeatError",
    "StopFieldError",
    "IndexFieldError",
    "StringError",
    "ExplicitError",
    "SelectError",
    # Phase 8 P0 新增。
    "ConstError",
    "CheckError",
    "ChecksumError",
    "TerminatedError",
    "CancelParsing",
    # Phase 8 P1+P2 新增。
    "MappingError",
    "ValidationError",
    "UnionError",
    "RotationError",
    "NamedTupleError",
    "TimestampError",
]
