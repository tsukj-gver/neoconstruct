"""StructMixin 基类、``field()``/``rfield()``/``wfield()`` 声明函数、
表达式类型系统（FieldRef/ExprRef）与 Schema 编译管线（纯 Python 实现）。

设计依据：
- ``docs/架构设计.md`` §A.2（StructMixin）、§A.3（field）、§A.6（空类）、
  §E.1-E.6（编译管线）
- ``docs/模块设计-表达式系统.md`` §2.1（三种 field 函数）、§2.2（default 与 kw_only）、
  §2.3（FieldRef/ExprRef 类型系统）、§2.4（context= 跨层引用）

核心流程：

1. 用户定义 ``@dataclass class X(StructMixin): ...``
2. Python 创建类对象后立即调用 ``StructMixin.__init_subclass__(X)``
3. ``__init_subclass__`` 从类体收集 ``field()``/``rfield()``/``wfield()`` 声明，
   调用 Rust ``compile_schema``
4. 编译产物存为 ``X._construct_compiled``（类属性，零开销查找）
5. ``_apply_dataclass_field_config`` 将 ``_FieldDescriptor`` 替换为
   ``dataclasses.field()`` 配置（RO→init=False，有 default→kw_only=True）
6. ``@dataclass`` 装饰器随后执行（生成 ``__init__`` 等）

关键约束（§A.5.1）：``__init_subclass__`` 在 ``@dataclass`` **之前**执行，因此不能
依赖 ``__dataclass_fields__``（尚不存在），从类体原始属性提取字段信息。
"""

import dataclasses
import operator
import threading

from ._errors import ConstructError

# ---------------------------------------------------------------------------
# Rust 扩展导入
# ---------------------------------------------------------------------------

# 延迟导入 Rust 扩展的 compile_schema 函数。
# 若扩展未构建（如纯 Python 开发），_compile_schema 为 None，在实际编译时抛出
# 明确错误而非导入时失败。
try:
    from ._construct_rust import compile_schema as _compile_schema
except ImportError:  # pragma: no cover - 仅在扩展未构建时触发
    _compile_schema = None


# ---------------------------------------------------------------------------
# 哨兵值（§2.1.1）
# ---------------------------------------------------------------------------

_MISSING = object()
"""哨兵值：表示 field() 未指定 ``default`` 参数。

用于区分 ``field(Int8ub)``（无默认值）与 ``field(Int8ub, default=None)``
（默认值为 None）。采用全局单例 object() 保证 identity 唯一性。
"""


# ---------------------------------------------------------------------------
# 表达式类型系统：_ExprMixin / _FieldDescriptor / _ExprRef（§2.3）
# ---------------------------------------------------------------------------


class _ExprMixin:
    """FieldRef/ExprRef 共享的算术运算符重载（§2.3.3）。

    ``_FieldDescriptor`` 和 ``_ExprRef`` 都继承此类，使字段引用和表达式节点
    都能通过 Python 运算符构建表达式树。每次二元运算返回新的 ``_ExprRef`` 节点。

    运算符支持矩阵（映射到 ExprOp）：

    ============  ======  ===========================================
    分类           运算符   ExprOp
    ============  ======  ===========================================
    算术           + - *   Add / Sub / Mul
    算术           // %    FloorDiv / Mod
    位运算         &       BitAnd
    位运算         |       BitOr
    位运算         ^       BitXor
    位运算         << >>   Shl / Shr
    一元           - ~     Neg / Not
    比较           == !=   Eq / Ne
    比较           < <=    Lt / Le
    比较           > >=    Gt / Ge
    ============  ======  ===========================================

    **不支持** ``__truediv__``（``/``）：结果可能为浮点，VM 栈为 i64。
    **不支持** ``and``/``or``：Python 短路求值无法重载。
    """

    __slots__ = ()

    # --- 二元算术运算 ---
    def __add__(self, other):
        return _ExprRef(operator.add, self, other)

    def __radd__(self, other):
        return _ExprRef(operator.add, other, self)

    def __sub__(self, other):
        return _ExprRef(operator.sub, self, other)

    def __rsub__(self, other):
        return _ExprRef(operator.sub, other, self)

    def __mul__(self, other):
        return _ExprRef(operator.mul, self, other)

    def __rmul__(self, other):
        return _ExprRef(operator.mul, other, self)

    def __floordiv__(self, other):
        return _ExprRef(operator.floordiv, self, other)

    def __rfloordiv__(self, other):
        return _ExprRef(operator.floordiv, other, self)

    def __mod__(self, other):
        return _ExprRef(operator.mod, self, other)

    def __rmod__(self, other):
        return _ExprRef(operator.mod, other, self)

    # --- 位运算 ---
    def __and__(self, other):
        return _ExprRef(operator.and_, self, other)

    def __or__(self, other):
        return _ExprRef(operator.or_, self, other)

    def __xor__(self, other):
        return _ExprRef(operator.xor, self, other)

    def __lshift__(self, other):
        return _ExprRef(operator.lshift, self, other)

    def __rshift__(self, other):
        return _ExprRef(operator.rshift, self, other)

    # --- 一元运算 ---
    def __neg__(self):
        return _ExprRef(operator.neg, self, None)

    def __pos__(self):
        return self  # 正号无操作

    def __invert__(self):
        return _ExprRef(operator.invert, self, None)

    # --- 比较运算（返回 _ExprRef，编译为 Eq/Lt/Gt 等，VM 返回 0/1） ---
    def __eq__(self, other):
        return _ExprRef(operator.eq, self, other)

    def __ne__(self, other):
        return _ExprRef(operator.ne, self, other)

    def __lt__(self, other):
        return _ExprRef(operator.lt, self, other)

    def __le__(self, other):
        return _ExprRef(operator.le, self, other)

    def __gt__(self, other):
        return _ExprRef(operator.gt, self, other)

    def __ge__(self, other):
        return _ExprRef(operator.ge, self, other)

    def __class_getitem__(cls, item):
        """支持 ``FieldRef[int]`` / ``ExprRef[int]`` 语法（T 仅用于 IDE 提示）。"""
        return cls

    __hash__ = None  # 表达式不可哈希（与 Python construct 一致）


class _FieldDescriptor(_ExprMixin):
    """``field()`` / ``rfield()`` / ``wfield()`` 的返回值。

    同时满足三套协议：

    - **@dataclass 字段协议**：作为字段的默认值。``@dataclass`` 将其视为普通默认值，
      用户构造实例时传入真正的值（如 ``MyMsg(address=1)``），默认值不参与热路径。
    - **Mixin 编译协议**：``__init_subclass__`` 从类体提取 ``_FieldDescriptor`` 实例，
      读取其 ``subcon`` / ``mode`` / ``default`` 属性配置编译。
    - **FieldRef 协议**：可被其他字段的表达式引用（如 ``Bytes(count)`` 中的 ``count``）。
      通过对象 identity（``id()``）唯一标识，编译期建立 ``id→field_index`` 映射。
      该对象支持算术运算重载（``__add__`` 等），返回 ``_ExprRef``。
    """

    __slots__ = ("subcon", "mode", "default", "name", "_context_injections")

    def __init__(self, subcon, *, mode="rw", default=_MISSING, context=None):
        """初始化字段描述符。

        :param subcon: 类型描述符对象（``Int8ub``、``Bytes(n)``、``GreedyBytes``、
                        或另一个 ``StructMixin`` 子类）。
        :param mode: 字段模式，``"rw"``（读写，默认）/ ``"ro"``（只读）/ ``"wo"``（只写）。
        :param default: 可选默认值。``_MISSING`` 表示未指定（默认无默认值）。
        :param context: 可选，跨层引用的 context 注入映射（决策 4 的 ``context=`` 参数）。
        """
        self.subcon = subcon
        self.mode = mode
        self.default = default
        self.name = None  # __set_name__ 填充（用于错误信息）
        self._context_injections = context

    def __set_name__(self, owner, name):
        """Python 3.6+ 描述符协议：类创建时自动调用，填充字段名。"""
        self.name = name

    def __repr__(self):
        return "_FieldDescriptor(name={!r}, mode={!r}, subcon={!r})".format(
            self.name, self.mode, self.subcon
        )


class _ExprRef(_ExprMixin):
    """表达式节点，编译期翻译为 ExprOp 指令序列（§2.3.3）。

    ``_ExprRef`` 形成一棵二叉表达式树：

    - ``lhs`` / ``rhs`` 可以是 ``_FieldDescriptor``、``_ExprRef`` 或常量（int）
    - ``op`` 是 ``operator`` 模块的函数（``operator.add`` 等）
    - 一元运算时 ``rhs=None``

    ``_ExprRef`` 本身也重载算术运算，支持无限嵌套。
    通过对象 identity 与 ``_FieldDescriptor`` 区分。
    """

    __slots__ = ("op", "lhs", "rhs")

    def __init__(self, op, lhs, rhs):
        """初始化表达式节点。

        :param op: ``operator`` 模块的函数（``operator.add`` 等）。
        :param lhs: 左操作数（``_FieldDescriptor`` / ``_ExprRef`` / int）。
        :param rhs: 右操作数（同 lhs），一元运算时为 ``None``。
        """
        self.op = op
        self.lhs = lhs
        self.rhs = rhs

    def __repr__(self):
        return "_ExprRef(op={!r})".format(getattr(self.op, "__name__", self.op))


# ---------------------------------------------------------------------------
# field() / rfield() / wfield() 声明函数（§2.1.2）
# ---------------------------------------------------------------------------


def field(subcon, *, default=_MISSING, context=None):
    """声明一个 RW 字段（build 要填，parse 能读）。

    用法::

        @dataclass
        class MyMsg(StructMixin):
            address: int = field(Int8ub)
            data: bytes = field(Bytes(4))
            version: int = field(Int8ub, default=1)  # 有 default → kw_only=True

    :param subcon: 类型描述符，决定该字段的二进制格式。必须是框架已知的描述符类型
        （``Int8ub`` 等 16 种单例、``Bytes(n)``、``GreedyBytes``、或另一个
        ``StructMixin`` 子类）。
    :param default: 可选默认值。指定后自动设为 ``kw_only=True``（决策 6）。
    :param context: 可选，跨层引用的 context 注入映射（决策 4）。
    :return: ``_FieldDescriptor(mode="rw")`` 对象，作为类属性默认值。

    字段顺序由 Python 3.7+ ``__annotations__`` 的插入顺序保证（PEP 526）。
    """
    return _FieldDescriptor(subcon, mode="rw", default=default, context=context)


def rfield(subcon, *, context=None):
    """声明一个 RO 字段（build 不用填，parse 能读）。

    RO 字段在 build 时自动计算（不从实例取值）：
    - ``Tell()``：build 时取流位置
    - ``Computed(expr)``：build 时求值表达式
    - ``Const(subcon, value)``：build 时用常量值

    RO 字段的 ``init=False``，不出现在 ``__init__`` 参数中。

    :param subcon: 类型描述符。
    :param context: 可选，跨层引用的 context 注入映射。
    :return: ``_FieldDescriptor(mode="ro")``。
    """
    return _FieldDescriptor(subcon, mode="ro", context=context)


def wfield(subcon, *, default=_MISSING, context=None):
    """声明一个 WO 字段（build 要填，parse 读不到）。

    parse 时正常解析（消费字节），但结果不存入实例属性（丢弃）。
    典型场景：padding、reserved 字段。

    :param subcon: 类型描述符。
    :param default: 可选默认值。指定后自动设为 ``kw_only=True``。
    :param context: 可选，跨层引用的 context 注入映射。
    :return: ``_FieldDescriptor(mode="wo")``。
    """
    return _FieldDescriptor(subcon, mode="wo", default=default, context=context)


# ---------------------------------------------------------------------------
# 字段信息收集（§E.2）
# ---------------------------------------------------------------------------


def _collect_field_descriptors(cls):
    """从类体收集字段信息。

    遍历 ``cls.__annotations__``（Python 3.7+ 保序），提取其中 ``_FieldDescriptor``
    类型的默认值。不依赖 ``__dataclass_fields__``（此刻尚未生成）。

    :param cls: 刚创建的 StructMixin 子类。
    :return: ``[(field_name, descriptor), ...]`` 有序列表，顺序与字段声明顺序一致。
             每项的 descriptor 是完整的 ``_FieldDescriptor`` 对象（含 mode、default 等）。
    """
    annotations = getattr(cls, "__annotations__", {})
    fields = []
    for name in annotations:
        # 仅检查 cls 自身的 __dict__（不含继承的属性）
        default = cls.__dict__.get(name)
        if isinstance(default, _FieldDescriptor):
            fields.append((name, default))
    return fields


# ---------------------------------------------------------------------------
# dataclass 字段配置注入（§2.2.1）
# ---------------------------------------------------------------------------


def _apply_dataclass_field_config(cls, descriptors):
    """将有 default 的 ``_FieldDescriptor`` 转换为 ``dataclasses.field()`` 配置。

    在 ``__init_subclass__`` 末尾调用（编译完成后，``@dataclass`` 执行前）。
    将类属性上的 ``_FieldDescriptor`` 替换为 ``dataclasses.field()`` 返回值，
    使 ``@dataclass`` 能正确配置 ``init`` / ``default`` / ``kw_only``。

    处理规则：
    - ``mode="ro"`` → ``dataclasses.field(init=False, default=None)``
      （RO 字段不在 ``__init__`` 中，parse 时由 force_setattr 覆盖）
    - ``mode="rw"`` 或 ``"wo"`` 且有 ``default`` →
      ``dataclasses.field(kw_only=True, default=desc.default)``
    - ``mode="rw"`` 或 ``"wo"`` 无 ``default`` → ``dataclasses.field()``
      （必填 positional 参数，无默认值）

    [设计质疑] 设计文档 §2.2.1 原文为"RW/WO 无 default → 保持 _FieldDescriptor
    不变（Phase 1 行为）"。但 Phase 2 为 ``_FieldDescriptor`` 添加了 ``__eq__``
    运算符重载（表达式系统），导致 ``__hash__ = None``。Python 3.x 的
    ``@dataclass`` 将 ``__hash__ is None`` 的对象视为 mutable default 并拒绝。
    因此所有 ``_FieldDescriptor`` 必须替换为 ``dataclasses.field()``。
    对于无默认值的 RW/WO 字段，使用无参 ``dataclasses.field()`` 使其成为
    必填 positional 参数——行为上等价（用户始终提供值）。

    :param cls: StructMixin 子类。
    :param descriptors: ``[(field_name, _FieldDescriptor), ...]`` 有序列表。
    """
    for name, desc in descriptors:
        if desc.mode == "ro":
            # RO 字段：init=False，用 None 占位（parse 时由 force_setattr 覆盖）
            setattr(cls, name, dataclasses.field(init=False, default=None))
        elif desc.default is not _MISSING:
            # RW/WO 字段且有 default：kw_only=True + default
            setattr(
                cls,
                name,
                dataclasses.field(kw_only=True, default=desc.default),
            )
        else:
            # RW/WO 无 default → dataclasses.field() 无默认值（必填 positional）
            setattr(cls, name, dataclasses.field())


# ---------------------------------------------------------------------------
# Schema 编译（§E.1, §E.3）
# ---------------------------------------------------------------------------


def _compile_schema_for_class(cls):
    """为 StructMixin 子类编译 schema。

    在 ``__init_subclass__`` 中调用。收集字段 → 调用 Rust ``compile_schema`` →
    注入 dataclass 配置 → 存储编译产物。前向引用未解析时安装延迟桩（§E.6），
    其他错误 fail fast。

    :param cls: 刚创建的 StructMixin 子类。
    """
    descriptors = _collect_field_descriptors(cls)
    field_names = [name for name, _ in descriptors]
    subcons = [desc.subcon for _, desc in descriptors]

    if _compile_schema is None:
        raise ConstructError(
            "construct-rs Rust 扩展不可用，无法编译 schema。"
            "请确保已通过 maturin develop 安装。"
        )

    try:
        cls._construct_compiled = _compile_schema(cls, field_names, subcons)
    except Exception as e:
        # 检查是否为前向引用未解析（UnresolvedReference）。
        # Rust 侧该错误消息含 "unresolved" 或 "未解析"。
        err_str = str(e)
        if "unresolved" in err_str.lower() or "未解析" in err_str:
            # SF-1 修复：在安装延迟桩前注入 dataclass 字段配置。
            # Phase 2 的 _FieldDescriptor 有 __hash__=None（表达式系统），
            # 若不替换为 dataclasses.field()，@dataclass 会拒绝。
            _apply_dataclass_field_config(cls, descriptors)
            _install_lazy_stubs(cls)
        else:
            raise

    # 编译成功：注入 dataclass 字段配置（在编译后、@dataclass 执行前）
    _apply_dataclass_field_config(cls, descriptors)


# ---------------------------------------------------------------------------
# 延迟桩机制（§E.6）
# ---------------------------------------------------------------------------

# 每个延迟桩类对应的编译锁，确保多线程下编译只发生一次。
# 使用 cls -> Lock 字典（cls 是 hashable）。
_lazy_compile_locks: "dict" = {}


def _install_lazy_stubs(cls):
    """安装延迟编译桩。

    当编译因前向引用失败时，将 ``parse``/``build`` 替换为桩方法。首次调用时
    在锁保护下重试编译，成功后删除桩（回退到 ``StructMixin`` 继承的方法）。

    线程安全（§E.6）：``threading.Lock`` + 双重检查确保编译只发生一次。

    :param cls: 需要安装延迟桩的 StructMixin 子类。
    """
    cls._construct_compiled = None
    lock = threading.Lock()
    _lazy_compile_locks[cls] = lock

    def _retry_compile():
        """在锁保护下重试编译。已编译则直接返回。"""
        if cls._construct_compiled is not None:
            return
        with lock:
            if cls._construct_compiled is not None:
                return
            descriptors = _collect_field_descriptors(cls)
            field_names = [name for name, _ in descriptors]
            subcons = [desc.subcon for _, desc in descriptors]
            schema = _compile_schema(cls, field_names, subcons)
            # 编译成功：注入 dataclass 配置，存储产物，删除桩
            _apply_dataclass_field_config(cls, descriptors)
            cls._construct_compiled = schema
            _remove_lazy_stubs(cls)

    @classmethod
    def _lazy_parse(cls_, data):
        """延迟 parse 桩：首次调用时重试编译，然后执行 parse。"""
        _retry_compile()
        schema = cls_._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译（延迟编译失败）".format(cls_.__name__)
            )
        # 方案 B'：Rust 内部直接构造实例（与 StructMixin.parse 一致）
        return schema._parse_raw(data)

    def _lazy_build(self):
        """延迟 build 桩：首次调用时重试编译，然后执行 build。"""
        _retry_compile()
        schema = type(self)._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译（延迟编译失败）".format(type(self).__name__)
            )
        return schema._build_raw(self)

    cls.parse = _lazy_parse
    cls.build = _lazy_build


def _remove_lazy_stubs(cls):
    """删除延迟桩方法，使 cls 回退到 StructMixin 继承的 parse/build。

    编译成功后调用，后续 parse/build 调用直接走 StructMixin 的方法（零桩开销）。
    """
    for method_name in ("parse", "build"):
        try:
            delattr(cls, method_name)
        except AttributeError:
            pass


# ---------------------------------------------------------------------------
# StructMixin 基类（§A.2）
# ---------------------------------------------------------------------------


class StructMixin:
    """所有用户二进制结构类的基类。

    子类需配合 ``@dataclass`` 装饰器使用。类创建时自动触发 Schema 编译，
    编译产物存于类属性 ``_construct_compiled``。

    用法::

        from dataclasses import dataclass
        from construct import StructMixin, field, rfield, Int8ub, GreedyBytes

        @dataclass
        class MyMsg(StructMixin):
            address: int = field(Int8ub)
            data: bytes = field(GreedyBytes)

        msg = MyMsg(address=1, data=b'\\x00\\x01')
        built = msg.build()
        parsed = MyMsg.parse(built)

    编译流程在 ``__init_subclass__`` 中完成（§E.1），用户无感。``@dataclass``
    在 ``__init_subclass__`` 之后执行（§A.5.1），两者通过 ``field()`` 返回值
    协调（同时满足 dataclass 默认值协议与 Mixin 编译协议）。
    """

    # 编译产物缓存槽（由 __init_subclass__ 设置）。None 表示尚未编译
    # （可能是延迟桩阶段，或空基类）。
    _construct_compiled = None

    def __init_subclass__(cls, **kwargs):
        """子类创建时自动编译 schema（§E.1）。

        此方法在 ``@dataclass`` 装饰器之前执行（Python 类创建顺序）。
        编译流程：收集字段 → 调用 Rust compile_schema → 注入 dataclass 配置 →
        存储产物。
        """
        super().__init_subclass__(**kwargs)
        _compile_schema_for_class(cls)

    @classmethod
    def parse(cls, data):
        """从字节解析为本类的实例。

        恰好一次 FFI 穿越：调用 ``CompiledSchema._parse_raw`` 直接返回本类实例
        （方案 B'：Rust 内部通过 create_class + force_setattr 构造实例，
        无 Python 侧 ``cls(**dict)`` kwargs unpacking 开销）。

        :param data: 待解析的字节数据。
        :return: 本类的实例，字段值从 ``data`` 中解析得到。
        :raises ConstructError: 编译未完成或解析失败（字节不足、格式错误等）。
        """
        schema = cls._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译（可能存在未解析的前向引用），"
                "请确保所有被引用的类型已定义后再调用 parse".format(cls.__name__)
            )
        # 方案 B'：Rust 内部直接构造实例，不再 cls(**raw_fields)
        return schema._parse_raw(data)

    def build(self):
        """从本实例构造字节。

        恰好一次 FFI 穿越：Rust 内部遍历执行树，通过 C API 读取本实例属性，
        直接写入输出缓冲区。

        :return: 构建的字节数据。
        :raises ConstructError: 编译未完成或构建失败（属性缺失、类型错误等）。
        """
        schema = type(self)._construct_compiled
        if schema is None:
            raise ConstructError(
                "{} 尚未完成编译".format(type(self).__name__)
            )
        return schema._build_raw(self)


__all__ = ["StructMixin", "field", "rfield", "wfield"]
