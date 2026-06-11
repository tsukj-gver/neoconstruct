# 模块设计：Core Trait

## 模块位置

`src/core/mod.rs`

## 职责

定义 `Construct` trait，这是整个库的核心抽象。所有构造器都必须实现此 trait。

## 详细设计

### Construct trait

```rust
pub trait Construct {
    /// 从流中解析数据，返回解析后的 Value
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value>;

    /// 将 Value 构建为二进制数据写入流
    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()>;

    /// 计算此构造器在当前上下文下的字节大小
    fn sizeof(&self, ctx: &Context) -> Result<usize>;
}
```

> 注：`Result<T>` 是 `std::result::Result<T, ConstructError>` 的类型别名，定义在 `error.rs` 中。

### 便捷方法（默认实现）

```rust
impl dyn Construct {
    /// 从字节切片解析
    pub fn parse_bytes(&self, data: &[u8]) -> Result<Value, ConstructError> { ... }

    /// 构建为字节向量
    pub fn build_bytes(&self, data: &Value) -> Result<Vec<u8>, ConstructError> { ... }

    /// 从文件解析
    pub fn parse_file(&self, path: &Path) -> Result<Value, ConstructError> { ... }

    /// 构建到文件
    pub fn build_file(&self, data: &Value, path: &Path) -> Result<(), ConstructError> { ... }
}
```

### Renamed 包装器

```rust
pub struct Renamed {
    pub inner: Box<dyn Construct>,
    pub name: String,
}
```

对应 Python 中 `"field_name" / Construct` 的命名操作。

### Subconstruct 抽象

```rust
pub struct Subconstruct {
    pub subcon: Box<dyn Construct>,
}
```

包装一个内部构造器的基类，供 Adapter、Tunnel 等继承。

## 设计考量

1. **`&mut dyn Stream`**：流需要可变引用以支持读写操作
2. **`&mut Context`**：parse 时需要写入上下文（先前字段值），build 时需要读取
3. **`&Context` for sizeof**：sizeof 不应修改上下文，仅读取
4. **`Result` 返回值**：所有操作都可能失败，统一错误处理

## 与 Python 版本对应

| Python | Rust |
|--------|------|
| `Construct._parse(stream, context)` | `Construct::parse(stream, ctx)` |
| `Construct._build(obj, stream, context)` | `Construct::build(data, stream, ctx)` |
| `Construct._sizeof(context)` | `Construct::sizeof(ctx)` |
| `Construct.parse(data)` | `Construct::parse_bytes(data)` |
| `Construct.build(obj)` | `Construct::build_bytes(data)` |
| `Construct.compile()` | （Phase 9 可选实现） |
