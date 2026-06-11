# 模块设计：Context 容器

## 模块位置

`src/core/context.rs`

## 职责

实现 `Context` 结构体，在 parse/build 过程中传递上下文信息，支持字段间引用和嵌套作用域。

## 详细设计

### Context 结构体

```rust
pub struct Context {
    fields: IndexMap<String, Value>,
    parent: Option<Box<Context>>,
}
```

### 核心方法

```rust
impl Context {
    /// 创建空上下文
    pub fn new() -> Self;

    /// 创建子上下文（引用当前上下文为 parent）
    pub fn subcontext(&self) -> Context;

    /// 插入键值对
    pub fn insert(&mut self, key: impl Into<String>, value: Value);

    /// 获取值（仅当前层级）
    pub fn get(&self, key: &str) -> Option<&Value>;

    /// 获取值（递归查找，先当前层级，再父级）
    pub fn get_recursive(&self, key: &str) -> Option<&Value>;

    /// 获取值，不存在则返回错误
    pub fn get_or_error(&self, key: &str) -> Result<&Value, ConstructError>;

    /// 通过 `this` 风格路径获取值
    /// 路径格式：`"header.length"` → 先查找 `header`，再在其值中查找 `length`
    pub fn get_path(&self, path: &[String]) -> Result<&Value, ConstructError>;

    /// 获取父级上下文（`_` 引用）
    pub fn parent(&self) -> Option<&Context>;

    /// 获取可变引用
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value>;
}
```

### 作用域管理

```rust
impl Context {
    /// 在子作用域中执行闭包
    /// 自动创建子上下文，执行后丢弃
    pub fn with_subcontext<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Context) -> R;

    /// 合并另一个 Context 的字段（用于 Union 等场景）
    pub fn merge(&mut self, other: Context);
}
```

### 路径查找逻辑

路径查找与 Python 版本的 `this` 引用一致：

```
Context {
    header: Container {
        length: 42,
        type: 1,
    },
    data: Bytes([...]),
}

this.header.length → fields["header"].as_container()["length"] → 42
```

查找规则：
1. 在当前上下文的 `fields` 中查找第一个 key
2. 如果路径有多级，在结果 Value 中继续查找
3. 如果当前层级找不到，向上查找 parent
4. 全部找不到则返回 `ConstructError`

## 设计考量

1. **`parent: Option<Box<Context>>`**：每次创建子上下文会复制父级的引用。Rust 中需要决定是持有引用还是复制。选择复制（`Box<Context>`）以避免生命周期复杂性，代价是子上下文的创建有额外内存开销。
2. **`IndexMap`**：保持插入顺序，支持按序遍历（如 Struct 的字段顺序）
3. **路径查找性能**：多级路径查找是 O(depth * key_length)，可接受

## 与 Python 版本对应

| Python | Rust |
|--------|------|
| `context = Container()` | `Context::new()` |
| `context.field = value` | `ctx.insert("field", value)` |
| `context.field` | `ctx.get("field")` |
| `context._` | `ctx.parent()` |
| `this.field.subfield` | `ctx.get_path(&["field", "subfield"])` |
