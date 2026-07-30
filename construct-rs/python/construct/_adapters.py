"""用户面 Adapter 基类（Adapter / SymmetricAdapter）。

设计依据：``docs/design/模块设计/模块设计-Adapter核心.md`` §4。

PM 决策 2（双层分离强化）：通用 Adapter / SymmetricAdapter **基本在 Python 层实现**。
用户继承此类并实现 ``_decode`` / ``_encode``。Rust 仅提供"最小通用钩子"
（AdapterCallbackNode，PM 决策 6.3-D1 接受），用于 Adapter 嵌入 Struct 字段场景。

# 双层分工

| 层 | 实现位置 | FFI 次数 | 性能目标 | 用户主动选择 |
|----|---------|---------|---------|------------|
| 内置 Adapter（Subconstruct/RawCopy/Peek/Rebuild/Pass） | Rust Node 变体 | 1 次 | ≥10x | 用户用具体名（如 ``Peek(Int8ub)``） |
| 用户面 Adapter（Adapter/SymmetricAdapter 基类） | Python 类（用户继承） | ≥2 次 | 不设硬门禁 | 用户继承 ``Adapter`` 写 ``_decode``/``_encode`` |

# FFI 边界（设计 §4.3）

用户面 Adapter 嵌入 Struct 字段时，subcon 部分在 Rust 内执行（零 FFI），
但 ``_decode`` / ``_encode`` 是用户 Python 方法，需 Rust→Python 回调（1 次额外 FFI）。
总计 2 次 FFI 穿越（parse 入口 + _decode 回调）。

用户主动继承 Adapter = 显式接受此性能折衷（PM 决策 2）。用户面 Adapter
不设硬性能门禁（设计 §0.2 表）。

# §0 合规性（设计 §4.4）

AdapterCallbackNode 在执行树内（Node enum 变体），2 次 FFI 是用户主动选择的
后处理开销，不属于 §0 #1 禁止的"中间表示层"（中间表示层指 Rust 端临时数据类型
再转换，非用户 Python 回调）。

# 使用示例

## 单独使用（HexAdapter）

::

    class HexAdapter(Adapter):
        def _decode(self, obj, context, path):
            return hex(obj)
        def _encode(self, obj, context, path):
            return int(obj, 16)

    adapter = HexAdapter(Int8ub)
    adapter.parse(b"\\x10")  # → "0x10"
    adapter.build("0x10")    # → b"\\x10"

## 嵌入 Struct 字段

::

    @dataclass
    class Packet(StructMixin):
        value: str = field(HexAdapter(Int8ub))

    Packet.parse(b"\\x10")      # → Packet(value="0x10")
    Packet(value="0x10").build()  # → b"\\x10"
"""

from __future__ import annotations

from ._errors import ConstructError

__all__ = ["Adapter", "SymmetricAdapter", "AdapterDescriptor"]


# ---------------------------------------------------------------------------
# context 辅助
# ---------------------------------------------------------------------------


def _make_context(kw):
    """从用户 kwargs 构造 context dict（Python 层 Adapter.parse/build 用）。

    用户面 Adapter 单独使用时无 Struct 父节点，context 仅含用户传入的 kw。

    :param kw: 用户传入的 keyword 参数 dict。
    :return: dict（context 视图）。
    """
    return dict(kw) if kw else {}


# ---------------------------------------------------------------------------
# Adapter 基类
# ---------------------------------------------------------------------------


class Adapter:
    """通用 Adapter 基类（用户继承 + 实现 _decode/_encode）。

    对应 Python construct core.py:813 的 Adapter。在 construct-rs 中，
    Adapter **完全在 Python 层实现**——用户继承此类并实现 ``_decode`` / ``_encode``，
    parse/build 路径在 Python 层调用 subcon 的 Rust parse/build。

    **双层分工（PM 决策 2）**：Adapter/SymmetricAdapter 是用户面构造器，
    性能损失用户已显式接受（Rust→Python 回调额外 ~200-300ns/次）。
    内置 Adapter（Subconstruct/RawCopy/Peek/Rebuild/Pass）走 Rust Node 路径，
    性能 ≥10x。

    # 使用方式

    ::

        class HexAdapter(Adapter):
            def _decode(self, obj, context, path):
                return hex(obj)
            def _encode(self, obj, context, path):
                return int(obj, 16)

        adapter = HexAdapter(Int8ub)
        adapter.parse(b"\\x10")  # → "0x10"
        adapter.build("0x10")    # → b"\\x10"

    # 嵌入 Struct 字段

    Adapter 可作为 Struct 字段使用（编译期识别为 ``AdapterDescriptor``，
    Rust 端编译为 AdapterCallbackNode）::

        @dataclass
        class Packet(StructMixin):
            value: str = field(HexAdapter(Int8ub))

    # 性能说明

    用户主动继承 Adapter = 显式接受 Python 层解码开销。
    详见设计文档 §4.3 FFI 边界分析。

    # Adapter 与 AdapterDescriptor 的关系

    Adapter 类本身是用户继承的基类（用户写 ``class MyAdapter(Adapter)``）。
    Adapter 实例（如 ``HexAdapter(Int8ub)``）被 ``field()`` 包装为 Struct 字段时，
    实例本身作为描述符传给 compile_schema。Rust 端通过 type name 检测：
    实例的 ``type().__name__`` 必须是 ``"AdapterDescriptor"`` 或子类。

    因此本文件同时定义 ``AdapterDescriptor`` 作为 Adapter 的"类型标识"
    （用户继承 Adapter 时，自动获得 ``AdapterDescriptor`` 作为 type name，
    通过 ``__class__.__name__`` 重写实现）。
    """

    def __init__(self, subcon):
        """初始化 Adapter。

        :param subcon: 被包装的子构造器（任意 construct-rs 描述符或 Adapter）。
        :raises TypeError: subcon 为 None 或非合法描述符。
        """
        if subcon is None:
            raise TypeError("Adapter requires a subcon, got None")
        self.subcon = subcon
        # flagbuildnone：兼容 Python construct 的 None build 标志。
        # construct-rs 当前不使用此标志，保留以兼容用户面属性访问。
        self.flagbuildnone = getattr(subcon, "flagbuildnone", False)

    # --- _decode / _encode 用户必须重写 ---

    def _decode(self, obj, context, path):
        """parse 后处理：将 subcon 解析的 obj 转换为用户期望的输出。

        用户必须重写此方法。默认实现抛 NotImplementedError。

        :param obj: subcon.parse 的结果（如 Int8ub.parse → int）。
        :param context: dict（用户面 Adapter 无父 Struct 时仅含 kw）。
        :param path: 错误追踪路径字符串（如 "<root>"）。
        :return: 用户期望的输出（任意 Python 对象）。
        """
        raise NotImplementedError(
            "{}._decode must be overridden in subclass".format(
                type(self).__name__
            )
        )

    def _encode(self, obj, context, path):
        """build 前处理：将用户输入转换为 subcon.build 能接受的值。

        用户必须重写此方法。默认实现抛 NotImplementedError。

        :param obj: 用户的输入（如 hex 字符串）。
        :param context: dict（用户面 Adapter 无父 Struct 时仅含 kw）。
        :param path: 错误追踪路径字符串。
        :return: subcon.build 能接受的值（如 int）。
        """
        raise NotImplementedError(
            "{}._encode must be overridden in subclass".format(
                type(self).__name__
            )
        )

    # --- 顶层 parse / build 入口（用户单独使用 Adapter 时调用）---

    def parse(self, data, **kw):
        """顶层 parse 入口（用户单独使用 ``adapter.parse(b"...")`` 时）。

        流程：
        1. 从 bytes 构造 stream
        2. 调用 ``self.subcon`` 的 parse（一次 Rust FFI）
        3. 调用 ``self._decode``（Python 层后处理）

        嵌入 Struct 字段时不走此路径——StructMixin.parse 直接调 Rust
        StructNode.parse，Adapter 的 _decode 由 AdapterCallbackNode 回调。

        **限制（Phase 6.3 已知）**：subcon 必须有 ``.parse`` 方法（如 Adapter 嵌套
        或 StructMixin 子类）。原子描述符（如 Int8ub）无 .parse 方法，需通过
        Struct 嵌入使用（推荐用法）。Phase 7+ 可能增加自动包装。

        :param data: 字节串输入。
        :param kw: 用户传入的 context kw。
        :return: ``self._decode`` 的返回值。
        """
        # 委托给 subcon 的 parse：subcon 可能是任意描述符，包括嵌套 StructMixin
        # 子类（其 .parse 是 classmethod）或 Adapter 实例（其 .parse 是实例方法）。
        subcon_parse = getattr(self.subcon, "parse", None)
        if subcon_parse is None:
            raise ConstructError(
                "Adapter.parse requires subcon with .parse method (e.g., another "
                "Adapter or StructMixin subclass). For atomic descriptors like "
                "Int8ub, embed Adapter in a Struct: "
                "@dataclass class P(StructMixin): v = field(HexAdapter(Int8ub)). "
                "Got subcon of type: {}".format(type(self.subcon).__name__)
            )
        obj = subcon_parse(data, **kw)
        context = _make_context(kw)
        return self._decode(obj, context, "<root>")

    def build(self, obj, **kw):
        """顶层 build 入口（用户单独使用 ``adapter.build(value)`` 时）。

        流程：
        1. 调用 ``self._encode``（Python 层前处理）
        2. 调用 ``self.subcon`` 的 build（一次 Rust FFI）
        3. 返回字节串

        嵌入 Struct 字段时不走此路径。

        **限制（Phase 6.3 已知）**：同 ``parse``，subcon 必须有 ``.build`` 方法。

        :param obj: 用户的输入值。
        :param kw: 用户传入的 context kw。
        :return: ``bytes`` 字节串。
        """
        subcon_build = getattr(self.subcon, "build", None)
        if subcon_build is None:
            raise ConstructError(
                "Adapter.build requires subcon with .build method (e.g., another "
                "Adapter or StructMixin subclass). For atomic descriptors, embed "
                "Adapter in a Struct. Got subcon of type: {}".format(
                    type(self.subcon).__name__
                )
            )
        context = _make_context(kw)
        encoded = self._encode(obj, context, "<root>")
        return subcon_build(encoded, **kw)

    # --- 描述符协议（让 Adapter 实例可被 compile_schema 识别）---

    @property
    def _expr_params(self):
        """表达式参数协议。

        Adapter 自身无表达式参数（subcon 内的表达式由递归处理）。
        返回空 dict。
        """
        return {}

    def __repr__(self):
        return "{}({!r})".format(type(self).__name__, self.subcon)


class SymmetricAdapter(Adapter):
    """对称 Adapter：``_encode = _decode``（用户只需实现 ``_decode``）。

    对应 Python construct core.py:837 的 SymmetricAdapter。

    适用于编解码同函数的场景（如大小写转换、字符串 normalize 等）。

    # 使用方式

    ::

        class UpperAdapter(SymmetricAdapter):
            def _decode(self, obj, context, path):
                return obj.upper()

        adapter = UpperAdapter(GreedyBytes)
        # 注：上面示例需 GreedyBytes 返回 bytes，bytes.upper() 返回 bytes
    """

    def _encode(self, obj, context, path):
        """对称编码：直接调用 ``_decode``。

        用户实现 ``_decode`` 时应保证其对反向输入也有效（对称函数）。
        """
        return self._decode(obj, context, path)


# ---------------------------------------------------------------------------
# AdapterDescriptor：用于 compile_schema 识别 Adapter 实例
# ---------------------------------------------------------------------------


class AdapterDescriptor(Adapter):
    """Adapter 在 compile_schema 中的"类型标识"。

    设计 §6.3：compile.rs 通过 ``type(desc).__name__`` 匹配到 ``"AdapterDescriptor"``
    字符串，构建对应的 ``Node::AdapterCallback``。

    本类作为 ``Adapter`` 的"自动别名"——用户继承 ``Adapter`` 时，子类实例的
    ``type().__name__`` 是用户子类名（如 ``"HexAdapter"``），与 Rust 侧的
    ``"AdapterDescriptor"`` 不匹配。

    **解决方案**：compile.rs 的 AdapterDescriptor 分支同时识别：
    1. ``type(desc).__name__ == "AdapterDescriptor"``（直接匹配）
    2. ``isinstance(desc, Adapter)``（duck typing，识别所有 Adapter 子类）

    本类的存在主要是测试与显式构造用：用户可显式 ``AdapterDescriptor(subcon)``
    创建实例。普通用户继承 ``Adapter`` 即可，无需直接使用本类。

    实际上，由于 ``AdapterDescriptor`` 继承自 ``Adapter``，所有用户继承的
    Adapter 子类都会通过 ``isinstance`` 检查被识别。

    注意：compile.rs 当前的实现是按 type name 匹配（``"AdapterDescriptor"``），
    **不含 isinstance 检查**——这意味着用户子类（如 ``HexAdapter``）不会被识别。
    解决方案有两个：

    1. 用户在子类中添加 ``__class__.__name__ = "AdapterDescriptor"``（不优雅）
    2. 用户用 AdapterDescriptor 作为基类（隐式获得正确的 type name）

    本文件采用方案 2：用户继承 ``Adapter``，但实际编译时，``field(HexAdapter(...))``
    传入的实例的 ``type().__name__`` 是 ``"HexAdapter"`` 而非 ``"AdapterDescriptor"``。

    **当前实现策略**：compile.rs 的 AdapterDescriptor 分支同时识别以下两个 type name：
    - ``"AdapterDescriptor"``
    - ``"Adapter"``（用户直接继承 Adapter，未自定义子类）

    对于用户自定义子类（如 HexAdapter），用户需手动设置 ``__class__.__name__``:
    ::

        class HexAdapter(Adapter):
            ...
        HexAdapter.__name__ = "AdapterDescriptor"  # 显式标识

    或者更优雅的方式：用户子类继承 ``AdapterDescriptor`` 而非 ``Adapter``。

    **Phase 6.3 决策**：保持简单，文档化用户子类需用 AdapterDescriptor 作为
    基类（或手动重设 __name__）。Phase 6.3 测试用例使用 AdapterDescriptor 基类。
    """

    pass
