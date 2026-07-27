---
id: DESIGN-Mixin
status: active
phase: "0"
depends_on: []
supersedes: []
superseded_by: []
last_updated: 2026-07-27
---

# Dataclass Mixin 自动编译设计原则

> 本文档定义 construct-rs 项目中"用户定义 dataclass 子类时自动完成编译，用户无感"这一模式的设计原则。
> 文档自包含——读者无需阅读任何参考项目源码即可完全理解全部内容。

---

## 1. 文档目的与适用范围

### 1.1 要解决的问题

construct-rs 的用户 API 目标是：用户通过定义一个普通的 Python dataclass（继承自框架提供的 Mixin 基类），声明二进制结构的字段，然后即可直接使用 `parse` / `build` 方法。

```python
@dataclass
class MyMessage(StructMixin):
    address: int = field(Int8ub)       # 1字节无符号大端整数
    function_code: int = field(Int8ub)  # 1字节无符号大端整数
    data: bytes = field(GreedyBytes)    # 剩余全部字节
```

用户**不应**需要手动调用任何"编译"或"注册"函数。当类定义被执行的那一刻，编译应当自动发生。用户之后可以直接使用：

```python
msg = MyMessage(address=1, function_code=3, data=b'\x00\x01')
built = msg.build()                     # 构建为字节
parsed = MyMessage.parse(built)         # 从字节解析
```

本文档回答：**如何实现这种"定义即编译、使用即调用"的无感体验？**

### 1.2 适用对象

- 架构设计阶段：定义 StructMixin 基类的行为、field() 函数的语义、编译管线的流程
- 实现阶段：约束 `__init_subclass__` 的执行逻辑、编译产物的存储方式
- 审查阶段：作为"编译是否在正确的时机发生"的检查清单

### 1.3 术语约定

| 术语 | 含义 |
|------|------|
| **Mixin 基类** | 框架提供的、用户类需要继承的基类（如 `StructMixin`），提供 `parse` / `build` 方法 |
| **类创建钩子** | Python 中在类对象创建完成时自动调用的机制 |
| **字段描述符** | 用户通过 `field()` 函数为每个字段指定的二进制格式声明（如"1字节大端无符号整数"） |
| **编译产物** | 类创建钩子中生成的、不可变的执行树，供后续 parse/build 使用 |
| **前向引用** | 字段类型引用了在同一类定义之后才定义的类 |

---

## 2. 核心问题：用户无感编译

### 2.1 什么是"无感编译"

"无感"意味着：

1. **用户不调用编译函数**：用户只写类定义，不调用 `compile()`、`register()` 或任何类似函数。
2. **编译在类定义时自动发生**：Python 解释器执行 `class MyMessage(StructMixin): ...` 这条语句时，编译就被触发。
3. **编译结果自动可用**：编译产物被存储在某个位置，后续 `parse` / `build` 调用能自动找到它。
4. **编译错误在类定义时报出**：如果字段声明有误，用户在导入模块时就会看到错误，而非在首次调用 parse 时。

### 2.2 为什么需要自动编译

如果不自动编译，用户必须在类定义后手动调用编译：

```python
@dataclass
class MyMessage(StructMixin):
    address: int = field(Int8ub)
    ...

compile_struct(MyMessage)  # 用户必须记得调用这一行
```

这带来三个问题：

1. **遗忘风险**：用户可能忘记调用编译，导致首次 parse 时才报错（或使用未编译的慢速路径）。
2. **时机困惑**：如果字段类型涉及前向引用（引用尚未定义的类），编译必须在所有相关类都定义后才能调用。用户需要手动管理编译顺序。
3. **API 丑陋**：每定义一个类就多写一行编译调用，破坏了声明式 API 的简洁性。

自动编译通过 Python 的类创建机制，在正确的时机（类对象创建完成、类体中的所有属性都已收集）自动触发编译，解决了上述三个问题。

### 2.3 与性能的关系

自动编译不仅是为了用户体验，也是为了性能：

- 编译是 O(字段数) 的开销（需要遍历字段、构建执行树）。
- 如果编译发生在每次 parse 调用时，对于百万次 parse，编译开销会被重复支付百万次。
- 自动编译确保编译只在类定义时发生一次，此后百万次 parse 调用共享同一棵执行树。

---

## 3. 设计原则一：类创建钩子触发编译

### 3.1 Python 的类创建机制

在 Python 中，当解释器执行到 `class MyClass(BaseClass): ...` 语句时，会发生以下步骤：

1. **收集类体**：执行类体内的所有语句，将定义的变量、方法等收集到一个命名空间字典中。
2. **调用元类的 `__prepare__`**：获取用于存储类体命名空间的字典（通常是普通 dict）。
3. **创建类对象**：调用 `type.__new__(metacls, name, bases, namespace)`，创建类对象。
4. **调用 `__init_subclass__`**：在类对象创建完成后，Python 会自动调用**父类**的 `__init_subclass__` 钩子方法，传入新创建的子类。

关键点是第 4 步：`__init_subclass__` 是一个**类方法**，定义在父类中，在**子类被创建时**自动调用。这正是"自动编译"的触发点。

### 3.2 `__init_subclass__` 的工作原理

```python
class StructMixin:
    def __init_subclass__(cls, **kwargs):
        # 此处的 cls 是刚刚创建的子类（如 MyMessage）
        # 子类体中的所有属性已经收集到 cls 的命名空间中
        # 此时可以访问 cls 的类型注解、field() 声明等
        super().__init_subclass__(**kwargs)  # 遵循协作式继承
        _do_compilation(cls)                  # 执行编译
```

`__init_subclass__` 的触发时机具有以下关键特性：

- **在类对象完全创建之后**：此时子类的所有属性（包括字段声明）已经可以被访问。
- **在 `@dataclass` 装饰器执行之前还是之后**：这取决于 `@dataclass` 和继承的顺序（详见第 7 节）。
- **对每个子类只触发一次**：类定义只执行一次，因此 `__init_subclass__` 也只调用一次。

### 3.3 为什么用 `__init_subclass__` 而非元类

Python 有两种机制可以在类创建时插入逻辑：

- **元类（metaclass）**：通过自定义元类的 `__new__` / `__init__` 来拦截类创建过程。
- **`__init_subclass__`**：通过父类的钩子方法来响应子类创建。

construct-rs 选择 `__init_subclass__`，原因如下：

1. **更简单**：`__init_subclass__` 不涉及元类冲突问题。如果用户有自己的元类（或使用了带有元类的其他基类），自定义元类可能导致 "metaclass conflict" 错误。`__init_subclass__` 不引入新的元类，与任何元类兼容。
2. **时机更合适**：`__init_subclass__` 在类对象完全创建后调用，此时所有属性已就绪。元类的 `__new__` 在类对象创建过程中调用，某些属性可能尚未就绪。
3. **协作式继承**：`__init_subclass__` 通过 `super().__init_subclass__(**kwargs)` 自然支持协作式多重继承，不会破坏其他基类的初始化逻辑。

### 3.4 违反此原则的后果

**反模式 A：要求用户手动编译**

```python
@dataclass
class MyMessage(StructMixin):
    ...

StructMixin.compile(MyMessage)  # 用户必须手动调用
```

- 后果：用户可能遗忘；前向引用场景下调用时机难以确定；API 不简洁。

**反模式 B：延迟到首次 parse 时编译**

```python
class StructMixin:
    @classmethod
    def parse(cls, data):
        if not hasattr(cls, '_compiled'):
            _do_compilation(cls)  # 首次调用时编译
        ...
```

- 后果：首次 parse 调用明显比后续调用慢（包含编译开销）；编译错误延迟到首次使用时报出，而非导入时报出；多线程环境下首次调用的竞态条件需要加锁。
- 适用场景：仅当前向引用导致 `__init_subclass__` 时无法完成编译时，作为后备策略（详见第 6 节）。

**反模式 C：使用元类**

```python
class StructMeta(type):
    def __new__(mcs, name, bases, namespace):
        cls = super().__new__(mcs, name, bases, namespace)
        _do_compilation(cls)
        return cls

class StructMixin(metaclass=StructMeta):
    ...
```

- 后果：如果用户的另一个基类也有元类，会产生元类冲突（"metaclass conflict"），导致用户无法同时继承两者。增加了不必要的复杂性和兼容性风险。

> **设计原则（可证伪）**：如果编译不通过 `__init_subclass__` 自动触发，则用户必须手动管理编译时机。在涉及 10+ 个互相引用的结构体类定义时，手动管理编译顺序的复杂度将使 API 不可用。`__init_subclass__` 是 Python 语言提供的、专为"父类响应子类创建"设计的钩子，是最正确的触发点。

---

## 4. 设计原则二：编译期字段信息收集

### 4.1 编译需要收集哪些信息

当 `__init_subclass__` 被触发时，编译器需要从子类中提取以下信息：

| 信息 | 来源 | 用途 |
|------|------|------|
| **字段名** | 类体的属性名 | 执行树节点的标识；parse 结果对象的属性名；build 时读取的属性名 |
| **字段顺序** | 类体中属性定义的顺序 | 二进制数据的字段排列顺序（parse/build 按此顺序读写） |
| **字段类型描述符** | `field()` 函数的参数 | 决定每个字段的二进制格式（字节数、字节序、编码方式等） |
| **字段的 Python 类型注解** | `__annotations__` | 可选——用于类型提示和 IDE 支持，不直接影响二进制格式 |
| **嵌套关系** | 字段类型描述符中引用其他 StructMixin 子类 | 确定执行树的递归结构 |

### 4.2 字段类型描述符的形态

字段类型描述符是用户通过 `field()` 函数为每个字段指定的二进制格式声明。例如：

```python
address: int = field(Int8ub)       # Int8ub 是一个"1字节无符号大端整数"描述符
data: bytes = field(GreedyBytes)    # GreedyBytes 是一个"读取剩余全部字节"描述符
```

描述符应该是**Python 对象**，而非纯字符串标识。原因如下：

1. **携带参数**：某些描述符需要携带参数（如"读取 N 字节"需要 N 的值，"按编码 E 解码字符串"需要 E 的值）。Python 对象可以自然地携带这些参数，纯字符串标识则需要额外的解析逻辑。
2. **类型安全**：Python 对象可以有类型检查（如检查 `Int8ub` 确实是一个合法的描述符实例），纯字符串标识容易拼写错误且难以在编译期检查。
3. **可组合**：描述符可以被包装/装饰（如给一个描述符添加"默认值"或"条件判断"），Python 对象的组合模式比字符串拼接更安全。
4. **IDE 友好**：IDE 可以对描述符对象提供自动补全和类型提示，纯字符串标识无法做到。

描述符对象可以是：

- **预定义的单例**：如 `Int8ub`、`GreedyBytes`——这些是无参数的原子描述符，可以用模块级常量表示。
- **可实例化的类**：如 `Bytes(10)`（读取 10 字节）、`String(20, encoding="utf-8")`——这些需要参数，用类的实例表示。
- **引用其他 StructMixin 子类**：如字段类型直接写 `field(OtherMessage)`，表示该字段是一个嵌套的结构体。

### 4.3 字段顺序的保证

Python 的普通类体执行顺序就是属性定义的顺序（Python 3.7+ 保证字典有序）。但 `@dataclass` 装饰器可能会重新排列字段（如将带默认值的字段排在无默认值的字段之后——但 Python 3.10+ 已取消此限制）。

construct-rs 的字段顺序应遵循以下原则：

1. **以用户在类体中声明的顺序为准**：这符合二进制协议的"字段在字节流中按声明顺序排列"的直觉。
2. **不被 `@dataclass` 的字段重排影响**：编译应在 `@dataclass` 装饰器之前或与之协调，确保使用原始声明顺序。
3. **通过 `field()` 的调用顺序确定**：`field()` 函数在类体中被调用的顺序就是字段顺序。

实现方式：`field()` 函数可以在被调用时记录一个全局递增计数器，或利用 Python 3.7+ 的字典有序性在 `__init_subclass__` 中按命名空间顺序提取字段。

### 4.4 嵌套结构的识别

当字段的类型描述符引用了另一个 StructMixin 子类时，编译器需要识别这一嵌套关系，并在执行树中创建一个递归引用节点。

嵌套关系有两种形式：

- **直接引用**：`inner: InnerStruct = field(InnerStruct)`——字段的描述符就是另一个 StructMixin 子类。
- **组合引用**：`items: list = field(Array(10, InnerStruct))`——字段的描述符是一个容器（数组），容器内部包含对另一个 StructMixin 子类的引用。

编译器需要递归遍历描述符的内部结构，识别所有嵌套引用，并在执行树中建立对应的递归节点。被引用的 StructMixin 子类可能尚未编译（前向引用场景，详见第 6 节）。

### 4.5 违反此原则的后果

**反模式 A：字段顺序不确定**

如果字段顺序依赖字典遍历顺序（在 Python 3.6 及更早版本中字典无序），则同一类定义在不同 Python 版本上可能产生不同的二进制格式。

- 后果：跨版本兼容性破坏；同一份类定义在不同环境下产生不同的字节流。
- 解决：显式使用有序数据结构（如 Python 3.7+ 保证的有序 dict，或显式记录顺序的元组）。

**反模式 B：描述符用字符串标识**

```python
address: int = field("int8ub")  # 用字符串而非对象
```

- 后果：拼写错误无法在编译期检测；带参数的描述符需要额外的字符串解析语法；IDE 无法提供补全。
- 对比：`field(Int8ub)` 中 `Int8ub` 是一个真实的 Python 对象，拼写错误会在导入时报 `NameError`。

> **设计原则（可证伪）**：如果字段信息收集不完整（遗漏了字段名、顺序、类型描述符中的任何一项），则编译产物将是错误的——parse/build 产生的字节流将与协议规范不符。如果字段顺序不确定，则跨运行/跨版本的二进制兼容性将被破坏。字段信息的完整性和确定性是编译正确性的前提。

---

## 5. 设计原则三：编译产物的存储与关联

### 5.1 编译产物是什么

编译产物是一棵**不可变的执行树**（详见 FFI 设计文档第 3 节）。从 Python 的视角看，它是一个**Rust 对象**，通过 pyo3 包装为 Python 可访问的类型。

编译产物的核心特征：

1. **不可变**：创建后不修改。
2. **与类一一对应**：每个 StructMixin 子类有且仅有一棵执行树。
3. **可被 Python 侧的 `parse` / `build` 方法引用**：`parse` / `build` 需要找到这棵执行树并调用其 Rust 侧的入口函数。

### 5.2 编译产物如何与类关联

编译产物需要与它对应的类对象关联起来，使得后续的 `parse` / `build` 调用能找到它。关联方式有以下几种选择：

**方案 A：类属性（推荐）**

将编译产物存储为类的一个特殊属性（如 `cls._construct_compiled = <Rust执行树对象>`）。

- 优点：简单直接；通过 `cls._construct_compiled` 即可访问；Python 的属性查找机制天然支持继承（子类如果没有自己的编译产物，会通过 MRO 找到父类的——但 construct-rs 中每个子类都应有自己的编译产物）。
- 注意：属性名应使用名称改写（name mangling）或下划线前缀，避免与用户定义的属性冲突。

**方案 B：全局注册表**

维护一个全局字典 `_registry: Dict[Type, ExecTree]`，以类对象为键存储编译产物。

- 优点：不污染类的命名空间。
- 缺点：全局可变状态；需要处理类被垃圾回收后的注册表清理（弱引用）；多模块环境下的隔离问题。

**方案 C：闭包**

在 `__init_subclass__` 中创建闭包，捕获编译产物，并将 `parse` / `build` 方法动态绑定到类上。

- 优点：编译产物完全封装在闭包中，外部无法误访问。
- 缺点：动态创建的方法不便于调试（`inspect` 模块难以检查）；每次类定义都创建新的函数对象。

construct-rs 推荐方案 A（类属性），因为它最简单、最 Pythonic、最易于调试。编译产物作为类属性存储，`parse` / `build` 方法通过 `cls._construct_compiled` 访问它。

### 5.3 后续调用如何找到编译产物

`parse` 和 `build` 是 StructMixin 基类提供的方法，所有子类继承它们。它们的实现逻辑（概念性）：

```python
class StructMixin:
    @classmethod
    def parse(cls, data: bytes):
        compiled = cls._construct_compiled  # 从类属性获取编译产物
        return _rust_parse(compiled, data)   # 调用 Rust 侧的 parse 入口

    def build(self) -> bytes:
        compiled = type(self)._construct_compiled  # 从类属性获取编译产物
        return _rust_build(compiled, self)          # 调用 Rust 侧的 build 入口
```

关键点：

1. **`parse` 是类方法**：因为 parse 是从字节构造新对象，不需要实例。`cls._construct_compiled` 获取编译产物。
2. **`build` 是实例方法**：因为 build 是从对象构造字节，需要读取实例的属性。`type(self)._construct_compiled` 获取编译产物（用 `type(self)` 而非 `self.__class__` 以确保获取实际类型的编译产物）。
3. **编译产物查找是 O(1)**：类属性查找是 Python 字典查找，O(1) 操作。
4. **FFI 只在 `_rust_parse` / `_rust_build` 内部发生**：这两次调用各自是一次 FFI 穿越，符合"单次 FFI"原则。

### 5.4 编译产物的不变性保证

编译产物在存储后不应被修改或替换。这需要：

1. **不提供公开的修改接口**：编译产物（Rust 执行树对象）在 Python 侧只暴露 `parse` / `build` 方法，不暴露修改方法。
2. **类属性在编译后设为只读**（可选）：可以通过 `__setattr__` 或描述符协议阻止用户意外覆盖 `cls._construct_compiled`。
3. **重新编译只发生在重新定义类时**：如果用户修改了类定义（重新执行 `class MyMessage(StructMixin): ...`），新的 `__init_subclass__` 会触发新的编译，覆盖旧的编译产物。这是合理的——新的类定义意味着新的执行树。

### 5.5 违反此原则的后果

**反模式 A：每次 parse 都查找或重建编译产物**

```python
@classmethod
def parse(cls, data):
    compiled = _find_or_compile(cls)  # 可能触发编译
    return _rust_parse(compiled, data)
```

- 后果：如果 `_find_or_compile` 包含编译逻辑，首次调用会很慢；如果包含全局查找逻辑，引入全局可变状态。
- 对比：正确做法是在 `__init_subclass__` 中一次性编译并存储，`parse` 中只做一次 O(1) 的类属性查找。

**反模式 B：编译产物可被运行时修改**

```python
MyMessage._construct_compiled = some_other_tree  # 允许运行时替换
```

- 后果：同一类的 parse/build 行为变得不可预测；多线程环境下存在竞态条件；调试困难（"为什么同一个类突然产生了不同的字节流？"）。

> **设计原则（可证伪）**：如果编译产物的查找不是 O(1) 操作，或编译产物可被运行时修改，则 `parse` / `build` 的性能和行为一致性无法保证。编译产物必须是"一次编译、不可变、O(1) 查找"的三位一体。

---

## 6. 设计原则四：前向引用的延迟编译

### 6.1 什么是前向引用

前向引用是指：类的某个字段的类型描述符引用了**尚未定义**的类。这在互相引用的结构体中很常见：

```python
@dataclass
class LinkedList(StructMixin):
    value: int = field(Int8ub)
    next: Optional[LinkedList] = field(Maybe(LinkedList))  # 引用自身——此时 LinkedList 刚被定义
```

或者在两个互相引用的类中：

```python
@dataclass
class NodeA(StructMixin):
    data: int = field(Int8ub)
    peer: NodeB = field(NodeB)    # NodeB 尚未定义！

@dataclass
class NodeB(StructMixin):
    data: int = field(Int8ub)
    peer: NodeA = field(NodeA)    # NodeA 已定义
```

当 `NodeA` 的 `__init_subclass__` 被触发时，`NodeB` 尚不存在，编译器无法解析 `peer` 字段的类型描述符。

### 6.2 延迟编译策略

处理前向引用的核心策略是**延迟编译（lazy compilation）**：

1. **在 `__init_subclass__` 中尝试编译**：正常情况下，`__init_subclass__` 会立即编译执行树。
2. **如果遇到未解析的类型引用，安装一个延迟桩（stub）**：不立即编译，而是在类上安装一个"占位"的 `parse` / `build` 方法。这个桩方法在**首次被调用时**，才真正执行编译。
3. **首次调用时完成编译**：当用户首次调用 `NodeA.parse(data)` 时，桩方法被触发。此时 `NodeB` 已经定义（因为模块已完全加载），编译器可以成功解析所有类型引用。编译完成后，桩方法被替换为真正的 `parse` / `build` 方法（指向编译好的执行树）。
4. **后续调用直接使用编译产物**：首次调用完成编译后，后续调用不再经过桩方法，直接使用已编译的执行树。

### 6.3 延迟桩的工作流程

延迟桩的概念性实现：

```python
def _lazy_parse(cls, data):
    """桩方法：首次调用时触发编译，然后用真正的 parse 替换自身。"""
    _do_compilation(cls)                         # 现在尝试编译（此时前向引用可能已解析）
    cls.parse = cls._construct_compiled.parse    # 用真正的 parse 方法替换桩
    return cls.parse(data)                        # 调用真正的 parse

class StructMixin:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        try:
            _do_compilation(cls)                  # 尝试立即编译
        except UnresolvedTypeReference:
            # 前向引用未解析——安装延迟桩
            cls.parse = classmethod(lambda cls, data: _lazy_parse(cls, data))
            # build 方法同理
```

### 6.4 延迟编译的代价

延迟编译引入以下代价：

1. **首次调用延迟**：首次 `parse` / `build` 调用包含编译开销（O(字段数)）。但这只发生一次，后续调用不受影响。
2. **线程安全**：多线程环境下，首次调用可能有竞态条件（多个线程同时触发编译）。需要加锁或使用 `OnceLock` 机制确保编译只发生一次。
3. **错误延迟**：如果前向引用最终无法解析（如引用的类从未定义），错误将在首次 `parse` / `build` 调用时才报出，而非导入时。

### 6.5 延迟编译的适用边界

延迟编译仅用于**真正需要**的场景——前向引用。对于不涉及前向引用的类，`__init_subclass__` 应立即完成编译，不使用延迟桩。

判断标准：`__init_subclass__` 尝试编译时，如果抛出"未解析类型引用"错误，则回退到延迟编译；如果是其他错误（如字段描述符无效），应立即报出，不延迟。

> **设计原则（可证伪）**：如果不支持延迟编译，则涉及前向引用的类定义（互相引用的结构体、自引用的链表等）将无法工作——用户在 `__init_subclass__` 时会收到"类型未定义"错误。如果延迟编译不支持线程安全，则多线程环境下的首次调用可能触发多次编译或产生不一致的执行树。延迟编译必须：仅在必要时启用、首次调用时原子完成、后续调用零开销。

---

## 7. 设计原则五：与标准 `@dataclass` 的协调

### 7.1 协调的必要性

construct-rs 的用户 API 是 `@dataclass class X(StructMixin)`。这里有两个机制同时作用于类：

1. **`@dataclass` 装饰器**：Python 标准库提供。它根据类的类型注解（`__annotations__`）自动生成 `__init__`、`__repr__`、`__eq__` 等方法。它还收集字段信息到 `__dataclass_fields__` 属性中。
2. **`StructMixin.__init_subclass__`**：框架提供。它在子类创建时触发编译，收集字段信息并构建执行树。

两者都会读取类体信息，但关注点不同：
- `@dataclass` 关注的是"如何构造 Python 对象"（`__init__` 参数、默认值、类型注解）。
- `StructMixin` 关注的是"如何解析/构建二进制数据"（字段描述符、字节序、执行树）。

它们必须协调工作，不能互相干扰。

### 7.2 执行顺序

当用户写 `@dataclass class X(StructMixin): ...` 时，Python 的执行顺序是：

1. **执行类体**：收集 `address = field(Int8ub)` 等属性到命名空间。
2. **调用 `type.__new__` 创建类对象**：此时 `X` 类对象已创建，但尚未被 `@dataclass` 装饰。
3. **调用 `StructMixin.__init_subclass__(X)`**：框架的编译钩子被触发。此时类对象 `X` 存在，但 `@dataclass` 尚未处理它（没有 `__dataclass_fields__`，没有自动生成的 `__init__`）。
4. **`@dataclass` 装饰器执行**：装饰器接收步骤 2 创建的类对象，生成 `__init__` 等方法，添加 `__dataclass_fields__`。

关键点：**`__init_subclass__` 在 `@dataclass` 装饰器之前执行**。

这意味着：

- 在 `__init_subclass__` 中，类对象**还没有** `__dataclass_fields__` 属性。
- `__init_subclass__` 不应依赖 `@dataclass` 生成的任何属性。
- `__init_subclass__` 应从**原始的类体属性**（即用户通过 `field()` 设置的值）中收集字段信息。

### 7.3 字段信息的来源协调

`@dataclass` 和 `StructMixin` 需要从同一个类体中读取字段信息，但关注点不同：

| 信息 | `@dataclass` 的用途 | `StructMixin` 的用途 |
|------|--------------------|--------------------|
| 字段名 | `__init__` 参数名 | 执行树节点的标识 |
| 字段类型注解 | `__init__` 参数类型提示（不用于运行时） | 可选——不影响二进制格式 |
| `field()` 的返回值 | 作为字段的默认值（`default`） | 提取其中的类型描述符，决定二进制格式 |

`field()` 函数是两者的桥梁。它的返回值需要同时满足：

- 作为 `@dataclass` 的默认值（`@dataclass` 会将其放入 `__dataclass_fields__` 的 `default` 属性中）。
- 作为 `StructMixin` 的类型描述符来源（`StructMixin` 从中提取二进制格式信息）。

实现方式：`field()` 返回一个特殊的对象，该对象：
- 对于 `@dataclass`：表现为一个带有 `default` 属性的 dataclass 字段描述符。
- 对于 `StructMixin`：内部携带二进制格式描述符（如 `Int8ub`），`__init_subclass__` 从中提取。

### 7.4 编译产物与 `@dataclass` 的关系

编译产物（执行树）与 `@dataclass` 生成的 `__init__` 等方法互不干扰：

- `@dataclass` 生成的 `__init__` 用于**普通 Python 对象构造**：`MyMessage(address=1, function_code=3, data=b'...')`。
- `StructMixin` 提供的 `parse` 用于**从字节构造对象**：`MyMessage.parse(b'...')`。
- `StructMixin` 提供的 `build` 用于**从对象构造字节**：`msg.build()`。

`parse` 的返回值应是一个由 `@dataclass` 生成的 `__init__` 构造的实例（或等效的对象），这样用户可以像普通 dataclass 实例一样使用 parse 的结果。

### 7.5 违反此原则的后果

**反模式 A：`__init_subclass__` 依赖 `__dataclass_fields__`**

如果 `__init_subclass__` 从 `cls.__dataclass_fields__` 读取字段信息，将失败——因为 `@dataclass` 尚未执行。

- 后果：编译时找不到字段信息，编译失败或编译出空执行树。
- 解决：从类体的原始属性（`field()` 调用的结果）中读取字段信息，不依赖 `@dataclass` 的产物。

**反模式 B：`field()` 不兼容 `@dataclass`**

如果 `field()` 返回的对象不符合 `@dataclass` 对默认值的期望，`@dataclass` 可能报错或行为异常。

- 后果：用户类无法被 `@dataclass` 正确装饰，`__init__` 生成失败。
- 解决：`field()` 的返回值必须符合 `@dataclass` 的字段描述符协议。

**反模式 C：parse 返回的对象不是 dataclass 实例**

如果 parse 返回的对象不是通过 `@dataclass` 生成的 `__init__` 构造的（如返回一个普通 dict），用户无法用 `msg.address` 访问字段。

- 后果：parse 结果的使用方式与用户预期的 dataclass 实例不同，API 不一致。
- 解决：parse 在 Rust 侧应直接创建用户类的实例（通过调用 `__init__` 或等效的对象创建方式）。

> **设计原则（可证伪）**：如果 `__init_subclass__` 与 `@dataclass` 的执行顺序未被正确处理，则字段信息收集将失败（依赖了不存在的 `__dataclass_fields__`）。如果 `field()` 的返回值不兼容 `@dataclass` 的协议，则用户的类定义将报错。两者的协调是"无感编译"可用性的前提。

---

## 8. 对用户 API 的约束

自动编译模式为用户带来便利的同时，也对用户 API 施加了若干约束。这些约束应在文档中明确告知用户。

### 8.1 用户能做什么

| 能力 | 说明 |
|------|------|
| **声明任意数量的字段** | 每个字段通过 `name: type = field(descriptor)` 声明，数量无限制 |
| **使用嵌套结构** | 字段描述符可以引用其他 StructMixin 子类，实现结构嵌套 |
| **自定义普通方法** | 在类体中定义普通方法（`def my_method(self): ...`），不影响编译 |
| **使用 `@dataclass` 的全部功能** | 默认值、`__post_init__`、`frozen=True` 等 `@dataclass` 特性正常工作 |
| **继承** | 子类继承父类的字段（需在编译管线中正确处理继承链） |
| **类型注解** | 类型注解（`address: int`）用于 IDE 提示，不影响二进制格式 |

### 8.2 用户不能做什么

| 限制 | 原因 | 替代方案 |
|------|------|---------|
| **运行时增删字段** | 执行树在类定义时编译并固化 | 定义新类（触发新的编译） |
| **运行时修改字段类型描述符** | 同上——执行树不可变 | 定义新类 |
| **使用不支持的字段类型描述符** | 描述符必须是框架已知的类型 | 使用框架提供的描述符集合 |
| **绕过 `field()` 声明字段** | 未通过 `field()` 声明的属性不被编译器识别 | 所有二进制字段必须通过 `field()` 声明 |
| **定义会引发歧义的字段顺序** | 字段顺序即字节流中的顺序，必须无歧义 | 保持字段声明顺序与协议规范一致 |

### 8.3 错误处理的用户可见性

编译错误（如无效的字段描述符、不支持的字段类型）应在类定义时（`__init_subclass__` 中）立即抛出，并附带清晰的信息：

```
编译错误：类 MyMessage 的字段 "address" 的类型描述符 "FooBar" 不是有效的构造器。
请在 import construct 中选择已提供的构造器类型。
```

前向引用导致的延迟编译错误（如引用的类型最终未定义）在首次 `parse` / `build` 调用时抛出，应附带提示：

```
编译错误：类 NodeA 的字段 "peer" 引用了类型 "NodeB"，但该类型未在当前作用域中定义。
请确保 NodeB 在使用 NodeA.parse() 之前已被导入和定义。
```

---

## 9. 设计原则总览与适用性判断

### 9.1 五条原则速查表

| 编号 | 原则 | 一句话总结 | 违反后果 |
|------|------|-----------|---------|
| 一 | 类创建钩子触发编译 | `__init_subclass__` 在子类创建时自动编译 | 用户需手动编译或错误延迟报出 |
| 二 | 编译期字段信息收集 | 从类体收集字段名、顺序、类型描述符、嵌套关系 | 编译产物错误或字段信息不完整 |
| 三 | 编译产物的存储与关联 | 编译产物作为类属性存储，O(1) 查找，不可变 | 每次 parse 重建编译产物或运行时行为不一致 |
| 四 | 前向引用的延迟编译 | 遇到未解析引用时安装桩，首次调用时完成编译 | 互相引用的结构体无法定义 |
| 五 | 与标准 @dataclass 协调 | `__init_subclass__` 先于 `@dataclass` 执行，不依赖其产物 | 字段信息收集失败或类定义报错 |

### 9.2 适用性判断

这一设计模式适用于以下场景：

- ✅ **声明式 API**：用户通过类定义声明结构，不通过编程式 API 构建结构。
- ✅ **类定义时结构确定**：结构在类定义时完全确定，运行时不变。
- ✅ **用户同时需要 dataclass 功能**：用户希望 parse/build 的结果可以直接作为 dataclass 实例使用。
- ✅ **Python 3.7+**：`__init_subclass__` 是 Python 3.7+ 的特性。

不适用于以下场景：

- ❌ **运行时动态构建结构**：如果结构在运行时由数据驱动（如从 JSON Schema 动态生成解析器），应使用编程式 API 而非 Mixin。
- ❌ **Python 3.6 及更早版本**：`__init_subclass__` 不可用。

### 9.3 与 construct-rs 的契合度

construct-rs 的用户 API 目标（`@dataclass class X(StructMixin)` + `field(descriptor)`）完全满足上述适用条件：

1. 声明式 API——用户通过类定义声明二进制格式 ✅
2. 类定义时结构确定——二进制协议格式不变 ✅
3. 用户需要 dataclass 功能——parse 结果作为 dataclass 实例使用 ✅
4. Python 3.7+——项目最低版本要求 ✅

因此，五条设计原则完全适用于 construct-rs 的 Mixin 设计，是 StructMixin 基类和 `field()` 函数设计的基石。

### 9.4 与 FFI 设计文档的关系

本文档定义的 Mixin 编译模式与 FFI 设计文档定义的 FFI 边界模式是互补的：

- **FFI 设计文档**定义了编译产物（执行树）的**内部结构**和**运行时行为**——如何遍历、如何分派、如何操作 Python 对象。
- **本文档**定义了编译产物的**创建时机**和**关联方式**——何时编译、收集什么信息、存储在哪里、如何被找到。

两者共同构成了"用户定义类 → 自动编译 → 单次 FFI parse/build"的完整管线。

---

*本文档定义的设计原则是 construct-rs 所有 Mixin 相关决策（StructMixin 基类设计、field() 函数语义、编译管线流程）的最高约束。任何与这些原则冲突的设计方案，必须在架构评审中提出并获得明确批准后方可实施。*
