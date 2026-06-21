"""StructMixin 基类、``field()`` 声明函数与 Schema 编译管线（纯 Python 实现）。

设计依据：``docs/架构设计.md`` §A.2（StructMixin）、§A.3（field）、§A.6（空类）、
§E.1-E.6（编译管线）。

核心流程：

1. 用户定义 ``@dataclass class X(StructMixin): ...``
2. Python 创建类对象后立即调用 ``StructMixin.__init_subclass__(X)``
3. ``__init_subclass__`` 从类体收集 ``field()`` 声明，调用 Rust ``compile_schema``
4. 编译产物存为 ``X._construct_compiled``（类属性，零开销查找）
5. ``@dataclass`` 装饰器随后执行（生成 ``__init__`` 等）

关键约束（§A.5.1）：``__init_subclass__`` 在 ``@dataclass`` **之前**执行，因此不能
依赖 ``__dataclass_fields__``（尚不存在），从类体原始属性提取字段信息。
"""

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
# field() 声明函数与 _FieldDescriptor
# ---------------------------------------------------------------------------


class _FieldDescriptor:
    """``field()`` 的返回值，存储 ``subcon`` 引用。

    同时满足两套协议（§A.3）：

    - **@dataclass 字段协议**：作为字段的默认值。``@dataclass`` 将其视为普通默认值，
      用户构造实例时传入真正的值（如 ``MyMsg(address=1)``），默认值不参与热路径。
    - **Mixin 编译协议**：``__init_subclass__`` 从类体提取 ``_FieldDescriptor`` 实例，
      读取其 ``subcon`` 属性获取类型描述符（如 ``Int8ub``）。

    该对象不是 Python ``dataclasses.Field``，也不需要是——``@dataclass`` 只需要
    一个合法的默认值即可。
    """

    __slots__ = ("subcon",)

    def __init__(self, subcon):
        """初始化字段描述符。

        :param subcon: 类型描述符对象（``Int8ub``、``Bytes(n)``、``GreedyBytes``、
                        或另一个 ``StructMixin`` 子类）。
        """
        self.subcon = subcon

    def __repr__(self):
        return "_FieldDescriptor(subcon={!r})".format(self.subcon)


def field(subcon):
    """声明一个 ``StructMixin`` 字段。

    用法::

        @dataclass
        class MyMsg(StructMixin):
            address: int = field(Int8ub)
            data: bytes = field(GreedyBytes)

    :param subcon: 类型描述符，决定该字段的二进制格式。必须是框架已知的描述符类型
        （``Int8ub`` 等 16 种单例、``Bytes(n)``、``GreedyBytes``、或另一个
        ``StructMixin`` 子类）。
    :return: ``_FieldDescriptor`` 对象，作为类属性默认值。同时满足 ``@dataclass``
        字段协议与 Mixin 编译协议（§A.3）。

    字段顺序由 Python 3.7+ ``__annotations__`` 的插入顺序保证（PEP 526）。
    """
    return _FieldDescriptor(subcon)


# ---------------------------------------------------------------------------
# 字段信息收集（§E.2）
# ---------------------------------------------------------------------------


def _collect_field_descriptors(cls):
    """从类体收集字段信息。

    遍历 ``cls.__annotations__``（Python 3.7+ 保序），提取其中 ``_FieldDescriptor``
    类型的默认值。不依赖 ``__dataclass_fields__``（此刻尚未生成）。

    :param cls: 刚创建的 StructMixin 子类。
    :return: ``[(field_name, subcon), ...]`` 有序列表，顺序与字段声明顺序一致。
    """
    annotations = getattr(cls, "__annotations__", {})
    fields = []
    for name in annotations:
        # 仅检查 cls 自身的 __dict__（不含继承的属性）
        default = cls.__dict__.get(name)
        if isinstance(default, _FieldDescriptor):
            fields.append((name, default.subcon))
    return fields


# ---------------------------------------------------------------------------
# Schema 编译（§E.1, §E.3）
# ---------------------------------------------------------------------------


def _compile_schema_for_class(cls):
    """为 StructMixin 子类编译 schema。

    在 ``__init_subclass__`` 中调用。收集字段 → 调用 Rust ``compile_schema`` →
    存储编译产物。前向引用未解析时安装延迟桩（§E.6），其他错误 fail fast。

    :param cls: 刚创建的 StructMixin 子类。
    """
    fields = _collect_field_descriptors(cls)
    field_names = [name for name, _ in fields]
    descriptors = [desc for _, desc in fields]

    if _compile_schema is None:
        raise ConstructError(
            "construct-rs Rust 扩展不可用，无法编译 schema。"
            "请确保已通过 maturin develop 安装。"
        )

    try:
        cls._construct_compiled = _compile_schema(cls, field_names, descriptors)
    except Exception as e:
        # 检查是否为前向引用未解析（UnresolvedReference）。
        # Rust 侧该错误消息含 "unresolved" 或 "未解析"。
        # 注：当前 Rust compile_schema 对 StructMixin 子类引用直接构建 StructRef
        # 节点（不抛 UnresolvedReference），此分支为未来兼容预留。
        err_str = str(e)
        if "unresolved" in err_str.lower() or "未解析" in err_str:
            _install_lazy_stubs(cls)
        else:
            raise


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
            fields = _collect_field_descriptors(cls)
            field_names = [name for name, _ in fields]
            descriptors = [desc for _, desc in fields]
            schema = _compile_schema(cls, field_names, descriptors)
            cls._construct_compiled = schema
            # 编译成功：删除桩方法，回退到 StructMixin.parse/build（继承）
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
        raw_fields = schema._parse_raw(data)
        return cls_(**raw_fields)

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
        from construct import StructMixin, field, Int8ub, GreedyBytes

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
        编译流程：收集字段 → 调用 Rust compile_schema → 存储产物。
        """
        super().__init_subclass__(**kwargs)
        _compile_schema_for_class(cls)

    @classmethod
    def parse(cls, data):
        """从字节解析为本类的实例。

        恰好一次 FFI 穿越：调用 ``CompiledSchema._parse_raw`` 返回字段 dict，
        随后在本类 ``__init__`` 中构造实例（Python 解释器内部，无额外 FFI）。

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
        raw_fields = schema._parse_raw(data)
        return cls(**raw_fields)

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


__all__ = ["StructMixin", "field"]
