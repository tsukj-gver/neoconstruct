# 模块设计：dataclass-first API（Phase 14）

> **文档性质**：模块设计（ARCH 产出）
> **日期**：2026-06-16
> **版本**：v3（N-1 复检修正）
> **状态**：DESIGNING（v1 → v2 → v3 修正，待 REV 复检）
> **范围**：ConstructMixin + cs_field + direct-to/from-dataclass 的 parse/build 路径
>
> **v2 修订记录**（回应 REV 阻塞×3 + 重要×4）：
> - **B-FEAS-1**：移除 `.field()` builder 假设，改用 `Struct(**kwargs)` 关键字形式（§2.2、§5、§8.2）
> - **B-FEAS-2**：消除 Python 层对 `extract_subcon` 的调用，全部下沉到 Rust 侧 `py_struct`（§8.2.1）
> - **B-FEAS-3**：逐行对照代码重写 §9.4 暴露状态表
> - **I-COMP-1**：修正 `_dict_to_instance` 类型解析，处理 `from __future__ import annotations`（§8.4.2）
> - **I-COMP-2**：多余键过滤改用 `dataclasses.fields()` 白名单（§6.4、§8.4.2）
> - **I-COMP-3**：`@construct_dataclass` 改为动态创建子类，避免修改 `__bases__`（§4.3）
> - **I-FEAS-1**（v2，**修正不完整**）：v2 试图明确嵌套 dataclass 走 Dynamic 路径，但 §8.2.3 正文仍误述为"内联编译为 `CompiledNode::Struct`"，由 REV v2 复检 N-1 发现（见 v3 N-1 彻底修正）
>
> **v3 修订记录**（回应 REV v2 复检 N-1 重要项）：
> - **N-1**：逐行核实 `py_struct` → `apply_rename` → `make_renamed` → `extract_subcon` 代码路径（`constructs_composite.rs:227-233`、`py_renamed.rs:175,86-104,93`、`py_adapter.rs:417-441`、`constructs_adapter.rs:1315-1457`），修正 §8.2.1 case (b)、§8.2.2、§8.2.3 中"嵌套 PyStruct 内联为 `CompiledNode::Struct`"的错误描述。真实路径为 `Dynamic(PyConstructAdapter(PyRenamed(PyStruct)))`（PyStruct 不在 `try_extract_registered`，PyRenamed 被明确排除 `line 1451-1454`）。补充功能影响（无）、性能影响（MVP 可接受：GIL + BytesIO round-trip）、MVP 方案标注及后续优化方向（方案 B PyDataclassSink / Rust 内联）
>
> **前置文档**：
> - `docs/refactor/顶层架构设计.md`（§5 Python API 设计方向、§8.2.8 ConstructMixin 验收标准）
> - `construct-py/src/compiled_ext.rs`（CompiledSchemaHolder，Phase 13 产物）
> - `construct-py/src/py_input.rs`（PyInput，build 方向已支持 attribute）
> - `construct-py/src/py_sink.rs`（PyDictSink，parse 方向产出 PyDict）

---

## 1. 概述与目标

### 1.1 设计目标

Phase 13 完成了 FFI 执行层（`CompiledSchemaHolder::parse_bytes_py` / `build_from_py`），
parse 产出 PyDict、build 接受任意 PyObject。Phase 14 在此之上构建 **Python 用户面向的
dataclass API**，让用户用 `@dataclass` + `cs_field` 声明二进制格式：

```python
from dataclasses import dataclass
from construct_rust import ConstructMixin, cs_field, Int32ul, Bytes

@dataclass
class Header(ConstructMixin):
    magic: bytes = cs_field(Bytes(4))
    version: int = cs_field(Int32ul)
    flags: int = cs_field(Int32ul)

# parse 产出 dataclass 实例（不是 dict）
header = Header.parse(b"MAGC\x01\x00\x00\x00\x02\x00\x00\x00")
assert header.magic == b"MAGC"
assert header.version == 1

# build 从 dataclass 实例
data = header.build()
assert data == b"MAGC\x01\x00\x00\x00\x02\x00\x00\x00"
```

### 1.2 核心设计原则

| 原则 | 说明 |
|------|------|
| **P1 Container 对用户不可见** | parse 直出 dataclass 实例，build 直读 dataclass 属性，用户永远不接触 dict/Container |
| **P2 复用 Phase 13 执行层** | ConstructMixin 内部调用 `CompiledSchemaHolder`，不绕过编译执行树 |
| **P3 与标准 dataclass 无缝兼容** | 用户保留 `@dataclasses.dataclass`，IDE / 类型检查器 / `dataclasses.fields()` 原生工作 |
| **P4 与声明式 API 互操作** | ConstructMixin 子类可作为 `Struct` 的子构造器，反之亦然 |
| **P5 渐进式优化** | MVP 用 PyDict 中转（零 Rust 改动），优化阶段直构 dataclass（新 sink） |

### 1.3 现有基础（Phase 13 遗产）

| 组件 | 现状 | Phase 14 利用方式 |
|------|------|------------------|
| `CompiledSchemaHolder::parse_bytes_py` | 产出 PyDict（经 `PyDictSink::new_root`） | MVP 直接复用；优化阶段新增 `target_cls` 重载 |
| `CompiledSchemaHolder::build_from_py` | 接受任意 PyObject（经 `PyInput`） | **完全复用**——PyInput 已支持 attribute 访问 |
| `PyInput::get_field_bound` | 先 `get_item` 后 `getattr`（dict/attr 双模式） | dataclass 实例走 `getattr` 路径，**零改动** |
| `PyDictSink` | 产出 `Py<PyDict>` / `Py<PyList>` | MVP 复用；优化阶段新增 `PyDataclassSink` |
| `OutputSink` trait | 已有 `into_produced` / `finish_field` 扩展点 | 新 sink 实现该 trait 即可接入 |
| `SchemaCompiler` | 编译 `CombinedConstruct` → `CompiledSchema` | v2：ConstructMixin 通过新增 pyfunction `compile_schema` 间接调用它（§9.4） |

### 1.4 设计约束（不可妥协项）

1. **C1**：`CompiledSchemaHolder` 现有 `parse_bytes_py` / `build_from_py` 签名**不变**
   （Phase 13 已验收，声明式 API 依赖）。新增能力通过**重载**或**新方法**提供。
2. **C2**：construct-rs（Rust 内核）**不依赖 pyo3**。dataclass 相关的 PyO3 代码全部在
   construct-py。`OutputSink` trait 的 `ProducedOutput::Object(Box<dyn Any + Send + Sync>)`
   是唯一的跨 crate 通道。
3. **C3**：用户代码**零修改**即可在标准 `@dataclass` 工具链下工作（`dataclasses.fields()`、
   `dataclasses.asdict()`、`isinstance(x, Header)` 等全部正常）。
4. **C4**：parse/build 对称性——`build(parse(data)) == data` 且 `parse(build(obj)) == obj`。

---

## 2. ConstructMixin — Python 基类

### 2.1 职责

`ConstructMixin` 是用户 dataclass 的基类，提供：
- 类方法 `parse(cls, data)` / `parse_stream(cls, stream)` / `parse_file(cls, filename)`：编译 + parse
- 实例方法 `build(self)` / `build_stream(self, stream)` / `build_file(self, filename)`：编译 + build
- 类方法 `sizeof(cls, **kw)`：编译 + sizeof
- 内部缓存 `CompiledSchemaHolder`（per-class，懒初始化）

### 2.2 Python 端结构（纯 Python，位于 `construct-py/construct_rust/_mixins.py`）

> **决策**：ConstructMixin 实现为**纯 Python 类**，不引入 PyO3 metaclass。
> 理由见 §4（排除方案 C metaclass）。它内部调用 Phase 13 暴露的 Rust 函数。

```python
# construct-py/construct_rust/_mixins.py
from typing import ClassVar, Optional
from construct_rust._core import (
    CompiledSchemaHolder,  # Phase 14 新增暴露为 pyclass（§9.4）
    compile_schema,        # Phase 14 新增 pyfunction（§9.4）
    Struct,                # py_struct pyfunction（Phase 10 已暴露，支持 **subconskw）
)
from construct_rust._fields import _get_field_subcon, _collect_field_subcons


class ConstructMixin:
    """
    Mixin base for dataclass-first binary formats.

    Subclass with @dataclasses.dataclass and cs_field-decorated fields::

        @dataclass
        class Header(ConstructMixin):
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul)
    """

    # Per-class compiled schema cache. Lazily initialized on first
    # parse/build/sizeof call. ClassVar prevents dataclass from treating
    # this as a field.
    _compiled: ClassVar[Optional["CompiledSchemaHolder"]] = None

    # ── Compilation ──

    @classmethod
    def _compile(cls) -> "CompiledSchemaHolder":
        """Compiles this dataclass's fields into a CompiledSchemaHolder.

        Scans dataclasses.fields(cls), collects cs_field subcons into a
        keyword dict, passes it to Rust ``Struct(**subcons_kw)`` (which
        internally calls ``extract_subcon`` on each value), then compiles.
        Result cached in cls._compiled.

        Key design point (B-FEAS-1/B-FEAS-2 fix): the subcon→Rust
        extraction happens **inside** ``py_struct`` on the Rust side.
        Python never calls ``extract_subcon`` (it returns a non-exposable
        ``Box<CombinedConstruct>``). Likewise, ``PyStruct`` has no
        ``.field()`` pymethod, so we use the ``Struct(**kwargs)`` factory
        form instead of a builder pattern.
        """
        if cls._compiled is not None:
            return cls._compiled
        subcons_kw = _collect_field_subcons(cls)   # {name: subcon, ...}
        struct_decl = Struct(**subcons_kw)           # Rust py_struct → PyStruct
        holder = compile_schema(struct_decl)         # FFI → CompiledSchemaHolder
        cls._compiled = holder
        return holder

    # ── Parse (classmethods) ──

    @classmethod
    def parse(cls, data: bytes, **kw) -> "ConstructMixin":
        """Parse bytes into a dataclass instance."""
        holder = cls._compile()
        result_dict = holder.parse_bytes_py(data, **kw)  # Phase 14: returns dict
        # MVP path: dict → dataclass (see §6 scheme A)
        return cls._dict_to_instance(result_dict)

    @classmethod
    def parse_stream(cls, stream, **kw) -> "ConstructMixin":
        """Parse from a file-like stream object."""
        # Read all bytes then delegate (matching PyStream behavior).
        data = stream.read()
        return cls.parse(data, **kw)

    @classmethod
    def parse_file(cls, filename: str, **kw) -> "ConstructMixin":
        """Parse from a file path."""
        with open(filename, "rb") as f:
            return cls.parse(f.read(), **kw)

    # ── Build (instance methods) ──

    def build(self, **kw) -> bytes:
        """Build this dataclass instance into bytes."""
        holder = type(self)._compile()
        # PyInput reads attributes lazily — self is a dataclass instance,
        # getattr(self, field_name) works natively. No dict conversion.
        return holder.build_from_py(self, **kw)

    def build_stream(self, stream, **kw) -> None:
        """Build and write to a file-like stream."""
        data = self.build(**kw)
        stream.write(data)

    def build_file(self, filename: str, **kw) -> None:
        """Build and write to a file path."""
        with open(filename, "wb") as f:
            self.build_stream(f, **kw)

    # ── Sizeof ──

    @classmethod
    def sizeof(cls, **kw) -> int:
        """Compute the static byte size of this format."""
        holder = cls._compile()
        return holder.sizeof(**kw)
```

### 2.3 关键设计点

| 点 | 说明 |
|----|------|
| **`_compiled` 用 ClassVar** | 避免被 `@dataclass` 当作字段。`typing.ClassVar` 是 dataclass 的标准排除机制。 |
| **懒编译** | 首次 `parse`/`build` 才触发 `compile_schema`。避免模块导入时编译所有 dataclass。 |
| **per-class 缓存** | `cls._compiled` 是类属性，所有实例共享同一 `CompiledSchemaHolder`。子类继承时各自独立缓存（Python 类属性查找规则）。 |
| **用 `Struct(**kwargs)` 而非 `.field()`** | `PyStruct` 没有 `.field()` pymethod（B-FEAS-1 修正）。`py_struct` pyfunction 支持 `**subconskw`（`constructs_composite.rs:215`），一次调用构建完整 Struct。 |
| **不在 Python 调用 `extract_subcon`** | `extract_subcon` 返回 `Box<CombinedConstruct>`（Rust 类型），无法暴露给 Python（B-FEAS-2 修正）。subcon 提取下沉到 `py_struct` 内部（Rust 侧统一处理）。 |
| **parse 返回 `cls._dict_to_instance(dict)`** | MVP 路径（§6 方案 A + §8.4 嵌套递归转换）。处理嵌套 dict→嵌套实例，并基于 `fields()` 白名单过滤多余键。 |
| **build 传 `self`** | dataclass 实例本身就是 PyObject，`PyInput::get_field_bound` 的 `getattr` 路径直接读属性。**零转换**。 |

### 2.4 与 Python 原版 Container 的对比

| 维度 | Python 原版 construct | ConstructMixin（本设计） |
|------|---------------------|------------------------|
| parse 产出 | `Container`（dict 子类，支持属性访问） | dataclass 实例（原生属性访问） |
| 用户类型提示 | 无（动态 dict） | 有（`header: Header`，IDE 自动补全） |
| 字段缺失行为 | `Container` 缺键 → `KeyError` | dataclass `__init__` 缺参数 → `TypeError`（更早失败） |
| 与 dataclass 互操作 | 不支持 | 原生（用户就是 dataclass） |

---

## 3. cs_field — dataclass 字段描述

### 3.1 职责

`cs_field(subcon)` 是 `dataclasses.field()` 的包装，附加 construct 构造器信息到字段的
`metadata` 中。编译时（`ConstructMixin._compile`）扫描 metadata 提取 subcon。

### 3.2 Python 端结构（`construct-py/construct_rust/_fields.py`）

```python
# construct-py/construct_rust/_fields.py
import dataclasses
from typing import Any

# Metadata key under which the construct subcon is stored.
_CS_SUBCON_KEY = "cs_subcon"


def cs_field(subcon: Any, **field_kwargs) -> Any:
    """Construct-aware dataclass field declaration.

    Wraps dataclasses.field(), attaching the construct subcon to the
    field's metadata. The subcon is extracted at compile time by
    ConstructMixin._compile.

    Args:
        subcon: A construct subcon (e.g. Int32ul, Bytes(4), or another
            ConstructMixin subclass for nesting).
        **field_kwargs: Passed through to dataclasses.field() (e.g.
            default=, default_factory=, repr=False).

    Returns:
        The return value of dataclasses.field(...), suitable as a
        dataclass field default.

    Example::

        @dataclass
        class Header(ConstructMixin):
            magic: bytes = cs_field(Bytes(4))
            version: int = cs_field(Int32ul, default=0)

    Note:
        cs_field does NOT set a default value for the field. The field
        is required unless ``default`` or ``default_factory`` is passed
        in ``field_kwargs``. This matches dataclass semantics: parse
        always produces all fields; build requires all non-default fields.
    """
    metadata = dict(field_kwargs.pop("metadata", {}))
    metadata[_CS_SUBCON_KEY] = subcon
    return dataclasses.field(metadata=metadata, **field_kwargs)


def _get_field_subcon(field: dataclasses.Field) -> Any:
    """Extracts the construct subcon from a dataclass field's metadata.

    Called by _collect_field_subcons during compilation.

    Raises:
        TypeError: If the field has no cs_field metadata (i.e. it was
            declared without cs_field()).
    """
    subcon = field.metadata.get(_CS_SUBCON_KEY)
    if subcon is None:
        raise TypeError(
            f"ConstructMixin field '{field.name}' must be declared with "
            f"cs_field(). Did you forget cs_field()?"
        )
    return subcon


def _collect_field_subcons(cls) -> dict:
    """Collects ``{field_name: subcon}`` pairs from a ConstructMixin subclass.

    This is the single entry point used by ``ConstructMixin._compile`` and
    ``ConstructMixin._build_struct_decl``. It scans
    ``dataclasses.fields(cls)``, extracts each cs_field subcon, and
    resolves nested ConstructMixin subclasses via ``_resolve_nested_subcon``.

    The returned dict is passed directly to ``Struct(**subcons_kw)``.
    The Rust-side ``py_struct`` then calls ``extract_subcon`` on each value
    — Python never touches ``extract_subcon`` (B-FEAS-2 fix).

    Returns:
        A dict mapping field names to construct subcons (PyO3 wrappers,
        PyStruct instances for nested dataclasses, or None for forward
        references — the latter raises here, see §8.3).
    """
    import dataclasses
    subcons_kw = {}
    for f in dataclasses.fields(cls):
        subcon = _get_field_subcon(f)
        subcon = _resolve_nested_subcon(subcon, cls)  # §8.2.1
        subcons_kw[f.name] = subcon
    return subcons_kw
```

### 3.3 关键设计点

| 点 | 说明 |
|----|------|
| **用 `metadata` 存储 subcon** | dataclass 的 `metadata` 是 `MappingProxyType`（只读 dict），专为扩展数据设计。不污染字段默认值。 |
| **`cs_field` 不设 default** | 默认情况下字段是必需的（匹配 construct 语义：parse 产出所有字段）。用户可通过 `cs_field(Int32ul, default=0)` 显式设默认值。 |
| **`field_kwargs` 透传** | 用户可传 `default=`、`default_factory=`、`repr=False`、`compare=False` 等，全部透传给 `dataclasses.field`。 |
| **类型提示由注解决定** | `magic: bytes = cs_field(Bytes(4))` —— 类型是 `bytes`，subcon 是 `Bytes(4)`。两者独立，类型检查器看 `bytes`，编译器看 metadata。 |

### 3.4 边界条件

| 场景 | 行为 |
|------|------|
| 字段无 cs_field metadata | `_get_field_subcon` 抛 `TypeError`（编译期，非运行时） |
| `cs_field(None)` | subcon 为 None —— 用于前向引用（§8.3），编译时延迟到首次 parse/build 解析 |
| `cs_field(Int32ul, default=0)` | 字段有默认值，dataclass `__init__` 中该参数可选；build 时若实例未设值则用默认值 |
| `cs_field(Int32ul, default_factory=list)` | 同上，但用工厂；适用于 mutable 默认值 |
| metadata 已有其他键 | `cs_field` 合并不覆盖（`metadata = dict(field_kwargs.pop("metadata", {}))`） |

---

## 4. 装饰器方案选择

### 4.1 候选方案

| 方案 | 用户写法 | 实现机制 |
|------|---------|---------|
| **A. 继承 ConstructMixin** | `@dataclass` + `class X(ConstructMixin)` | ConstructMixin 是普通基类，提供方法 |
| **B. @construct_dataclass 装饰器** | `@construct_dataclass` + `class X` | 装饰器内部调 `@dataclass` + 注入 ConstructMixin 行为 |
| **C. metaclass** | `class X(metaclass=ConstructMeta)` | metaclass 在类创建时扫描字段、注入方法 |

### 4.2 评估

| 维度 | A 继承 | B 装饰器 | C metaclass |
|------|--------|---------|-------------|
| 与标准 `@dataclass` 兼容 | ✅ 用户显式写 `@dataclass` | ⚠ 装饰器需内部调 `dataclasses.dataclass` | ❌ metaclass 与 dataclass 的 `__init_subclass__` 冲突 |
| IDE / 类型检查器 | ✅ 原生支持 | ⚠ 需识别 `@construct_dataclass` | ❌ metaclass 类型推断困难 |
| `dataclasses.fields()` 工作 | ✅ | ✅（只要装饰器调了 `dataclasses.dataclass`） | ⚠ 取决于 metaclass 实现 |
| 用户学习成本 | 低（标准继承） | 中（新装饰器） | 高（metaclass 概念） |
| 调试难度 | 低 | 低 | 高（metaclass 魔法） |
| 多继承支持 | ✅（Mixin 设计） | ⚠ 装饰器需处理基类 | ❌ metaclass 冲突 |
| 前向引用处理 | ✅ 懒编译 | ✅ 懒编译 | ⚠ metaclass 在类创建时就扫描，前向引用失败 |

### 4.3 选择：方案 A（主推荐）+ 方案 B（语法糖）

**主推荐方案 A**：用户写 `@dataclass` + 继承 `ConstructMixin`。

**理由**：
1. **与 Python 生态最大兼容**：标准 `@dataclass`、`dataclasses.fields()`、`dataclasses.asdict()`、
   `isinstance(x, Header)`、IDE 自动补全、mypy/pyright 类型检查全部原生工作。
2. **零魔法**：ConstructMixin 是普通基类，无 metaclass 副作用。前向引用（字符串注解）通过
   懒编译（首次 parse/build 才扫描字段）天然解决。
3. **多继承友好**：用户可 `class X(ConstructMixin, SomeOtherMixin)`。metaclass 方案在多继承时
   会遇到 metaclass 冲突（`TypeError: metaclass conflict`）。
4. **C3 Realms 经验**：pydantic v1 用 metaclass 导致大量边界问题，pydantic v2 弃用 metaclass
   改用 `@dataclass` + 函数式验证。我们吸取这个教训。

**方案 B 作为语法糖**（可选，非必需）：提供 `@construct_dataclass` 给偏好一行装饰器的用户。

> **I-COMP-3 修正**：v1 通过修改 `cls.__bases__` 注入 ConstructMixin，有风险（可能与用户
> 已有基类冲突，且 `__bases__` 赋值在某些 Python 实现中行为不一致）。
> v2 改为：检查是否已继承 ConstructMixin，如果没有则**动态创建新子类**（通过 `type()`）。

```python
# construct-py/construct_rust/__init__.py
def construct_dataclass(cls=None, **dataclass_kwargs):
    """Syntactic sugar: @dataclasses.dataclass + ConstructMixin injection.

    Equivalent to::
        @dataclasses.dataclass(**dataclass_kwargs)
        class X(ConstructMixin):
            ...

    I-COMP-3: Instead of mutating ``cls.__bases__`` (risky), this creates a
    new subclass via ``type()`` when ``cls`` does not already inherit
    ConstructMixin. The new class has the same name and __dict__ as ``cls``
    but with ConstructMixin prepended to bases.

    Usage::
        @construct_dataclass
        class Header:
            magic: bytes = cs_field(Bytes(4))

        # or with dataclass kwargs:
        @construct_dataclass(frozen=True)
        class Header:
            ...
    """
    import dataclasses

    def decorate(original_cls):
        # Check if ConstructMixin is already in the MRO
        if ConstructMixin in original_cls.__mro__:
            # Already inherits ConstructMixin — just apply @dataclass
            return dataclasses.dataclass(original_cls, **dataclass_kwargs)
        # Create a new subclass with ConstructMixin prepended (I-COMP-3 fix:
        # avoids mutating __bases__, which can conflict with existing bases)
        new_cls = type(
            original_cls.__name__,
            (ConstructMixin,) + original_cls.__bases__,
            dict(original_cls.__dict__),
        )
        new_cls.__module__ = original_cls.__module__
        new_cls.__qualname__ = original_cls.__qualname__
        return dataclasses.dataclass(new_cls, **dataclass_kwargs)

    if cls is not None:
        return decorate(cls)  # @construct_dataclass (no parens)
    return decorate            # @construct_dataclass(frozen=True)
```

### 4.4 排除方案 C（metaclass）的详细理由

1. **前向引用硬伤**：metaclass 在类创建时（`__init_subclass__` / `__new__`）扫描字段，
   此时字符串注解 `"Node"` 尚未解析（`Node` 类正在定义中）。需要复杂的延迟机制。
   方案 A/B 的懒编译（首次 parse/build 才扫描）天然规避。
2. **与 dataclass metaclass 冲突**：`@dataclass` 不使用 metaclass，但如果用户同时用
   其他带 metaclass 的库（如某些 ORM），会触发 `TypeError: metaclass conflict`。
3. **调试黑盒**：metaclass 的 `__new__` / `__init__` 在类创建时执行，错误堆栈深、
   难以定位。方案 A 的错误发生在 `_compile()` 调用时，堆栈清晰。
4. **现代 Python 趋势**：PEP 487（`__init_subclass__`）和 pydantic v2 的转向都表明
   metaclass 是过时方案。

---

## 5. 编译流程（dataclass → CompiledSchema）

### 5.1 流程总览

```
用户 dataclass 类
    │
    ▼  ConstructMixin._compile()（首次 parse/build 触发，懒编译）
_collect_field_subcons(cls) → {name: subcon, ...}
    │  对每个 field：_get_field_subcon(f) → 从 metadata 取 subcon
    │  嵌套 ConstructMixin 子类 → _resolve_nested_subcon（§8.2.1）
    │
    ▼
Struct(**subcons_kw)           ← Rust py_struct（per-class 一次构建）
    │  Rust 侧 py_struct 内部对每个 value 调用 extract_subcon
    │  （Python 不接触 extract_subcon —— B-FEAS-2 修正）
    │
    ▼
compile_schema(struct_decl)    ← FFI 穿越（per-class 一次）
    │
    ▼
CompiledSchemaHolder           ← 缓存到 cls._compiled
    │
    ▼  后续 parse/build/sizeof 直接复用缓存
```

### 5.2 关键步骤详解

#### 步骤 1：扫描 fields

```python
import dataclasses
for f in dataclasses.fields(cls):
    # f.name: str          字段名（= Struct 字段名）
    # f.type: type/str     类型注解（cs_field 不使用，仅文档）
    # f.metadata: Mapping  含 _CS_SUBCON_KEY
    # f.default:           dataclass 默认值（cs_field 透传）
    subcon = _get_field_subcon(f)   # f.metadata["cs_subcon"]
```

**注意**：`dataclasses.fields()` 只返回**有类型注解**的字段。无注解的类属性
（如 `_compiled: ClassVar`）会被排除——这正是我们想要的（ClassVar 不参与编译）。

#### 步骤 2：收集 subcons 并构建 Rust Struct

```python
subcons_kw = _collect_field_subcons(cls)
# subcons_kw 中的每个 value 可能是：
#   (a) 普通 PyO3 构造器 wrapper（Int32ul / Bytes(4) 等）→ py_struct 内部 extract_subcon 直接提取
#   (b) ConstructMixin 子类（嵌套 dataclass）→ §8.2 递归处理后得到 PyStruct，py_struct 内部提取 inner
#   (c) None（前向引用占位）→ §8.3 延迟解析（_collect_field_subcons 中先报错，由 _compile 的前向引用分支处理）
struct_decl = Struct(**subcons_kw)   # py_struct pyfunction（Phase 10）
```

**`Struct()` 的来源**：Phase 10 已暴露 `py_struct` 工厂函数（`constructs_composite.rs:215`），
签名 `Struct(*subcons, **subconskw)`。它接受关键字参数，内部对每个 value 调用
`extract_subcon`（B-FEAS-1/B-FEAS-2 修正：不在 Python 调用 extract_subcon，不在 PyStruct 上调用 .field()）。

#### 步骤 3：编译

```python
holder = compile_schema(struct_decl)
# compile_schema 是 Phase 14 新增的 pyfunction（包装 api.rs:compile_schema）
# 输入 PyStruct，返回 CompiledSchemaHolder（Phase 14 新增 pyclass）
```

**FFI 边界**：此步骤穿越 FFI 一次（per-class），将 Python 声明树（PyStruct + 子 subcon）
传入 Rust 编译为 `CompiledSchema`。编译结果缓存，后续零编译开销。

#### 步骤 4：缓存

```python
cls._compiled = holder   # ClassVar，所有实例共享
```

**缓存失效**：dataclass 定义后字段不再变化（dataclass 是不可变声明），无需失效机制。
若用户动态修改 `__annotations__`（极罕见），需手动 `cls._compiled = None` 重新编译。

### 5.3 与声明式 API 编译流程的对比

| 维度 | 声明式 API（`Struct(...).parse()`） | dataclass API（`Header.parse()`） |
|------|----------------------------------|----------------------------------|
| 声明来源 | 用户手动组合 `Struct("a"/Byte)` | `dataclasses.fields()` 扫描 |
| Struct 构建 | 用户直接传 subcons | `_collect_field_subcons` 收集 kwargs，传给 `Struct(**kwargs)` |
| 编译触发 | 首次 parse/build（PyStruct 持有 OnceLock） | 首次 parse/build（cls._compiled 懒初始化） |
| 缓存位置 | PyStruct wrapper 内（OnceLock） | 类属性 `_compiled`（ClassVar） |
| CompiledSchemaHolder | 相同 | 相同 |

**结论**：两者共享同一套 `compile_schema` + `CompiledSchemaHolder`，区别仅在"如何从用户
声明构建出 Rust Struct"。dataclass API 的 `_compile` 本质上是"fields 扫描 → Struct kwargs"
的适配层。

### 5.4 编译期错误处理

| 错误场景 | 检测时机 | 错误类型 |
|---------|---------|---------|
| 字段无 cs_field metadata | `_get_field_subcon` 扫描时 | `TypeError`（Python 层，清晰消息） |
| subcon 是不支持的类型 | `py_struct`/`compile_schema` FFI 调用时 | `ConstructError`（Rust 层映射） |
| 前向引用未解析（subcon=None 且类型注解无效） | 首次 parse/build 时 | `NameError` / `TypeError` |
| 嵌套 dataclass 循环引用 | `_resolve_nested_subcon` 递归时 | `RecursionError`（带路径） |

---

## 6. direct-to-dataclass parse（PyDict → dataclass 实例）

### 6.1 候选方案

| 方案 | 机制 | Rust 改动 | 性能 |
|------|------|----------|------|
| **A. PyDict 中转** | `parse_bytes_py`（现有）→ PyDict → Python 层 `cls(**dict)` | 零 | 基线（有 dict 中转） |
| **B. PyDataclassSink 直构** | 新 sink，parse 时直接 `setattr` 到 dataclass 实例 | 新增 sink + holder 重载 | 最优（无中转） |
| **C. __post_init__ 转换** | PyDictSink 产 PyDict，用户写 `__post_init__` 转 dataclass | 零 | 同 A，且需用户手写代码 |

### 6.2 评估

| 维度 | A PyDict 中转 | B PyDataclassSink | C __post_init__ |
|------|--------------|-------------------|-----------------|
| 用户代码量 | 零（ConstructMixin 内部） | 零 | **需手写 `__post_init__`**（违反 P1 对用户不可见） |
| Rust 改动 | 零 | 新增 sink + 1 个 holder 重载 | 零 |
| 性能 | dict 中转开销（`set_item` × N + `cls(**dict)`） | 直接 `setattr` × N，省去 dict 构建 + kwargs 展开 | 同 A |
| 实现复杂度 | 极低 | 中（新 sink，需处理嵌套） | 低但违反设计原则 |
| 嵌套 dataclass 支持 | 天然（dict 嵌套 dict，`cls(**{header: Header(**inner_dict)})`） | 需递归 sink | 需用户手动处理嵌套 |
| 字段顺序保证 | dataclass `__init__` 按 fields 顺序接受 kwargs，dict 保序 | sink 按 parse 顺序 setattr | 取决于用户代码 |

### 6.3 选择：方案 A（MVP）→ 方案 B（优化）

**MVP 阶段用方案 A**，优化阶段升级到方案 B。理由：

1. **方案 A 零 Rust 改动，立即可用**：复用 Phase 13 的 `parse_bytes_py`，ConstructMixin
   的 `parse` 方法只需 `cls(**holder.parse_bytes_py(data))`。符合顶层架构 §5.5
   "MVP 阶段纯 Python ConstructMixin"。
2. **方案 A 已满足 P1（对用户不可见）**：用户调用 `Header.parse(data)` 得到 `Header` 实例，
   dict 中转是 ConstructMixin 内部细节，用户不感知。
3. **方案 B 是性能优化**：当 S-PERF 基准显示 dict 中转是瓶颈时（预计 parse 密集场景），
   再实现 `PyDataclassSink`。符合顶层架构 §8.2.8 `[P]` 验收标准（"直构比 dict 中转快 ≥30%"）。
4. **排除方案 C**：违反 P1（对用户不可见），且嵌套 dataclass 需用户手写递归转换，体验差。

### 6.4 方案 A 实现细节（MVP）

```python
# ConstructMixin.parse（§2.2 已展示，此处细化）
@classmethod
def parse(cls, data: bytes, **kw) -> "ConstructMixin":
    holder = cls._compile()
    result = holder.parse_bytes_py(data, **kw)  # 返回 dict
    # dict → dataclass 实例（含嵌套递归 + fields() 白名单过滤）
    return cls._dict_to_instance(result)
```

**边界条件**：

| 场景 | 行为 |
|------|------|
| PyDict 键与 dataclass 字段名完全匹配 | 正常实例化 |
| PyDict 多余键（Struct 有匿名字段产出，如 `_compiled_schema` 内部缓存键） | **基于 `dataclasses.fields(cls)` 白名单过滤**（I-COMP-2 修正）。只取白名单内的键，忽略其余。**不**用下划线前缀过滤（下划线前缀是脆弱约定，用户可能合法地有 `_internal` 字段）。 |
| PyDict 缺少键（dataclass 字段无默认值） | `cls(**dict)` 抛 `TypeError: missing required argument`。<br>这通常是 parse/build 不对称的信号（Struct 字段比 dataclass 多） |
| dataclass 字段有默认值且 PyDict 缺该键 | 正常（用默认值） |
| 嵌套 dict（嵌套 dataclass 字段） | `cls(header={'size': 4})` → `header` 参数收到 dict 而非 `FileHeader` 实例。<br>**方案 A 的局限**：需在 `_dict_to_instance` 中递归转换嵌套 dict → 嵌套 dataclass。<br>详见 §8.4（嵌套处理）。 |

### 6.5 方案 B 设计预览（优化阶段，非 MVP）

若性能基准要求直构，新增 `PyDataclassSink`：

```rust
// construct-py/src/py_dataclass_sink.rs（Phase 14 优化阶段新增）
use construct::compiled::sink::{OutputSink, ProducedOutput};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyType};

/// OutputSink that writes parse results directly into a dataclass instance
/// via setattr, avoiding the PyDict intermediate.
///
/// Created with a target dataclass class. On first set_field, instantiates
/// the dataclass via cls.__new__(cls) (bypassing __init__), then setattr
/// each field. Nested dataclass fields create recursive PyDataclassSink.
pub struct PyDataclassSink {
    target_cls: Py<PyType>,
    instance: Option<Py<PyAny>>,  // lazily created on first set_field
    // ... (Struct mode tracking, similar to PyDictSink)
}

impl PyDataclassSink {
    pub fn new(target_cls: Py<PyType>) -> Self { /* ... */ }
}

impl OutputSink for PyDataclassSink {
    fn set_field(&mut self, name: &str, value: Value) -> Result<()> {
        Python::with_gil(|py| {
            // Lazy-create instance: cls.__new__(cls)
            if self.instance.is_none() {
                let inst = self.target_cls
                    .bind(py)
                    .call_method0("__new__")?;  // bypass __init__
                self.instance = Some(inst.unbind());
            }
            let py_obj = value_to_py(py, &value)?;
            self.instance.as_ref().unwrap()
                .bind(py)
                .setattr(name, py_obj)?;
            Ok(())
        })
    }
    // ... other methods
}
```

**holder 重载**（不破坏现有签名）：

```rust
// construct-py/src/compiled_ext.rs 新增方法（非修改现有）
impl CompiledSchemaHolder {
    /// Parse into a dataclass instance directly (no PyDict intermediate).
    /// Phase 14 optimization: uses PyDataclassSink.
    pub fn parse_bytes_into_dataclass(
        &self,
        py: Python<'_>,
        data: &[u8],
        target_cls: &Bound<'_, PyType>,
        kw: IndexMap<String, Value>,
    ) -> PyResult<PyObject> {
        let mut sink = PyDataclassSink::new(target_cls.clone().unbind());
        // ... exec_parse with this sink
    }
}
```

**性能预期**：省去 `PyDict` 构建（N 次 `set_item`）+ `cls(**dict)` 的 kwargs 展开，
预计 parse 密集场景提升 20-40%（需基准验证）。

---

## 7. direct-from-dataclass build（dataclass → PyInput）

### 7.1 现状：PyInput 已支持 attribute 访问

**关键发现**：`PyInput::get_field_bound`（`py_input.rs:60-73`）已实现 dict + attribute
双模式访问：

```rust
fn get_field_bound(&self, py: Python<'py>, name: &str)
    -> std::result::Result<Bound<'py, PyAny>, ConstructError>
{
    let bound = self.obj.bind(py);
    bound
        .get_item(name)           // 先试 dict 风格 __getitem__
        .or_else(|_| bound.getattr(name))  // 失败则 getattr（dataclass 属性）
        .map_err(|_| ConstructError::FieldMissing { /* ... */ })
}
```

这意味着 **build 方向对 dataclass 实例零改动即可工作**。dataclass 实例的属性访问走
`getattr` 路径（`get_item` 对普通对象会失败，自动 fallback 到 `getattr`）。

### 7.2 ConstructMixin.build 实现

```python
# ConstructMixin.build（§2.2 已展示，此处细化）
def build(self, **kw) -> bytes:
    holder = type(self)._compile()
    # 直接传 self（dataclass 实例）给 build_from_py
    # PyInput::from_bound(self) → get_field("magic") 走 getattr(self, "magic")
    return holder.build_from_py(self, **kw)
```

### 7.3 数据流详解

```
dataclass 实例 (self)
    │  header = Header(magic=b'MAGC', version=1, flags=2)
    │
    ▼  holder.build_from_py(self)
FFI 穿越：self 作为 PyObject 传入 Rust
    │
    ▼  PyInput::from_bound(self)
PyInput { obj: Py<Header instance> }
    │
    ▼  exec_build 遍历 CompiledNode::Struct
对每个字段（magic, version, flags）：
    │  CompiledStruct::exec_build 调用 input.get_field("magic")
    │
    ▼  PyInput::get_field("magic")
Python::with_gil → self.get_item("magic") 失败 → self.getattr("magic") 成功
    │  → py_to_value(b'MAGC') → Value::Bytes(...)
    │
    ▼  subcon.build(Value::Bytes(...), stream, ctx)
写入字节流
    │
    ▼  返回 bytes
```

**FFI 穿越分析**：
- 顶层进入：1 次（`build_from_py`）
- 每字段读取：1 次（`get_field` → `getattr`）—— 这是 Input trait 的细粒度 FFI，
  按需触发，只读消费的字段
- 嵌套 dataclass：`sub_input_field` 返回新 PyInput 包装嵌套实例，递归同上
- 顶层返回：1 次

**对比声明式 API build**：声明式 API 传 dict，`get_item` 成功（不走 getattr）。
dataclass API 传实例，`get_item` 失败 fallback 到 `getattr`。**仅多一次失败的
`get_item` 调用**（约几十纳秒），性能差异可忽略。

### 7.4 边界条件

| 场景 | 行为 |
|------|------|
| dataclass 实例字段值为 None（Optional 字段） | `PyInput::has_field` 对 None 返回 False（`py_input.rs:230-239` 测试），匹配 construct 的 `flagbuildnone` 语义 |
| dataclass 字段缺失（用户未赋值但有默认值） | `getattr` 返回默认值（dataclass `__init__` 已设），正常 build |
| dataclass 实例有额外属性（非 cs_field） | exec_build 只读取 Struct 定义的字段，忽略额外属性 |
| dataclass frozen=True（不可变） | `getattr` 仍可读（frozen 只禁止 setattr），build 正常 |
| 传入非 dataclass 实例（如普通 dict） | `get_item` 成功，走 dict 路径，正常 build（duck-typing） |

### 7.5 是否需要 Rust 改动？

**结论：build 方向零 Rust 改动**。PyInput 的双模式访问已完整覆盖 dataclass 场景。
唯一可能的微优化：对已知是 dataclass 的对象，跳过失败的 `get_item` 直接 `getattr`。
但这需要类型检测（`isinstance(obj, dataclass)` 的 Rust 等价），收益微小（省一次失败调用），
**不建议在 Phase 14 实现**。

---

## 8. 嵌套 dataclass 支持

### 8.1 场景定义

```python
@dataclass
class FileHeader(ConstructMixin):
    size: int = cs_field(Int32ul)
    flags: int = cs_field(Int32ul)

@dataclass
class FileFormat(ConstructMixin):
    header: FileHeader = cs_field(FileHeader)      # 嵌套 dataclass
    data: bytes = cs_field(Bytes(this.header.size))  # 表达式引用嵌套字段
```

关键挑战：
1. `cs_field(FileHeader)` 中 `FileHeader` 是 ConstructMixin 子类，不是普通 PyO3 构造器
2. 编译 `FileFormat` 时需递归编译 `FileHeader` 为 `CombinedConstruct::Struct`
3. parse 时 `header` 字段应产出 `FileHeader` 实例（不是 dict）—— 方案 A 的局限点
4. build 时 `file_format.header` 是 `FileHeader` 实例，`PyInput` 递归读取

### 8.2 嵌套 subcon 解析（编译期）

#### 8.2.1 `_resolve_nested_subcon` 辅助函数

> **B-FEAS-2 修正**：原 v1 设计的 `_resolve_subcon` 调用 `extract_subcon(subcon)` 返回
> `Box<CombinedConstruct>`（Rust 类型），Python 无法持有。v2 消除此调用：
> Python 端只产出 **Python 对象**（PyO3 wrapper 或 PyStruct），全部由 Rust 侧 `py_struct`
> 内部的 `extract_subcon` 统一处理。

```python
# construct-py/construct_rust/_fields.py 新增

# 线程局部集合，用于检测嵌套 dataclass 的循环引用（A → B → A）
import threading
_compiling_stack = threading.local()

def _resolve_nested_subcon(subcon, owner_cls):
    """Resolves a cs_field subcon for nesting inside a parent Struct.

    Returns a **Python object** suitable as a value in the ``Struct(**kwargs)``
    dict. The Rust-side ``py_struct`` will later call ``extract_subcon`` on it.

    Handles two cases:
    (a) subcon is a PyO3 construct wrapper (Int32ul, Bytes(4), PyStruct...)
        → returned as-is. ``py_struct`` → ``extract_subcon`` handles it
        on the Rust side.
    (b) subcon is a ConstructMixin subclass (nested dataclass)
        → recursively build a PyStruct via ``subcon._build_struct_decl()``
          and return that PyStruct. ``py_struct`` applies ``apply_rename``
          → ``make_renamed`` → ``extract_subcon(PyStruct)``. Since PyStruct
          is **not** in ``try_extract_registered``, ``extract_subcon`` falls
          back to duck-typing (PyStruct has ``parse_stream``/``build_stream``
          via ``impl_api_methods!``), wrapping it as
          ``Dynamic(PyConstructAdapter(PyRenamed(PyStruct)))`` — **not** an
          inlined ``CompiledNode::Struct``. See §8.2.3 for the full path
          and the MVP performance note.
    (c) subcon is None (forward reference placeholder)
        → raise TypeError; the caller (_compile's forward-ref branch, §8.3)
          handles it separately.
    """
    if subcon is None:
        raise TypeError(
            "cs_field(None) requires a forward-reference type annotation. "
            "Use cs_field(None) only with a string annotation like 'Node'."
        )
    # Case (b): ConstructMixin subclass (duck-typing on the class itself)
    if isinstance(subcon, type) and issubclass(subcon, ConstructMixin):
        # Cycle detection: guard against A → B → A infinite recursion
        stack = getattr(_compiling_stack, 'classes', None)
        if stack is None:
            stack = set()
            _compiling_stack.classes = stack
        if subcon in stack:
            raise RecursionError(
                f"Circular nested dataclass reference: "
                f"{subcon.__name__} appears in its own nesting chain "
                f"({' -> '.join(c.__name__ for c in stack)} -> {subcon.__name__}). "
                f"Use cs_field(None) with a forward-reference annotation for "
                f"recursive structures (see §8.3)."
            )
        stack.add(subcon)
        try:
            return subcon._build_struct_decl()   # → PyStruct (recursive)
        finally:
            stack.discard(subcon)
    # Case (a): regular PyO3 construct wrapper — pass through unchanged
    return subcon
```

**关键变更说明（v1 → v2）**：
- **删除**了 `from construct_rust._core import extract_subcon`（不可暴露的 Rust 函数）
- **删除**了 case (a) 中 `return extract_subcon(subcon)`，改为 `return subcon`（原样透传）
- subcon → Rust 类型的转换**全部下沉**到 `py_struct` 的 Rust 实现中

#### 8.2.2 `_build_struct_decl` 方法（ConstructMixin 新增）

```python
class ConstructMixin:
    # ... (§2.2 方法)

    @classmethod
    def _build_struct_decl(cls):
        """Builds a PyStruct declaration (not yet compiled) from this
        dataclass's fields. Used when this class is nested inside another.

        Returns a PyStruct instance (Python object). The parent's
        ``_collect_field_subcons`` passes this PyStruct as a value in the
        ``Struct(**kwargs)`` dict; ``py_struct`` then ``apply_rename``s it
        and calls ``extract_subcon``. Since PyStruct is not in
        ``try_extract_registered``, it falls back to duck-typing and is
        wrapped as ``Dynamic(PyConstructAdapter(PyRenamed(PyStruct)))`` —
        **not** an inlined ``CompiledNode::Struct`` (see §8.2.3 for the
        full path and the MVP performance note).

        This method does NOT compile — it only assembles the declaration.
        Compilation happens once, at the parent level.
        """
        subcons_kw = _collect_field_subcons(cls)  # recursive for nesting
        return Struct(**subcons_kw)                # PyStruct (Rust-backed)
```

**关键点**：
- `_build_struct_decl` 返回**未编译的** `PyStruct`（Python 对象），不是 `CompiledSchemaHolder`
- 父类 `_compile` 调用 `compile_schema(parent_struct)`，编译器递归处理嵌套 Struct
- 嵌套 PyStruct 在父 schema 的执行树中表现为 `Dynamic(PyConstructAdapter(...))` 节点（**非内联** `CompiledNode::Struct`，详见 §8.2.3），无独立缓存

#### 8.2.3 编译流程（含嵌套）—— N-1 复检修正

> **N-1 复检修正（v3）**：v1/v2 均描述"嵌套 dataclass 内联为 `CompiledNode::Struct`"。
> 经 REV v2 复检逐行核实代码，**此描述与代码事实不符**。真实路径如下：
>
> `py_struct` 的 kwargs 循环（`constructs_composite.rs:227-233`）对每个 value 调用
> `apply_rename` → `make_renamed`（`py_renamed.rs:175,86-104`），后者内部调用
> `extract_subcon(subcon_obj)`（`py_renamed.rs:93`）。当 `subcon_obj` 是 PyStruct 时：
>
> 1. `extract_subcon`（`py_adapter.rs:417-441`）先调用 `try_extract_registered(obj)`
>    （`constructs_adapter.rs:1315-1457`）——**PyStruct 不在注册列表中**
>    （连 PyRenamed 都被明确排除，见 `line 1451-1454` 注释），返回 `None`。
> 2. 回退到 duck-typing 检查（`py_adapter.rs:426-430`）：PyStruct 通过
>    `impl_api_methods!` 宏（`constructs_composite.rs:269` → `construct_macros.rs:29-172`）
>    注入了 `parse_stream` / `build_stream` 方法，**duck-typing 成立**。
> 3. 因此 `extract_subcon(PyStruct)` 返回
>    `Dynamic(PyConstructAdapter(PyStruct))`，被 `Renamed` 包裹后再 `extract_subcon`
>    同理得到 `Dynamic(PyConstructAdapter(PyRenamed(PyStruct)))`。
>
> **最终结论**：嵌套 PyStruct 在父 Struct 的编译树中表示为
> **`Dynamic(PyConstructAdapter(PyRenamed(PyStruct)))`** 节点，**不是**内联的
> `CompiledNode::Struct`。v1/v2 的"内联编译"描述是错误的。

```
FileFormat._compile()
    │
    ▼  _collect_field_subcons(FileFormat)
    │  header: _get_field_subcon → FileHeader (ConstructMixin 子类)
    │    └─ _resolve_nested_subcon(FileHeader, FileFormat)
    │         └─ issubclass(FileHeader, ConstructMixin) → FileHeader._build_struct_decl()
    │              └─ _collect_field_subcons(FileHeader)
    │                   └─ {size: Int32ul, flags: Int32ul}
    │              └─ Struct(size=Int32ul, flags=Int32ul) → PyStruct(FileHeader)
    │         └─ 返回 PyStruct（Python 对象）
    │  data: _get_field_subcon → Bytes(this.header.size) (PyO3 wrapper)
    │    └─ _resolve_nested_subcon → 原样透传（case a）
    │
    ▼  subcons_kw = {header: <FileHeader PyStruct>, data: <Bytes>}
    │
    ▼  Struct(**subcons_kw)  ← py_struct（Rust 侧）
    │  py_struct kwargs 循环（constructs_composite.rs:227-233）对每个 (key, val) 执行：
    │    renamed = apply_rename(val, name)            # py_renamed.rs:175 → make_renamed
    │    └─ make_renamed 内部调用 extract_subcon(val)  # py_renamed.rs:93
    │         └─ [header] extract_subcon(PyStruct):
    │              (1) try_extract_registered(PyStruct) → None（PyStruct 未注册）
    │              (2) duck-typing 检查 parse_stream/build_stream（impl_api_methods! 注入）→ 成立
    │              (3) → Dynamic(PyConstructAdapter(PyStruct))
    │         └─ Renamed{inner: Dynamic(...), name:"header"} → PyRenamed
    │    inner = extract_subcon(renamed=PyRenamed)    # constructs_composite.rs:230
    │         └─ try_extract_registered(PyRenamed) → None（明确排除，见 line 1451-1454）
    │         └─ duck-typing → Dynamic(PyConstructAdapter(PyRenamed(PyStruct)))
    │    builder.field("header", Dynamic(PyConstructAdapter(PyRenamed(PyStruct))))
    │
    │    [data] 同理（PyBytes 先经 try_extract_registered 提取 inner，再被 Renamed 包裹，
    │           最终 extract_subcon(PyRenamed) 仍走 Dynamic）：
    │         → Dynamic(PyConstructAdapter(PyRenamed(Bytes)))
    │
    ▼  compile_schema(parent_struct)
    │  编译树：CompiledNode::Struct { fields: [
    │       ("header", CompiledNode::Dynamic { adapter: PyConstructAdapter(PyRenamed(PyStruct)) }),
    │       ("data",   CompiledNode::Dynamic { adapter: PyConstructAdapter(PyRenamed(Bytes)) })
    │  ]}
    │  ⚠ header 是 Dynamic 节点，不是内联的 CompiledNode::Struct。
    │    （不仅嵌套 dataclass，所有通过 **kwargs 传入的字段都走 Dynamic 路径，
    │     因为 apply_rename 总是产出 PyRenamed，而 PyRenamed 不在 try_extract_registered 中。）
    │
    ▼  CompiledSchemaHolder (缓存到 FileFormat._compiled)
```

**N-1 复检结论**：嵌套 PyStruct（以及所有通过 kwargs 传入的字段）在编译树中表示为
**`CompiledNode::Dynamic(PyConstructAdapter(PyRenamed(...)))`**，**不是**内联的
`CompiledNode::Struct`。v1/v2 的"内联编译"描述与代码事实不符，已修正。

**功能影响：无。** `PyConstructAdapter` 完整实现了 `Construct` trait 的
parse/build/sizeof（`py_adapter.rs:195-404`）。parse 时通过 BytesIO round-trip 调用
`PyRenamed.parse_stream` → `Renamed.parse` → inner（`Dynamic(PyStruct)`）的
`PyConstructAdapter.parse`，其 fast path（`py_adapter.rs:202-204`）检测到 py_obj 是
PyStruct → 委托给 `PyStruct.inner.parse`（即 Rust `Struct::parse`），最终产出正确结果。
build 方向同理。`build(parse(data)) == data` 对称性不受影响。

**性能影响（MVP 可接受）：** 嵌套字段每次 parse/build 需穿越 GIL 边界 + BytesIO
round-trip（读取全部剩余字节到 Python BytesIO → 调用 Python 方法 → 通过 `tell()`
同步流位置），无法享受 Rust 编译内联的零开销优势。对于浅层结构（1-2 层嵌套），
开销以微秒计，可忽略；深层嵌套或高频解析场景下会成为热点。

> **MVP 方案标注**：当前 kwargs 路径（`apply_rename` → `Dynamic`）是 Phase 14 的
> 可接受折中。后续优化可从两个方向改善：
> - **方案 B（PyDataclassSink，§6.5）**：parse 方向直构 dataclass 实例，省去 dict 中转；
>   但**不改变**编译树的 Dynamic 节点结构（仍是 Python 回调）。
> - **Rust 内联（根本性优化）**：在 `try_extract_registered` 中注册 PyStruct/PyRenamed，
>   使 `extract_subcon` 直接提取 inner Rust Struct（绕过 `Dynamic`/`PyConstructAdapter`），
>   实现零开销内联。需谨慎处理 `Renamed` 的 name 语义与 `make_owned` 生命周期，
>   建议作为 Phase 14 后续或 Phase 15 性能优化阶段的独立子任务评估。

### 8.3 前向引用（字符串注解）

#### 8.3.1 场景

```python
@dataclass
class Node(ConstructMixin):
    value: int = cs_field(Int32ul)
    next: "Optional[Node]" = cs_field(None)  # 前向引用 + Optional
```

`"Optional[Node]"` 在类定义时无法解析（`Node` 尚未绑定）。`cs_field(None)` 占位，
延迟到首次 parse/build 时解析。

#### 8.3.2 延迟解析机制

> **I-COMP-1 修正**：`from __future__ import annotations`（PEP 563）下所有类型注解
> 变为字符串。`typing.get_type_hints` 需要正确的 module globals 才能解析这些字符串。
> v2 显式传入 `globalns`（从 `cls.__module__` 对应的 `sys.modules` 获取）。

```python
class ConstructMixin:
    @classmethod
    def _compile(cls):
        if cls._compiled is not None:
            return cls._compiled
        import dataclasses
        import typing
        import sys
        # Resolve type hints (handles string annotations / forward refs).
        # Under `from __future__ import annotations`, all annotations are
        # strings. typing.get_type_hints evaluates them using the module's
        # globals — we must pass globalns explicitly to ensure the module
        # context is correct (I-COMP-1 fix).
        module = sys.modules.get(getattr(cls, '__module__', None))
        globalns = getattr(module, '__dict__', {}) if module else None
        try:
            hints = typing.get_type_hints(cls, globalns=globalns)
        except Exception:
            # Fallback: raw annotations dict (may contain unresolved strings)
            hints = getattr(cls, '__annotations__', {})

        subcons_kw = {}
        for f in dataclasses.fields(cls):
            subcon = _get_field_subcon(f)
            if subcon is None:
                # Forward reference: resolve from type annotation
                subcon = _resolve_forward_ref(f.name, f.type, hints, cls)
            subcon = _resolve_nested_subcon(subcon, cls)
            subcons_kw[f.name] = subcon
        struct_decl = Struct(**subcons_kw)
        cls._compiled = compile_schema(struct_decl)
        return cls._compiled
```

**I-COMP-1 修复要点**：
- 显式传入 `globalns=getattr(module, '__dict__', {})`，确保 `typing.get_type_hints` 能在
  `from __future__ import annotations` 下正确解析字符串注解（如 `"Node"` → 实际的 `Node` 类）
- 若模块未加载（`module is None`），传 `globalns=None`，`get_type_hints` 使用 `cls` 的
  `__globals__`（对在函数内动态定义的类有效）
- 异常时 fallback 到原始 `__annotations__`（可能含未解析字符串，后续 `_resolve_forward_ref` 会报错）

#### 8.3.3 `_resolve_forward_ref`

```python
def _resolve_forward_ref(field_name, raw_type, hints, cls):
    """Resolves a forward-reference type annotation to a ConstructMixin subclass.

    Supported forms:
    - "Node" (bare string) → look up Node in cls.__module__ globals
    - Optional[Node] → extract Node, wrap with Optional() construct
    - typing.get_type_hints already evaluated hints dict (uses globalns)

    Args:
        field_name: The dataclass field name (for error messages).
        raw_type: The raw annotation (may be a string under PEP 563).
        hints: The resolved type hints dict from typing.get_type_hints.
        cls: The owning class (for module context).
    """
    # hints is the evaluated type hints dict (resolved via globalns)
    resolved = hints.get(field_name)
    if resolved is None:
        raise NameError(
            f"Cannot resolve forward reference for field '{field_name}' "
            f"in {cls.__name__}: annotation '{raw_type}' could not be "
            f"evaluated. Ensure the referenced class is defined in the "
            f"same module or imported."
        )
    # ... extract ConstructMixin subclass, handle Optional[...], etc.
```

**注意**：前向引用解析较复杂（需处理 `Optional[X]`、`Union[X, None]`、`List[X]` 等），
建议作为 §10 子任务 14.3 的独立工作项，MVP（14.1）不支持前向引用。

### 8.4 parse 方向的嵌套实例化（方案 A 的局限）

#### 8.4.1 问题

方案 A（PyDict 中转）下，嵌套 dataclass 字段 parse 后得到的是**嵌套 dict**，不是嵌套
dataclass 实例：

```python
# FileFormat.parse(data) 的方案 A 流程：
holder.parse_bytes_py(data) → {
    'header': {'size': 4, 'flags': 1},   # 嵌套 dict，不是 FileHeader 实例！
    'data': b'...'
}
FileFormat(**result) → FileFormat(header={'size': 4, 'flags': 1}, data=b'...')
# header 字段类型注解是 FileHeader，但实际值是 dict —— 类型不匹配
```

#### 8.4.2 解决方案：Python 层递归转换（MVP 方案 A1）

> **v2 修正**：v1 曾设想在 `_compile` 时用 `_DataclassAdapter` 包装嵌套 dataclass 字段，
> 在 Rust 执行树内完成 dict→实例转换。但此方案不可行：
> 1. `_DataclassAdapter` 需要持有 Rust subcon（`_rust_subcon`），但 Python 无法持有
>    `Box<CombinedConstruct>`（B-FEAS-2）
> 2. Phase 13 执行树是 direct-to-PyDict（`PyDictSink`），没有"parse 后转换"的拦截点
>
> 因此方案 A 下嵌套 dataclass 的 dict→实例转换**只能在 Python 层后处理**（`_dict_to_instance`，
> 见 §8.4.2 续）。

方案 A（PyDict 中转）下，嵌套 dataclass 字段 parse 后得到的是**嵌套 dict**。
两种 Python 层后处理方案：

| 子方案 | 机制 |
|--------|------|
| A1 递归 kwargs 转换 | `cls(**result)` 前，递归把嵌套 dict 转为嵌套 dataclass |
| A2 等方案 B（PyDataclassSink） | 优化阶段用直构 sink，天然支持嵌套 |

**MVP 选 A1**（递归 kwargs 转换）：

> **I-COMP-1 / I-COMP-2 修正**：`_dict_to_instance` 需要：
> 1. 正确解析类型提示（`from __future__ import annotations` 下传入 `globalns`）
> 2. 基于 `dataclasses.fields()` 白名单过滤多余键（而非下划线前缀）

```python
    @classmethod
    def _dict_to_instance(cls, d: dict) -> "ConstructMixin":
        """Recursively converts nested dicts to nested dataclass instances,
        based on field type annotations.

        I-COMP-1: Uses typing.get_type_hints with explicit globalns to
        resolve string annotations under `from __future__ import annotations`.
        I-COMP-2: Filters dict keys against dataclasses.fields(cls) whitelist
        (not underscore-prefix convention).
        """
        import dataclasses, typing, sys
        # Resolve type hints with module globals (I-COMP-1)
        module = sys.modules.get(getattr(cls, '__module__', None))
        globalns = getattr(module, '__dict__', {}) if module else None
        try:
            hints = typing.get_type_hints(cls, globalns=globalns)
        except Exception:
            hints = getattr(cls, '__annotations__', {})

        # Build whitelist from dataclasses.fields (I-COMP-2)
        field_names = {f.name for f in dataclasses.fields(cls)}
        kwargs = {}
        for fname in field_names:              # iterate whitelist, not dict keys
            if fname not in d:
                continue                       # rely on dataclass default
            val = d[fname]
            ftype = hints.get(fname)
            # If field type is a ConstructMixin subclass and val is dict → recurse
            if (isinstance(ftype, type)
                    and issubclass(ftype, ConstructMixin)
                    and isinstance(val, dict)):
                val = ftype._dict_to_instance(val)
            kwargs[fname] = val
        return cls(**kwargs)
```

**I-COMP-2 关键变更**：原 v1 遍历 `d` 的键并用下划线前缀过滤。v2 改为遍历
`dataclasses.fields(cls)` 的字段名（白名单），从 `d` 中取对应值。这样：
- Struct 的匿名字段产出（如 `_compiled_schema` 等内部键）自然被忽略（不在白名单）
- 用户合法的 `_internal` 字段不会被误过滤（只要它在 dataclass 中声明）

**性能**：递归转换有 Python 层开销，但 MVP 可接受。方案 B（PyDataclassSink）优化阶段消除。

### 8.5 build 方向的嵌套（已天然支持）

build 方向无此问题。`PyInput::sub_input_field` 返回包装嵌套实例的新 PyInput，递归读取属性：

```python
file_format = FileFormat(header=FileHeader(size=4, flags=1), data=b'...')
file_format.build()
# PyInput(file_format).sub_input_field("header") → PyInput(file_format.header)
# → get_field("size") → getattr(file_format.header, "size") = 4
```

**build 方向嵌套零特殊处理**，PyInput 的递归 sub_input 机制天然覆盖。

---

## 9. REV #6 回应：PyO3 wrapper 的 `__parse__`/`__build__` 方法

### 9.1 质疑内容

> 用户期望 `MyStruct.parse(data)` 和 `my_struct.build()` 作为方法调用。需要在 PyO3
> wrapper 或 Python mixin 中提供这些方法。

### 9.2 回应：确认设计已解决，方法在 Python mixin 层

**本设计已完整解决 REV #6**。方法注入位置：**纯 Python ConstructMixin 基类**（§2.2），
不在 PyO3 wrapper（Rust）层。

**理由**：

1. **ConstructMixin 是 Python 类，方法即用户 API**：
   - `Header.parse(data)` 是 ConstructMixin 的 `@classmethod`，用户直接调用
   - `header.build()` 是 ConstructMixin 的实例方法，用户直接调用
   - 这些方法**已经是** `__parse__`/`__build__` 的等价物，用户可见、可调用

2. **不在 PyO3 wrapper 层的原因**：
   - PyO3 wrapper（`PyStruct` 等）是**声明式 API** 的载体（`Struct(...).parse()`），
     已有 `parse`/`build` 方法（`impl_api_methods!` 宏注入，`construct_macros.rs:34-156`）
   - dataclass API 的 `parse`/`build` 语义不同（parse 返回 dataclass 实例，不是 dict；
     build 读实例属性），与声明式 API 的 `parse`/`build` **签名不同**，不能复用 PyO3 wrapper 方法
   - 在 PyO3 层为 dataclass 注入方法需要 metaclass 或 `__init_subclass__` 钩子（Rust 侧），
     复杂度高且违反 §4.4 排除 metaclass 的决策

3. **方法分发路径**：

```
Header.parse(data)          # 用户调用
    │
    ▼  ConstructMixin.parse (classmethod, 纯 Python, §2.2)
    │
    ▼  cls._compile() → CompiledSchemaHolder（首次懒编译，后续缓存）
    │
    ▼  holder.parse_bytes_py(data)  ← FFI 穿越（Phase 13 Rust 方法）
    │
    ▼  返回 dict（MVP）或 dataclass 实例（优化）
    │
    ▼  cls(**dict) → Header 实例（Python 层实例化）
    │
    ▼  返回给用户
```

4. **FFI 边界清晰**：
   - Python 层（ConstructMixin）：方法定义、字段扫描、dict→实例转换
   - Rust 层（CompiledSchemaHolder）：编译、parse/build 执行（复用 Phase 13）
   - 新增 Rust 暴露层（§9.4）：`CompiledSchemaHolder` pyclass + `compile_schema` pyfunction

### 9.3 验证：用户 API 完整性

| 用户操作 | 方法 | 定义位置 | 实现路径 |
|---------|------|---------|---------|
| `Header.parse(data)` | classmethod | ConstructMixin（Python） | → `_compile` + `parse_bytes_py` + `cls(**dict)` |
| `Header.parse_stream(stream)` | classmethod | ConstructMixin（Python） | → `stream.read()` + `parse` |
| `Header.parse_file(path)` | classmethod | ConstructMixin（Python） | → `open().read()` + `parse` |
| `header.build()` | instance method | ConstructMixin（Python） | → `_compile` + `build_from_py(self)` |
| `header.build_stream(stream)` | instance method | ConstructMixin（Python） | → `build()` + `stream.write()` |
| `header.build_file(path)` | instance method | ConstructMixin（Python） | → `open().write(build())` |
| `Header.sizeof(**kw)` | classmethod | ConstructMixin（Python） | → `_compile` + `holder.sizeof(**kw)` |

**结论**：REV #6 的所有方法需求（parse/build/parse_stream/build_stream 等）均在 ConstructMixin
中定义，用户可直接调用。无需 PyO3 wrapper 改动。

### 9.4 依赖项暴露状态（v2 —— 逐行对照代码核实）

> **B-FEAS-3 修正**：v1 表格存在多处事实性错误。v2 逐行对照源码核实每一项的实际暴露状态。

ConstructMixin 的 Python 实现依赖 Rust 接口暴露到 Python。以下是对照源码的核实结果：

| Rust 项 | Python 名 | 实际暴露状态（源码核实） | Phase 14 动作 |
|---------|----------|----------------------|--------------|
| `CompiledSchemaHolder` | `CompiledSchemaHolder` | ❌ **未暴露**。`compiled_ext.rs:60` 是普通 `pub struct`，无 `#[pyclass]`；`lib.rs:register_classes`（143-229 行）未注册它 | **新增** `#[pyclass(name = "CompiledSchemaHolder")]`，并在 `register_classes` 中 `m.add_class::<CompiledSchemaHolder>()` |
| `CompiledSchemaHolder::parse_bytes_py` | `.parse_bytes_py(data, **kw)` | ❌ **不是 pymethod**。`compiled_ext.rs` 中 `CompiledSchemaHolder` 没有 `#[pymethods]` impl 块，该方法是普通 `pub fn` | **新增** `#[pymethods]` impl 块，注册 `parse_bytes_py` 为 pymethod（签名需适配 Python：`&self, py, data: &[u8], **kw`） |
| `CompiledSchemaHolder::build_from_py` | `.build_from_py(obj, **kw)` | ❌ **不是 pymethod**（同上） | **新增** 注册为 pymethod |
| `CompiledSchemaHolder::sizeof` | `.sizeof(**kw)` | ❌ **不存在**。`CompiledSchemaHolder` 没有 `sizeof` 方法，需通过 `self.schema().static_size()` 转发 | **新增** pymethod `sizeof`，内部调用 `self.schema().static_size()` |
| `api::compile_schema` | `compile_schema(struct)` | ❌ **未暴露**。`api.rs:361` 是普通 `pub fn`，返回 `Arc<CompiledSchemaHolder>`（Rust 类型）；`lib.rs:register_functions`（231-326 行）未注册 | **新增** pyfunction 包装：接受 PyStruct，内部调用 `extract_subcon` 提取 `CombinedConstruct`，调用 `CompiledSchemaHolder::compile`，返回 pyclass 实例 |
| `py_struct`（`Struct` 工厂） | `Struct(**subconskw)` | ✅ **已暴露**（Phase 10）。`lib.rs:255` 注册；`constructs_composite.rs:215` 签名 `(*subcons, **subconskw)` | 无需改动 |
| `PyStruct.field()` | `.field(name, subcon)` | ❌ **不存在**。`PyStruct` 的 `#[pymethods]`（`constructs_composite.rs:241-267`）只有 `__getattr__` + `__repr__` + 宏注入的 api methods，**没有** `.field()` builder 方法 | **不新增**。v2 改用 `Struct(**kwargs)` 一次性构建（零 Rust 改动） |
| `extract_subcon` | `extract_subcon(wrapper)` | ❌ **不可暴露**。`py_adapter.rs:417` 返回 `Box<CombinedConstruct>`（Rust 类型），Python 无法持有 | **不暴露**。v2 消除 Python 层对它的所有调用，subcon 提取下沉到 `py_struct` 内部 |

**Phase 14 的 Rust 改动清单（v2 修正版）**：

1. **`CompiledSchemaHolder` 注册为 pyclass**（`compiled_ext.rs`）：
   - 添加 `#[pyclass(name = "CompiledSchemaHolder")]`
   - 添加 `#[pymethods]` impl 块，暴露 `parse_bytes_py`、`build_from_py`、`sizeof`
   - 在 `lib.rs:register_classes` 中注册
2. **新增 pyfunction `compile_schema`**（`api.rs` 或 `compiled_ext.rs`）：
   - 签名：`#[pyfunction] fn compile_schema(py, struct_decl: PyRef<PyStruct>) -> PyResult<Py<CompiledSchemaHolder>>`
   - 内部：`struct_decl.inner` → `CombinedConstruct::Struct` → `CompiledSchemaHolder::compile`
   - 在 `lib.rs:register_functions` 中注册

这些都是**纯暴露层**改动（加 `#[pyclass]`/`#[pyfunction]`/`#[pymethods]` 注解），
不改 Rust 业务逻辑。`parse_bytes_py`/`build_from_py` 的现有实现完全复用。

> **注意**：`parse_bytes_py`/`build_from_py` 现有签名接受 `IndexMap<String, Value>` 作为 kw。
> 暴露为 pymethod 时，需用 `**kwargs` → `pykw_to_indexmap` 转换（`api.rs:52` 已有此辅助函数），
> 或直接接受 `Option<&Bound<PyDict>>` 参数。

---

## 10. 子任务拆分建议

基于上述设计，Phase 14 建议拆分为 **4 个子任务**，按依赖顺序：

### 10.1 子任务 14.1：ConstructMixin + cs_field + 方案 A MVP（核心）

**目标**：实现扁平 dataclass 的 parse/build，验证 API 正确性。

**范围**：
- 新增 `construct-py/construct_rust/_mixins.py`（ConstructMixin 纯 Python 类）
- 新增 `construct-py/construct_rust/_fields.py`（cs_field + _get_field_subcon + _collect_field_subcons + _resolve_nested_subcon）
- Rust 改动（§9.4 v2 清单）：
  - `CompiledSchemaHolder` 注册为 pyclass（`#[pyclass]` + `m.add_class`）
  - `parse_bytes_py` / `build_from_py` / `sizeof` 注册为 `#[pymethods]`
  - 新增 pyfunction `compile_schema(struct) -> CompiledSchemaHolder`
  - **不需要**暴露 `extract_subcon`（B-FEAS-2：Python 不调用它）
  - **不需要**给 PyStruct 添加 `.field()` 方法（B-FEAS-1：用 `Struct(**kwargs)`）

**出口标准**：
- `Header.parse(data)` 返回 Header 实例（扁平字段）
- `header.build()` 返回 bytes
- 往返：`build(parse(data)) == data`
- `Header.sizeof()` 返回正确值
- `dataclasses.fields(Header)` 正常工作（cs_field 不破坏 dataclass 协议）
- 单元测试：≥10 个用例（扁平 Struct、多字段、不同类型、往返、错误路径）

**不含**：嵌套 dataclass、前向引用、Optional、@construct_dataclass 语法糖

### 10.2 子任务 14.2：嵌套 dataclass 支持

**目标**：支持 dataclass 字段类型为另一个 ConstructMixin 子类。

**范围**：
- 实现 `_resolve_nested_subcon`（识别 ConstructMixin 子类，递归 `_build_struct_decl`，循环引用检测）
- 实现 `_build_struct_decl`（ConstructMixin 新增方法，返回未编译 PyStruct，用 `Struct(**kwargs)`）
- 实现 `_dict_to_instance`（parse 方向递归 dict→嵌套实例转换，方案 A1；含 I-COMP-1/I-COMP-2 修正）
- build 方向天然支持（PyInput sub_input_field），验证即可

**出口标准**：
- 3 层嵌套 dataclass parse/build 往返正确
- 嵌套字段的 `this.header.size` 表达式求值正确（依赖 Phase 5 表达式系统）
- `dataclasses.asdict(nested_instance)` 正确递归（验证 dataclass 协议完整）
- 单元测试：≥8 个用例（2层/3层嵌套、混合嵌套与普通字段、表达式引用嵌套字段、往返）

### 10.3 子任务 14.3：前向引用 + Optional + 边界情况

**目标**：支持自引用/互引用结构（链表、树）、Optional 字段。

**范围**：
- 实现 `_resolve_forward_ref`（基于 `typing.get_type_hints` 解析字符串注解）
- 支持 `Optional[X]`（映射到 construct 的 `Optional(X)` 构造器）
- 循环引用检测（编译时检测 `A → B → A`，抛 `RecursionError` 带）
- 边界：空 dataclass（无字段）、全 Optional dataclass

**出口标准**：
- 链表结构 `Node(value=int, next=Optional[Node])` parse/build 正确
- 互引用 `A(b=Optional[B])` / `B(a=Optional[A])` 正确
- 循环引用（非 Optional 的直接循环）抛清晰错误
- 单元测试：≥6 个用例（链表、树、互引用、Optional、空 dataclass、循环引用错误）

### 10.4 子任务 14.4：@construct_dataclass 语法糖 + 互操作 + 性能基线

**目标**：完善 API 表面，验证与声明式 API 互操作，建立性能基线。

**范围**：
- 实现 `@construct_dataclass` 装饰器（§4.3 方案 B 语法糖）
- 互操作测试：ConstructMixin 子类作为 `Struct("inner"/MyClass)` 的子构造器
- 互操作测试：声明式 `Struct` 的 parse 结果传入 dataclass build
- 性能基线：建立 `benches/dataclass_api_bench.rs`（对标方案 A）
- 文档：用户指南（README 或 docs/）

**出口标准**：
- `@construct_dataclass` 与 `@dataclass + ConstructMixin` 行为等价
- 互操作：声明式 Struct 可嵌套 ConstructMixin 子类，反之亦然
- 性能基线：parse/build 往返延迟记录（方案 A），为后续方案 B 优化提供对比基准
- 单元测试：≥10 个用例（装饰器、互操作 4 方向、frozen dataclass、继承）

### 10.5 可选子任务 14.5：方案 B 直构优化（PyDataclassSink）

**目标**：性能优化，parse 直构 dataclass 实例（无 PyDict 中转）。

**范围**：
- 新增 `construct-py/src/py_dataclass_sink.rs`（PyDataclassSink，实现 OutputSink）
- `CompiledSchemaHolder::parse_bytes_into_dataclass` 新方法（非破坏性新增）
- ConstructMixin.parse 检测优化可用性，优先用直构路径
- 性能对比：方案 A vs 方案 B，验证 ≥30% 提升（§8.2.8 [P] 标准）

**出口标准**：
- PyDataclassSink 通过 OutputSink trait 全部测试
- 嵌套 dataclass 直构正确（递归 sink）
- 性能基准：parse 密集场景（100 字段 Struct × 1000 次）方案 B 比方案 A 快 ≥30%

**时机**：**Phase 14.1-14.4 完成后**，视性能基准结果决定是否启动。若方案 A 性能已
满足 S-PERF-1（≥1.0x），可推迟到 Phase 15。

### 10.6 子任务依赖图

```
14.1 (MVP 核心) ──────┬──► 14.2 (嵌套)
                      │
                      └──► 14.3 (前向引用/Optional)
                               │
                               ▼
                           14.4 (语法糖/互操作/基线)
                               │
                               ▼ (可选，视性能)
                           14.5 (方案 B 直构优化)
```

- 14.1 是所有后续任务的基础（ConstructMixin 框架）
- 14.2 和 14.3 可并行（不同扩展维度），但都依赖 14.1
- 14.4 依赖 14.2/14.3（互操作需嵌套 + 前向引用就绪）
- 14.5 是纯优化，独立于功能完整性

---

## 附录 A：与顶层架构 §8.2.8 验收标准的映射

| 验收标准（§8.2.8） | 本设计对应章节 | 实现子任务 |
|-------------------|--------------|-----------|
| `[I]` from_bytes/to_bytes/sizeof/cs_field | §2.2, §3 | 14.1 |
| `[I]` Container 对用户完全不可见 | §1.2 P1, §6 | 14.1 (MVP), 14.5 (直构) |
| `[I]` 嵌套自动递归解析 | §8 | 14.2 |
| `[I]` Optional/前向引用支持 | §8.3 | 14.3 |
| `[I]` 与声明式 API 互操作 | §5.3, §10.4 | 14.4 |
| `[T]` 扁平 dataclass | §6 | 14.1 |
| `[T]` 嵌套 dataclass（3层） | §8 | 14.2 |
| `[T]` Optional 字段 | §8.3 | 14.3 |
| `[T]` 前向引用（字符串注解） | §8.3 | 14.3 |
| `[T]` 与声明式 Struct 互操作 | §10.4 | 14.4 |
| `[T]` 往返对称性 | §1.4 C4 | 全部子任务 |
| `[T]` 边界：空 dataclass/全Optional/循环引用 | §8.3, §10.3 | 14.3 |
| `[P]` 直构比 dict 中转快 ≥30% | §6.5, §10.5 | 14.5（可选） |

## 附录 B：设计决策摘要（供 PM/DEV 快速查阅）

| 决策 | 选择 | 理由摘要 |
|------|------|---------|
| 装饰器方案 | A（继承 ConstructMixin）为主，B（@construct_dataclass）为辅 | 与标准 dataclass 最大兼容；排除 metaclass（§4.4）。B 用 `type()` 动态创建子类（I-COMP-3） |
| direct-to-dataclass parse | A（PyDict 中转）为 MVP，B（PyDataclassSink）为优化 | A 零 Rust 改动立即可用；B 性能优化视基准启动 |
| direct-from-dataclass build | 复用现有 PyInput（attribute 访问已支持） | 零 Rust 改动；PyInput 双模式天然覆盖 dataclass |
| Struct 构建方式 | `Struct(**subcons_kw)` 关键字形式（非 `.field()` builder） | PyStruct 无 `.field()` pymethod（B-FEAS-1）；`py_struct` 支持 `**subconskw` |
| subcon 提取位置 | Rust 侧 `py_struct` 内部（非 Python 层） | `extract_subcon` 返回 Rust 类型不可暴露（B-FEAS-2） |
| 嵌套 dataclass 编译 | `_build_struct_decl` 递归构建 PyStruct → 内联 `CompiledNode::Struct` | 父 compile 递归处理，嵌套内联在父执行树（I-FEAS-1 确认） |
| 嵌套 parse 实例化 | MVP 用 A1（递归 kwargs 转换）；优化用方案 B | A1 在 Python 层递归；B 用 PyDataclassSink 递归 sink |
| 多余键过滤 | `dataclasses.fields()` 白名单（非下划线前缀） | 避免误过滤用户合法的 `_` 前缀字段（I-COMP-2） |
| 类型提示解析 | `typing.get_type_hints(cls, globalns=...)` | 处理 `from __future__ import annotations`（I-COMP-1） |
| 前向引用 | 懒编译 + typing.get_type_hints + globalns | 首次 parse/build 才解析，规避类定义时未绑定问题 |
| 方法注入位置 | 纯 Python ConstructMixin（非 PyO3 wrapper） | 避免 metaclass；签名与声明式 API 不同需独立实现 |
| Rust 改动范围 | 仅暴露层（pyclass/pyfunction/pymethods 注册） | 不改 Rust 逻辑；CompiledSchemaHolder 现有方法签名不变 |

---

**文档结束（v2）**。v2 已修正 REV 提出的全部 3 个阻塞级 + 4 个重要级问题。
下一步：PM 分派 DESIGN_REVIEW（v2 复检），或直接进入 CODING（若 PM 判定 v2 可接受）。


