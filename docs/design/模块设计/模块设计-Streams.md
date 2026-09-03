---
id: DESIGN-Streams
status: active
phase: "7"
depends_on: [ADR-001, ADR-014, ADR-016, ADR-017, ADR-018, DESIGN-Adapter核心, DESIGN-Array, AGENTS.md, 分析报告-Phase7启动前置.md]
last_updated: 2026-07-30
---

# 模块设计 - Streams（Seek / Pointer / Prefixed）+ Stream 基础设施扩展

> 子任务 `7.2 [Streams 详细设计]`（PM 决策 3：Stream 扩展纳入 7.2 内）。
> 信息源：Python `construct/construct/core.py` Pointer L4384 / Seek L4594 / Prefixed L4862；
> construct-rs `stream.rs`（ParseStream + BuildStream，扩展对象）/ `nodes/prefixed_array.rs`（子流 + temp build 参考）/ `nodes/tell.rs`（save/restore 参考）/ `nodes/peek.rs`（错误路径 seek 回 fallback 参考）。
> **§0 原则对照表**见 §5（L-01 对策，必查项）。已对照 L-04（跨阶段模式）/ L-09（性能测量 ns 级标注）。

## 模块位置

| 组件 | 文件路径 | 性质 |
|------|---------|------|
| `Whence` enum | `construct-rs/src/stream.rs`（新增） | Stream 基础设施 |
| `ParseStream::seek_whence` | `construct-rs/src/stream.rs`（新增方法） | Stream 基础设施 |
| `BuildStream::seek` | `construct-rs/src/stream.rs`（新增方法 + `pos` 字段） | Stream 基础设施 |
| `SeekNode` | `construct-rs/src/nodes/seek.rs`（新建） | Node |
| `PointerNode` | `construct-rs/src/nodes/pointer.rs`（新建） | Node |
| `PrefixedNode` | `construct-rs/src/nodes/prefixed.rs`（新建） | Node |
| Node enum 扩展 | `construct-rs/src/nodes/mod.rs`（追加 3 变体） | Node 注册 |
| Descriptor 编译 | `construct-rs/src/compile.rs`（追加 3 个 `build_*_node`） | 编译系统 |
| Python 侧 Descriptor | `construct/_descriptors.py`（新增 3 个 Descriptor 类） | 用户面编译入口 |

## 职责

为 Phase 7 Streams 提供 3 个流操作构造器（Seek / Pointer / Prefixed）+ 支撑它们所必需的 Stream 基础设施扩展（ParseStream whence 通用化 + BuildStream seek 新增）。全部为纯 Rust 内部流操作，零跨 FFI。

## 1. 关键设计决策（含 PM 要求的 3 项）

### 1.1 BuildStream.seek 零填充方案（PM 关注点 1）

**决策：填充 `0x00`，对齐 Python `io.BytesIO` 标准行为。**

**依据**：
- Python `io.BytesIO` 在 `seek(pos)` 超过当前末尾后 `write(data)`，中间间隙由 `\x00` 填充（CPython `Modules/_io/bytesio.c` `bytesio_write` 的 `pad` 路径）。
- Python construct `Pointer._build` 示例（core.py L4407-4408）：
  ```python
  >>> d = Pointer(8, Bytes(1))
  >>> d.build(b"Z")
  b'\x00\x00\x00\x00\x00\x00\x00\x00Z'  # 8 个 0x00 + Z
  ```
- Python construct `Seek` build 示例（core.py L4615-4617）：
  ```python
  >>> (Bytes(10) >> Seek(5) >> Byte).build([b"0123456789", None, 255])
  b'01234\xff6789'  # Seek(5) 定位后 Byte 覆盖位置 5
  ```
  注意：此例 seek 到 pos=5 < len=10，是覆盖写非零填充。零填充仅在 `pos > len` 时触发。

**实现**：`Vec::resize(new_len, 0u8)`（Rust 标准库，一次性预留容量 + 填充，比 `push` 循环高效，对齐 `BuildStream::write_padding_bits` 的 `resize_with` 模式）。

### 1.2 Pointer save/restore 方案（PM 关注点 2）

**决策：复用 Tell 模式（`fallback = stream.tell()` → seek 到 offset → 处理 subcon → seek 回 fallback），错误路径强制 seek 回（对齐 Python `finally`）。**

**依据**：
- Python `Pointer._pointer_seek`（core.py L4417-4426）：`fallback = stream_tell` → seek → 返回 fallback；`_parse`/`_build` 在 subcon 处理后 `stream_seek(stream, fallback, 0, path)`。
- construct-rs `PeekNode::parse`（`nodes/peek.rs` L82-103）已验证此模式：`fallback = stream.tell()` → inner.parse → 无论 Ok/Err 都 `stream.seek(fallback, path)`。Pointer 复用同一模式。
- **错误路径强制 seek 回**：subcon.parse/build 返回 Err 时，必须先 seek 回 fallback 再返回 Err（否则主流位置错乱，破坏后续字段）。Rust 用 `match` + 显式 seek（无 `finally`），与 PeekNode 一致。

**两个方向对称**：
- parse：`fallback = stream.tell()` → seek(offset) → subcon.parse → seek(fallback) → return obj
- build：`fallback = stream.tell()` → seek(offset) → subcon.build → seek(fallback) → return Ok

### 1.3 Prefixed 与 PrefixedArray 的关系（PM 关注点 3）

**决策：Prefixed 独立实现为新 Node（`PrefixedNode`），不重构 PrefixedArray。**

**依据**：
- **两者本质不同**：
  - `PrefixedArray(countfield, subcon)`：lengthfield 是**元素计数**，parse 循环 N 次 inner.parse（`prefixed_array.rs` L141-153）。
  - `Prefixed(lengthfield, subcon)`：lengthfield 是**字节计数**，parse 读 N 字节作为子流，subcon.parse 整个子流（core.py L4894-4899）。
- **PrefixedArray 已 ACCEPTED**（Phase 4.6，tag `phase-4-complete`）：重写需新开 Phase，违反"已验收阶段总纲不可修改"红线（AGENTS.md §2）。
- **Python 侧 PrefixedArray 是 FocusedSeq macro**（core.py L4956），construct-rs 已独立实现避开 FocusedSeq 依赖（Phase 4 决策）。Prefixed 是独立 Subconstruct，与 PrefixedArray 无 Python 层继承关系。
- **Prefixed 的子流模式与 PrefixedArray build temp 模式部分重叠**（temp BuildStream 收集后写主流），但 parse 路径完全不同（子流 vs 循环）。复用模式但不共享代码。

**关系表述**：Prefixed 是 PrefixedArray 的"字节计数 + 单元素"兄弟，不是其父类或子类。两者各自独立 Node。

## 2. 现状与缺口（Stream 基础设施）

### 2.1 ParseStream 现状

- `seek(&mut self, pos: usize, path: &Path)`（`stream.rs` L148）：**仅 whence=0**（绝对定位），`pos > data.len()` 返回 Stream Err。
- 缺口：Pointer 的负 offset（whence=2 从 EOF）和 relativeOffset=True（whence=1 相对）无法支持。

### 2.2 BuildStream 现状

- **完全无 seek 方法**：仅有 `tell()` 返回 `buf.len()`（`stream.rs` L385）。
- `write(&mut self, data: &[u8])`（L372）：仅追加，无随机写。
- BuildStream 无 `pos` 字段——写入位置永远是 `buf.len()`（末尾追加模型）。
- 缺口：Pointer/Seek build 需要 seek 到任意位置写，且 seek 超出末尾需零填充扩展。

### 2.3 扩展必要性

| 缺口 | 阻塞的构造器 | 优先级 |
|------|------------|--------|
| ParseStream whence=1/2 | Pointer（负 offset / relativeOffset） | P0 |
| BuildStream.seek（含零填充） | Pointer build / Seek build | P0 |

## 3. 详细设计

### 3.1 Stream 基础设施扩展

#### 3.1.1 `Whence` enum（新增，stream.rs）

```rust
/// 流定位的参考点，对齐 Python `io.SEEK_SET/CUR/END`（0/1/2）。
///
/// 用于 `ParseStream::seek_whence` 与 `BuildStream::seek`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whence {
    /// 从流开头（绝对定位）。对应 Python whence=0。
    Start,
    /// 从当前位置（相对定位）。对应 Python whence=1。
    Current,
    /// 从流末尾（at 通常为负）。对应 Python whence=2。
    End,
}

impl Whence {
    /// 从 Python 传入的整数（0/1/2）构造。其他值返回 Err（编译期校验）。
    pub fn from_python_int(n: i64) -> Result<Self, ConstructError> {
        match n {
            0 => Ok(Whence::Start),
            1 => Ok(Whence::Current),
            2 => Ok(Whence::End),
            other => Err(ConstructError::Compilation {
                message: format!("whence must be 0/1/2, got {}", other),
            }),
        }
    }
}
```

#### 3.1.2 `ParseStream::seek_whence`（新增方法）

```rust
impl<'a> ParseStream<'a> {
    /// 通用 seek，支持 whence=Start/Current/End。
    ///
    /// 对齐 Python `stream_seek(stream, offset, whence, path)`（core.py L204）。
    ///
    /// # 行为
    ///
    /// - `Whence::Start`：`at < 0` 或 `at > data.len()` → Stream Err；否则 `pos = at`。
    /// - `Whence::Current`：`tell() + at` 溢出 usize 或越界 → Stream Err；否则 `pos = tell() + at`。
    ///   `at` 可为负（向后回退）。
    /// - `Whence::End`：`data.len() + at`，`at` 通常为负（从末尾向前）；`at > 0` → Stream Err
    ///   （parse 不可超越 EOF，与 Python BytesIO 一致：BytesIO seek 超过 EOF 在 read 时才报错，
    ///   但 parse 场景 seek 超过 EOF 无意义，提前报错更清晰）。
    /// - 重置 `bit_pos = 0`（字节对齐，与现有 `seek` 一致）。
    ///
    /// # 保留 `seek(pos, path)` 兼容
    ///
    /// 现有 `ParseStream::seek(pos, path)`（whence=0 专用）保留不变，供 GreedyRange/Peek
    /// 等已有调用方继续使用（避免破坏 Phase 4 已验收代码）。新方法 `seek_whence` 供
    /// Pointer/Seek 使用。
    pub fn seek_whence(
        &mut self,
        at: i64,
        whence: Whence,
        path: &Path,
    ) -> Result<(), ConstructError> {
        let new_pos: usize = match whence {
            Whence::Start => {
                if at < 0 {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek with whence=0 (Start) requires non-negative offset, got {}",
                            at
                        ),
                        path: path.to_string(),
                    });
                }
                let at_us = at as usize;
                if at_us > self.data.len() {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek out of bounds, at={}, data_len={}",
                            at_us,
                            self.data.len()
                        ),
                        path: path.to_string(),
                    });
                }
                at_us
            }
            Whence::Current => {
                let cur = self.pos as i64;
                let target = cur.checked_add(at).ok_or_else(|| ConstructError::Stream {
                    message: format!(
                        "stream seek overflow: cur={} + at={}",
                        cur, at
                    ),
                    path: path.to_string(),
                })?;
                if target < 0 || target as usize > self.data.len() {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek out of bounds, target={}, data_len={}",
                            target,
                            self.data.len()
                        ),
                        path: path.to_string(),
                    });
                }
                target as usize
            }
            Whence::End => {
                if at > 0 {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek with whence=2 (End) requires non-positive offset on parse stream, got {}",
                            at
                        ),
                        path: path.to_string(),
                    });
                }
                let target = (self.data.len() as i64).checked_add(at).ok_or_else(|| {
                    ConstructError::Stream {
                        message: format!("stream seek underflow: len={} + at={}", self.data.len(), at),
                        path: path.to_string(),
                    }
                })?;
                if target < 0 {
                    return Err(ConstructError::Stream {
                        message: format!(
                            "stream seek out of bounds, target={}, data_len={}",
                            target,
                            self.data.len()
                        ),
                        path: path.to_string(),
                    });
                }
                target as usize
            }
        };
        self.pos = new_pos;
        self.bit_pos = 0;
        Ok(())
    }
}
```

**与现有 `seek` 的关系**：现有 `seek(pos, path)` 等价于 `seek_whence(pos as i64, Whence::Start, path)`，但保留独立方法以：(a) 不破坏 Phase 4 已验收代码；(b) 避免 whence=0 的常见路径多一次 match 分派。

#### 3.1.3 `BuildStream` 扩展（新增 `pos` 字段 + `seek` 方法）

```rust
#[derive(Debug, Default)]
pub struct BuildStream {
    /// 输出缓冲。
    buf: Vec<u8>,
    /// **新增**：当前写入位置（字节偏移）。0..=buf.len()。
    ///
    /// Phase 7 前无此字段（写入永远是末尾追加）。引入 seek 后，写入位置可与末尾分离：
    /// - `pos < buf.len()`：覆盖写（下次 write 从 pos 开始覆盖，pos 推进，buf.len() 不变）
    /// - `pos > buf.len()`：零填充扩展到 pos（下次 write 前 resize）
    /// - `pos == buf.len()`：末尾追加（原行为）
    pos: usize,
    /// 当前字节内 bit 偏移（0-7）。0 表示字节对齐。
    bit_pos: u8,
    /// `bit_pos > 0` 时的部分字节。
    current_byte: u8,
}

impl BuildStream {
    /// **行为变更**：write 现在从 `pos` 开始写，可能覆盖或扩展 buf。
    ///
    /// - `pos + data.len() <= buf.len()`：覆盖写（`buf[pos..pos+data.len()] = data`）
    /// - `pos + data.len() > buf.len()`：先 resize 到 `pos + data.len()`（零填充间隙），
    ///   再写入 data（覆盖刚填充的零的尾部）
    /// - 写完后 `pos += data.len()`
    ///
    /// # bit 对齐前置条件
    ///
    /// 与原 `write` 一致：`bit_pos == 0`。
    pub fn write(&mut self, data: &[u8]) {
        debug_assert!(
            self.bit_pos == 0,
            "byte-level write in bit-unaligned position (bit_pos={})",
            self.bit_pos
        );
        let end = self.pos + data.len();
        if end > self.buf.len() {
            // 间隙 [buf.len()..end) 用 0x00 填充（Python BytesIO 行为）。
            self.buf.resize(end, 0u8);
        }
        self.buf[self.pos..end].copy_from_slice(data);
        self.pos = end;
    }

    /// **行为变更**：tell 现在返回 `pos`（写入位置），不是 `buf.len()`。
    ///
    /// 对齐 Python `io.BytesIO.tell()`（返回当前指针位置）。
    /// 旧调用方（StructNode build）依赖 `tell()` 返回末尾位置——由于 StructNode 不调 seek，
    /// `pos` 始终等于 `buf.len()`，行为不变（向后兼容）。
    pub fn tell(&self) -> usize {
        self.pos
    }

    /// **新增**：seek 到指定位置（支持零填充扩展）。
    ///
    /// 对齐 Python `io.BytesIO.seek(offset, whence)`。
    ///
    /// # 行为
    ///
    /// - `Whence::Start`：`at < 0` → Stream Err；否则 `pos = at`（不扩展 buf，仅移动指针）
    /// - `Whence::Current`：`pos + at`，`at` 可为负；溢出/负值 → Stream Err
    /// - `Whence::End`：`buf.len() + at`，`at` 可为正（build 允许 seek 超过末尾，下次 write 时零填充）
    /// - **不主动扩展 buf**：seek 仅移动 `pos`，零填充延迟到下次 `write`（避免无 write 的空 seek 浪费内存）
    /// - 重置 `bit_pos = 0`、`current_byte = 0`（字节对齐，与 ParseStream 一致）
    ///
    /// # 错误
    ///
    /// at/whence 组合导致 pos 为负或溢出 → `ConstructError::Stream`。
    pub fn seek(
        &mut self,
        at: i64,
        whence: Whence,
        path: &Path,
    ) -> Result<(), ConstructError> {
        let new_pos: i64 = match whence {
            Whence::Start => at,
            Whence::Current => (self.pos as i64)
                .checked_add(at)
                .ok_or_else(|| ConstructError::Stream {
                    message: format!("BuildStream seek overflow: pos={} + at={}", self.pos, at),
                    path: path.to_string(),
                })?,
            Whence::End => (self.buf.len() as i64)
                .checked_add(at)
                .ok_or_else(|| ConstructError::Stream {
                    message: format!("BuildStream seek overflow: len={} + at={}", self.buf.len(), at),
                    path: path.to_string(),
                })?,
        };
        if new_pos < 0 {
            return Err(ConstructError::Stream {
                message: format!("BuildStream seek to negative position: {}", new_pos),
                path: path.to_string(),
            });
        }
        self.pos = new_pos as usize;
        self.bit_pos = 0;
        self.current_byte = 0;
        Ok(())
    }

    /// **行为变更**：`into_bytes` / `as_bytes` 返回 buf 本身（不变）。
    ///
    /// 注意：若 seek 后未 write 满末尾，`buf.len()` 反映实际写入边界。
    /// StructNode build 顶层不 seek，`pos == buf.len()`，行为不变。

    /// **新增**：已写入字节数（buf 实际长度，非 pos）。
    ///
    /// 用于 sizeof 计算与顶层 build 收尾。与 `tell()`（返回 pos）区分。
    pub fn written_len(&self) -> usize {
        self.buf.len()
    }
}
```

**向后兼容性分析**（关键）：
- `StructNode::build`（Phase 1）调 `stream.write(field_bytes)` + `stream.tell()`（RO 字段取位置）。引入 `pos` 后：
  - `write` 仍追加（pos == buf.len() 时 `end > buf.len()` → resize + copy，等价于 `extend_from_slice`）
  - `tell()` 返回 pos == buf.len()，与旧 `buf.len()` 一致
  - **零行为变更**，Phase 1-6 已验收代码无需修改。
- `BitwiseNode` / `BytewiseNode` / `TransformNode` build 用 `write_bits` / `write_padding_bits`——这些方法操作 `buf.push`（末尾追加），不涉及 pos。引入 pos 后需审计：**bit 级 API 假设末尾追加模型**。
  - **决策**：bit 级 API（`write_bits` / `write_padding_bits`）保持末尾追加语义，**不感知 pos**。文档化为已知限制：Bitwise 域内不可用 Pointer/Seek（与 Python RestreamedBytesIO 行为一致，Pointer/Seek 在 bit 域内行为未定义）。
  - 审计结论：`write_bits_slow` / `write_padding_bits` 用 `self.buf.push`，与 `pos` 无关，无需修改。但若用户在 Bitwise 域内 seek，pos 与 buf.len() 分离，bit API 仍写 buf 末尾——会破坏一致性。**用 `debug_assert!(self.pos == self.buf.len())` 在 bit API 入口守卫**（仅 debug build 捕获契约违反，release 静默）。

### 3.2 SeekNode（对应 Python `Seek`，core.py L4594）

```rust
/// 流定位节点：parse/build 执行 stream.seek，返回新位置。
///
/// 对应 Python construct `Seek(at, whence=0)`（core.py L4594）。
///
/// # 三方法行为
///
/// - parse：求 at（常量或 ExprProgram）→ `stream.seek_whence(at, whence, path)` →
///   返回新位置 `PyLong(stream.tell())`（对齐 Python `_parse` 返回 `stream_seek` 返回值）
/// - build：求 at → `stream.seek(at, whence, path)` → 返回 Ok(())
///   （construct-rs build 签名不返回值；Python `_build` 返回 stream_seek 返回值被 Sequence 忽略）
/// - sizeof：永远 Err（对齐 Python `SizeofError`）
///
/// # flagbuildnone
///
/// Python `Seek.__init__` 设 `flagbuildnone=True`（build 接受 None 输入）。construct-rs
/// build 签名接收 `&Bound<PyAny>`，Seek build 忽略 obj 内容（仅求 at/whence 定位），
/// 自然兼容 None。
#[derive(Debug)]
pub struct SeekNode {
    /// 定位目标：编译期常量或运行时表达式。
    at: SeekOffset,
    /// 定位参考点（编译期常量，Python whence 通常是 int 字面量）。
    whence: Whence,
}

/// Seek 的定位目标，对应 Python `at` 参数（int 或 context lambda）。
#[derive(Debug)]
pub enum SeekOffset {
    /// 编译期常量（如 `Seek(5)`）。
    Const(i64),
    /// 表达式（如 `Seek(offset)`），编译期从 FieldRef/ExprRef 翻译为 ExprProgram。
    Expr(ExprProgram),
}

impl SeekNode {
    pub fn new(at: SeekOffset, whence: Whence) -> Self {
        Self { at, whence }
    }
    pub fn at(&self) -> &SeekOffset { &self.at }
    pub fn whence(&self) -> Whence { self.whence }
    /// has_expressions：仅 Expr 变体为 true。
    pub fn has_expressions(&self) -> bool {
        matches!(self.at, SeekOffset::Expr(_))
    }
}

impl Construct for SeekNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let at = self.eval_at(ctx, py, path)?;
        stream.seek_whence(at, self.whence, path)?;
        // 返回新位置（对齐 Python _parse 返回 stream_seek 返回值）
        Ok(stream.tell().into_py(py))
    }

    fn build(
        &self,
        py: Python<'_>,
        _obj: &Bound<'_, PyAny>,  // Seek 忽略 obj（flagbuildnone 兼容）
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let at = self.eval_at(ctx, py, path)?;
        stream.seek(at, self.whence, path)
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Err(ConstructError::Generic {
            message: "Seek only moves the stream, size is not meaningful".to_string(),
            path: String::new(),
        })
    }
}

impl SeekNode {
    /// 求值 at（Const 直接返回，Expr 调 eval_expr_int）。
    fn eval_at(
        &self,
        ctx: &Context<'_>,
        py: Python<'_>,
        path: &mut Path,
    ) -> Result<i64, ConstructError> {
        match &self.at {
            SeekOffset::Const(n) => Ok(*n),
            SeekOffset::Expr(prog) => crate::expr::eval_expr_int(prog, ctx, py)
                .map_err(|mut e| {
                    e.push_path_segment("at");
                    e
                }),
        }
    }
}
```

### 3.3 PointerNode（对应 Python `Pointer`，core.py L4384）

```rust
/// 绝对偏移读写节点：seek 到 offset 处理 subcon，再 seek 回原位置。
///
/// 对应 Python construct `Pointer(offset, subcon, stream=None, relativeOffset=False)`（core.py L4384）。
///
/// # 三方法行为
///
/// - parse：求 offset → 记 `fallback = stream.tell()` → seek(offset, whence) →
///   subcon.parse → **无论 Ok/Err 都 seek(fallback, Start)** → 返回 subcon 结果
/// - build：对称（seek → subcon.build → seek 回 fallback）
/// - sizeof：返回 0（Pointer 不占主流位置）
///
/// # whence 计算（对齐 Python `_pointer_seek`，core.py L4421-4424）
///
/// - `relative == true` → `Whence::Current`
/// - `relative == false`：
///   - `offset >= 0` → `Whence::Start`
///   - `offset < 0` → `Whence::End`（从 EOF 向前）
///
/// # 已知限制（文档化）
///
/// - **`stream` 参数（换流）不支持**：Python 允许 `Pointer(offset, subcon, stream=context_lambda)`
///   换流。construct-rs 单一流模型不支持。换流极少用（主流用法是默认流）。
/// - **错误路径强制 seek 回 fallback**：subcon.parse/build 失败时，必须先 seek 回 fallback
///   再返回 Err（对齐 Python `finally: stream_seek(fallback, 0)`）。Rust 用 `match` + 显式
///   seek（无 finally），与 PeekNode 模式一致。
#[derive(Debug)]
pub struct PointerNode {
    /// 偏移目标：编译期常量或运行时表达式。
    offset: PointerOffset,
    /// relativeOffset 编译期 bool（Python 默认 False）。
    relative: bool,
    /// 被偏移处理的子树。
    subcon: Box<Node>,
}

/// Pointer 的偏移目标，对应 Python `offset` 参数（int 或 context lambda）。
#[derive(Debug)]
pub enum PointerOffset {
    /// 编译期常量（如 `Pointer(8, Bytes(1))`）。
    Const(i64),
    /// 表达式（如 `Pointer(off, Bytes(1))`），编译期从 FieldRef/ExprRef 翻译为 ExprProgram。
    Expr(ExprProgram),
}

impl PointerNode {
    pub fn new(offset: PointerOffset, relative: bool, subcon: Node) -> Self {
        Self {
            offset,
            relative,
            subcon: Box::new(subcon),
        }
    }
    pub fn offset(&self) -> &PointerOffset { &self.offset }
    pub fn relative(&self) -> bool { self.relative }
    pub fn subcon(&self) -> &Node { &self.subcon }
    /// has_expressions：Expr offset 或 subcon 含表达式。
    pub fn has_expressions(&self) -> bool {
        matches!(self.offset, PointerOffset::Expr(_)) || self.subcon.has_expressions()
    }

    /// 求值 offset（Const 直接返回，Expr 调 eval_expr_int）。
    fn eval_offset(
        &self,
        ctx: &Context<'_>,
        py: Python<'_>,
        path: &mut Path,
    ) -> Result<i64, ConstructError> {
        match &self.offset {
            PointerOffset::Const(n) => Ok(*n),
            PointerOffset::Expr(prog) => crate::expr::eval_expr_int(prog, ctx, py)
                .map_err(|mut e| {
                    e.push_path_segment("offset");
                    e
                }),
        }
    }

    /// 计算 whence（对齐 Python `_pointer_seek` L4421-4424）。
    fn compute_whence(&self, offset_val: i64) -> Whence {
        if self.relative {
            Whence::Current
        } else if offset_val < 0 {
            Whence::End
        } else {
            Whence::Start
        }
    }
}

impl Construct for PointerNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        let offset_val = self.eval_offset(ctx, py, path)?;
        let whence = self.compute_whence(offset_val);
        let fallback = stream.tell();
        // seek 到目标（失败直接返回，未改 fallback）
        stream.seek_whence(offset_val, whence, path)?;
        // subcon.parse —— 无论 Ok/Err 都 seek 回 fallback
        let result = self.subcon.parse(py, stream, ctx, path);
        let seek_back = stream.seek(fallback, path);  // whence=0 用现有 seek
        // seek 失败优先（破坏主流位置，比 subcon 错误更严重）
        seek_back?;
        result
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        let offset_val = self.eval_offset(ctx, py, path)?;
        let whence = self.compute_whence(offset_val);
        let fallback = stream.tell();
        stream.seek(offset_val, whence, path)?;
        let result = self.subcon.build(py, obj, stream, ctx, path);
        let seek_back = stream.seek(fallback as i64, Whence::Start, path);
        seek_back?;
        result
    }

    fn sizeof(&self, _ctx: &Context<'_>) -> Result<usize, ConstructError> {
        Ok(0)  // 对齐 Python _sizeof return 0
    }
}
```

**关键实现细节**：
- parse 用 `stream.seek(fallback, path)`（现有 whence=0 方法），因为 fallback 是绝对位置且已校验过 bounds（fallback 来自 tell，必在 [0, data.len()] 内）。
- build 用 `stream.seek(fallback as i64, Whence::Start, path)`（新方法）。
- `seek_back?` 优先于 `result`：若 seek 回失败（极端，fallback 越界），即使 subcon 成功也报错；若 subcon 失败但 seek 回成功，返回 subcon 错误（携带 subcon 的 path）。

### 3.4 PrefixedNode（对应 Python `Prefixed`，core.py L4862）

```rust
/// 长度前缀子流节点：lengthfield 给出字节数，subcon 在该子流上处理。
///
/// 对应 Python construct `Prefixed(lengthfield, subcon, includelength=False)`（core.py L4862）。
///
/// # 三方法行为
///
/// - parse：lengthfield.parse → 得 length（PyLong → i64）→ if includelength 减 lengthfield.sizeof →
///   `stream.read(length)` 取子切片 → `ParseStream::new(slice)` → subcon.parse(子流)
/// - build：创建 temp BuildStream → subcon.build(temp) → `data = temp.into_bytes()` →
///   length = data.len()（+ lengthfield.sizeof if includelength）→ lengthfield.build(length) →
///   主 stream.write(data)
/// - sizeof：lengthfield.sizeof + subcon.sizeof（includelength 不影响 sizeof 总和）
///
/// # 与 PrefixedArrayNode 的关系
///
/// 见 §1.3。两者独立 Node，不共享代码。PrefixedArray 是元素计数 + 循环；
/// Prefixed 是字节计数 + 子流。
#[derive(Debug)]
pub struct PrefixedNode {
    /// 长度字段（任意能产生整数的 Node，如 VarInt / Byte / Int16ub）。
    lengthfield: Box<Node>,
    /// 子树（在子流上 parse/build）。
    subcon: Box<Node>,
    /// length 是否包含 lengthfield 自身大小（Python 默认 False）。
    includelength: bool,
}

impl PrefixedNode {
    pub fn new(lengthfield: Node, subcon: Node, includelength: bool) -> Self {
        Self {
            lengthfield: Box::new(lengthfield),
            subcon: Box::new(subcon),
            includelength,
        }
    }
    pub fn lengthfield(&self) -> &Node { &self.lengthfield }
    pub fn subcon(&self) -> &Node { &self.subcon }
    pub fn includelength(&self) -> bool { self.includelength }
    /// has_expressions：lengthfield 或 subcon 含表达式。
    pub fn has_expressions(&self) -> bool {
        self.lengthfield.has_expressions() || self.subcon.has_expressions()
    }
}

impl Construct for PrefixedNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. 解析 lengthfield 得到 length（对齐 PrefixedArray PA-1/PA-2 错误处理）。
        let length_obj = self.lengthfield.parse(py, stream, ctx, path)
            .map_err(|mut e| { e.push_path_segment("lengthfield"); e })?;
        let mut length_i64: i64 = length_obj.bind(py).extract().map_err(|_| {
            ConstructError::Range {
                message: "Prefixed lengthfield did not produce an integer".to_string(),
                path: path.to_string(),
            }
        })?;
        if length_i64 < 0 {
            return Err(ConstructError::Range {
                message: format!("invalid Prefixed length {}", length_i64),
                path: path.to_string(),
            });
        }
        // 2. if includelength，减去 lengthfield.sizeof。
        if self.includelength {
            let lf_size = self.lengthfield.sizeof(ctx).map_err(|mut e| {
                e.push_path_segment("lengthfield");
                e
            })? as i64;
            length_i64 -= lf_size;
            if length_i64 < 0 {
                return Err(ConstructError::Range {
                    message: format!(
                        "Prefixed length {} shorter than lengthfield size {}",
                        length_i64 + lf_size, lf_size
                    ),
                    path: path.to_string(),
                });
            }
        }
        let length = length_i64 as usize;
        // 3. 读 length 字节作为子流（对齐 Python BytesIOWithOffsets.from_reading）。
        let slice = stream.read(length, path)?;
        let mut sub_stream = ParseStream::new(slice);
        // 4. subcon 在子流上 parse（path 不变，子流错误直接传播）。
        //    注意：subcon 未消费完的字节被忽略（对齐 Python "忽略剩余"语义）。
        self.subcon.parse(py, &mut sub_stream, ctx, path)
    }

    fn build(
        &self,
        py: Python<'_>,
        obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // 1. 创建 temp BuildStream，subcon.build 到 temp（对齐 PrefixedArray build 模式）。
        let mut temp = BuildStream::new();
        self.subcon.build(py, obj, &mut temp, ctx, path)?;
        let data = temp.into_bytes();
        // 2. 计算 length（+ lengthfield.sizeof if includelength）。
        let mut length = data.len() as i64;
        if self.includelength {
            length += self.lengthfield.sizeof(ctx).map_err(|mut e| {
                e.push_path_segment("lengthfield");
                e
            })? as i64;
        }
        // 3. 先 build lengthfield（写入长度，对齐 PrefixedArray PA-5：溢出由 lengthfield 自报）。
        let length_py = length.into_py(py);
        if let Err(mut e) = self.lengthfield.build(py, length_py.bind(py), stream, ctx, path) {
            e.push_path_segment("lengthfield");
            return Err(e);
        }
        // 4. 写入 data 到主流。
        stream.write(&data);
        Ok(())
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        // 对齐 Python _sizeof：lengthfield.sizeof + subcon.sizeof（includelength 不影响总和，
        // 因为 includelength 仅调整 length 值，不改变两字段总字节数）。
        let lf = self.lengthfield.sizeof(ctx).map_err(|mut e| {
            e.push_path_segment("lengthfield");
            e
        })?;
        let sub = self.subcon.sizeof(ctx)?;
        Ok(lf + sub)
    }
}
```

**关键实现细节**：
- parse 子流：`ParseStream::new(slice)` 是零拷贝（借用 `stream.read` 返回的 `&'a [u8]`）。子流的生命周期绑定到主流的借用（`'a`），子流 parse 期间主流不可用（Rust 借用规则保证）。
- build temp：`BuildStream::new()` 创建独立缓冲，subcon.build 写入 temp，`into_bytes` 消费 temp 取 Vec。
- includelength：sizeof 总和不变（lengthfield + subcon 字节数固定），仅 length 值调整。

### 3.5 Node enum 扩展 + has_expressions（`nodes/mod.rs`）

```rust
// 在 Node enum 追加 3 个变体（Phase 7.2）
pub enum Node {
    // ... 现有 40+ 变体 ...
    /// 流定位节点（对应 Python construct `Seek`，Phase 7.2）。
    /// parse/build 执行 stream.seek；sizeof 永远 Err。
    Seek(SeekNode),
    /// 绝对偏移读写节点（对应 Python construct `Pointer`，Phase 7.2）。
    /// seek 到 offset 处理 subcon 再 seek 回；sizeof 返回 0。
    Pointer(PointerNode),
    /// 长度前缀子流节点（对应 Python construct `Prefixed`，Phase 7.2）。
    /// lengthfield 给出字节数，subcon 在子流上处理。
    Prefixed(PrefixedNode),
}

// has_expressions 追加（mod.rs L307 impl Node）
impl Node {
    pub fn has_expressions(&self) -> bool {
        match self {
            // ... 现有分支 ...
            Node::Seek(s) => s.has_expressions(),
            Node::Pointer(p) => p.has_expressions(),
            Node::Prefixed(p) => p.has_expressions(),
            _ => false,
        }
    }
}
```

**SeekNode/PointerNode/PrefixedNode 不作为 RO 字段**（不加入 `compute_ro_value`，与 SubconstructNode 同模式——它们是构造器主字段，非 Tell/Computed 类 RO 字段）。

### 3.6 编译路径（`compile.rs` + Python `_descriptors.py`）

#### 3.6.1 Python 侧 Descriptor（新增 3 个，`construct/_descriptors.py`）

```python
class SeekDescriptor(BaseDescriptor):
    """Seek(at, whence=0) 的编译期描述符。
    
    _expr_params = {"at": "at"}  # at 键存 ExprOp 列表（仅当 at 是表达式）
    """
    # 属性：at（int 或 FieldRef/ExprRef）、whence（int 0/1/2）

class PointerDescriptor(BaseDescriptor):
    """Pointer(offset, subcon, stream=None, relativeOffset=False) 的编译期描述符。
    
    _expr_params = {"offset": "offset"}  # offset 键存 ExprOp 列表（仅当 offset 是表达式）
    # 属性：offset、subcon（子描述符）、relativeOffset（bool）
    # stream 参数：编译期拒绝非 None（文档化为已知限制，换流不支持）

class PrefixedDescriptor(BaseDescriptor):
    """Prefixed(lengthfield, subcon, includelength=False) 的编译期描述符。
    
    _expr_params = {}  # 无顶层表达式（lengthfield/subcon 递归编译）
    # 属性：lengthfield（子描述符）、subcon（子描述符）、includelength（bool）
```

`_extract_and_compile_exprs`（Python 侧表达式提取）需识别这 3 个 Descriptor 类，递归 PointerDescriptor.subcon / PrefixedDescriptor.lengthfield + subcon（与 BitwiseDescriptor.subcon / PrefixedArrayDescriptor.countfield+subcon 同模式，但**不递归 inner 内的表达式**——L-04 模式：包装型描述符的 inner 不支持含表达式子描述符）。

#### 3.6.2 Rust 侧编译（`compile.rs` 追加 3 个 `build_*_node`）

```rust
/// 从 `SeekDescriptor` 构建 `SeekNode`。
///
/// 编译路径（参照 StopIfDescriptor 的 "condfunc" 模式 + BytesDescriptor 的 length 模式）：
/// 1. 从 desc 读取 `at`：
///    - Python int 常量 → `SeekOffset::Const(i64)`
///    - FieldRef/ExprRef → 从 `expr_programs[field_index]["at"]` 取 ExprOp 列表 → `SeekOffset::Expr`
/// 2. 从 desc 读取 `whence`（int 0/1/2）→ `Whence::from_python_int`
fn build_seek_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
) -> Result<SeekNode, ConstructError> { /* ... */ }

/// 从 `PointerDescriptor` 构建 `PointerNode`。
///
/// 编译路径（参照 PascalStringDescriptor lengthfield 递归 + StopIf condfunc 表达式）：
/// 1. 从 desc 读取 `offset`：常量 → Const；表达式 → 从 expr_programs[field_index]["offset"] 取
/// 2. 从 desc 读取 `relativeOffset`（bool）
/// 3. 递归编译 `subcon`（沿用 field_index，与 BitwiseDescriptor 同模式，不递归 inner 表达式）
/// 4. 校验 `stream` 属性为 None（非 None 编译期拒绝，文档化已知限制）
fn build_pointer_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<PointerNode, ConstructError> { /* ... */ }

/// 从 `PrefixedDescriptor` 构建 `PrefixedNode`。
///
/// 编译路径（参照 PascalStringDescriptor，双重子描述符递归）：
/// 1. 递归编译 `lengthfield`（沿用 field_index，与 PascalString 同模式）
/// 2. 递归编译 `subcon`（同上）
/// 3. 读取 `includelength`（bool）
fn build_prefixed_node(
    py: Python<'_>,
    desc: &Bound<'_, PyAny>,
    field_index: usize,
    expr_programs: &[Option<Py<PyAny>>],
    field_names: &[String],
) -> Result<PrefixedNode, ConstructError> { /* ... */ }
```

**Descriptor 名到 build 函数的分派**（`compile.rs` 现有 match）：追加 `"SeekDescriptor" =>` / `"PointerDescriptor" =>` / `"PrefixedDescriptor" =>` 三个分支。

## 4. 与 Python 版本对应

| Python 类/方法 (core.py) | Rust 类型/方法 | 行为对齐 |
|--------------------------|---------------|---------|
| `Seek.__init__(at, whence=0)` | `SeekNode::new(at: SeekOffset, whence: Whence)` | ✅ whence 默认 Start |
| `Seek._parse` (L4626-4629) | `SeekNode::parse` | ✅ 求 at/whence → seek → 返回新位置 PyLong |
| `Seek._build` (L4631-4634) | `SeekNode::build` | ⚠️ construct-rs build 不返回值（Python 返回 stream_seek 返回值被 Sequence 忽略） |
| `Seek._sizeof` (L4636-4637) | `SeekNode::sizeof` | ✅ 永远 Err（SizeofError） |
| `Seek.flagbuildnone=True` | `SeekNode::build` 忽略 obj | ✅ 兼容 None 输入 |
| `Pointer.__init__(offset, subcon, stream=None, relativeOffset=False)` | `PointerNode::new(offset: PointerOffset, relative: bool, subcon: Node)` | ⚠️ stream 参数不支持（已知限制） |
| `Pointer._pointer_seek` (L4417-4426) | `PointerNode::compute_whence` + `seek_whence`/`seek` | ✅ relative→Current；负 offset→End；正 offset→Start |
| `Pointer._parse` (L4428-4432) | `PointerNode::parse` | ✅ fallback → seek → subcon.parse → seek 回 fallback |
| `Pointer._build` (L4434-4438) | `PointerNode::build` | ✅ 对称 parse |
| `Pointer._sizeof` (L4440-4441) | `PointerNode::sizeof` | ✅ 返回 0 |
| `Prefixed.__init__(lengthfield, subcon, includelength=False)` | `PrefixedNode::new(lengthfield, subcon, includelength)` | ✅ |
| `Prefixed._parse` (L4894-4899) | `PrefixedNode::parse` | ✅ lengthfield.parse → if includelength 减 sizeof → read 子流 → subcon.parse |
| `Prefixed._build` (L4901-4910) | `PrefixedNode::build` | ✅ temp build → into_bytes → lengthfield.build → write data |
| `Prefixed._sizeof` (L4912-4913) | `PrefixedNode::sizeof` | ✅ lengthfield.sizeof + subcon.sizeof |
| `BytesIOWithOffsets.from_reading` (L240-243) | `stream.read(length)` + `ParseStream::new(slice)` | ✅ 零拷贝子流（Rust 借用切片） |
| `io.BytesIO` seek 超末尾 + write 零填充 | `BuildStream::write` 的 `resize(end, 0u8)` | ✅ 0x00 填充 |
| `stream_seek` whence 0/1/2 | `Whence::Start/Current/End` + `seek_whence`/`seek` | ✅ |

## 5. §0 原则对照表（L-01 对策，必查项）

逐条对照 `AGENTS.md §0` 核心原则 1-8：

| §0 原则 | 本设计如何满足 | 证据 |
|---------|--------------|------|
| **#1 一次 FFI**（编译/parse/build 各仅一次 Python↔Rust 边界穿越） | ✅ parse/build 主入口由 StructNode 顶层进入一次。Seek/Pointer/Prefixed 内部全 Rust：stream.seek/read/write、ExprProgram 求值（零 FFI）、子流构造。lengthfield/subcon 是 `Box<Node>`（Rust 内部 dispatch）。**无跨 FFI 返回 dict**。 | §3.2-3.4 全部签名用 `&mut ParseStream`/`&mut BuildStream`（纯 Rust），无 `Py<PyDict>` 跨边界 |
| **#2 无中间表示层**（parse 直接 bytes→PyObject；build 直接 PyObject→bytes） | ✅ Pointer parse：subcon.parse 直接产 PyObject（Seek 仅移动游标，Pointer 转发）；Prefixed parse：子流是 `&[u8]` 切片（非中间数据类型），subcon.parse 直接产 PyObject。build：temp BuildStream 是输出缓冲本身（Vec<u8>），非 Rust 中间枚举。 | §3.4 `ParseStream::new(slice)` 零拷贝借用；§3.3 subcon 直接转发 |
| **#3 输入输出侧无 trait 抽象层** | ✅ 全部持有 `Box<Node>`（enum_dispatch 静态分派），不引入新 trait 抽象。Stream 是已存在的纯 Rust 抽象（§0 #8），本设计仅扩展方法不引入 trait。 | §3.5 Node enum 追加 3 变体 |
| **#4 pyo3 是核心依赖** | ✅ 单 crate `construct-rs`，无独立 Python 绑定层。 | 全文档 |
| **#5 mashumaro 式 API** | N/A（Streams 不涉及用户面 dataclass，是 Struct 字段内的构造器） | — |
| **#6 enum_dispatch 静态分派** | ✅ Seek/Pointer/Prefixed 加入 Node enum，编译期静态 match 分派。 | §3.5 |
| **#7 Result<T, ConstructError> + thiserror，错误携带 path** | ✅ 所有方法返回 `Result<_, ConstructError>`。错误路径通过 `push_path_segment("lengthfield"/"offset"/"at")` 重建 path（Pointer/Prefixed）。Stream Err 复用现有 `ConstructError::Stream` 变体（含 path）。 | §3.3-3.4 `map_err` + `push_path_segment` |
| **#8 Stream 抽象纯 Rust 内部，不跨 FFI** | ✅ **本设计的核心**。ParseStream/BuildStream 全 Rust（无 C API 调用）。Whence/seek_whence/seek 扩展均在 Rust 内部。 | §3.1 全部签名无 pyo3 类型 |

**REV 必查项**：本对照表是 L-01 对策的硬性检查。若实现偏离（如 parse 返回 dict 跨 FFI / 引入 I/O trait 层），立即驳回。

## 6. 性能假设（L-02/L-05/L-09 对策）

### 6.1 瓶颈识别（量化数据 + 来源）

| 构造器 | 主要开销来源 | Python baseline 参考 | 量级参考（L-09：ns 级标注） |
|--------|------------|---------------------|-------------------------|
| **Seek** | stream.seek（Rust 内部，~5-10ns）+ 可能的 ExprProgram 求值（~10ns） | Python `stream_seek` 包装 + `evaluate` lambda + io.BytesIO.seek（~300-500ns） | Seek 是"纯定位"构造器，Rust 侧 <50ns，加速比取决于 Python baseline 的 evaluate 开销 |
| **Pointer** | 2× seek + subcon.parse + seek 回 fallback（4 次 seek + 1 次 subcon） | Python 同结构 + Subconstruct 包装开销 | Pointer 性能 ≈ subcon 性能（Seek 开销在 Pointer 总开销中占比 <5%） |
| **Prefixed** | lengthfield.parse + stream.read（子流切片）+ subcon.parse + build 方向 temp BuildStream | Python BytesIOWithOffsets 子流构造 + io.BytesIO temp | Prefixed parse ≈ lengthfield.parse + subcon.parse（子流是零拷贝切片，开销 <10ns） |

**数据来源**：
- ExprProgram 求值 ~10ns：Phase 2 perf 数据（`docs/perf-scenarios.csv` Computed 场景）
- ParseStream.seek ~5ns：Phase 4 GreedyRange 回退路径 perf（`docs/perf-scenarios.csv`）
- Python io.BytesIO.seek ~300-500ns：CPython `Modules/_io/bytesio.c` 公开 benchmark 量级

### 6.2 可证伪预测（覆盖所有 FFI/拷贝/转换来源）

**FFI 来源清单**（本设计引入的）：
1. Seek parse 返回新位置 `PyLong`（`stream.tell().into_py(py)`）——每次 Seek.parse 1 次 PyLong 构造（~20-30ns，CPython 小整数缓存仅对 -5..256 有效）
2. Pointer/Prefixed 无新增 FFI（subcon 内部已有的 FFI 不计）
3. Prefixed parse：lengthfield.parse 返回的 PyLong → `extract::<i64>()`（1 次 PyLong→i64 转换，~10ns）

**预测**：
- **Seek**（纯定位）：Rust 侧 ~30-50ns（seek + PyLong 构造），Python baseline ~400ns（evaluate + io.seek），预测 **≥8x**。若 ExprProgram 路径（at 是表达式），加 ~10ns，仍 ≥7x。
- **Pointer**（Struct 内单字段）：性能由 subcon 主导，Pointer 额外开销 4× seek（~20-40ns）。预测 Pointer+Bytes(1) **≥10x**（Bytes(1) 已 13-17x，Pointer 开销占比 <3%）。
- **Prefixed**（VarInt + GreedyRange）：性能由 subcon 主导。预测 Prefixed(VarInt, GreedyRange(Int32ul)) **≥10x**（VarInt + Int32ul + 子流切片，子流零拷贝）。

**可证伪条件**：
- 若 Seek <5x：怀疑 PyLong 构造 FFI 被低估，或 Python baseline 比 io.BytesIO.seek 更快（极少见）。
- 若 Pointer <8x：怀疑 seek 回 fallback 在错误路径触发（但成功路径无此问题），或 4× seek 开销被低估。
- 若 Prefixed <8x：怀疑 temp BuildStream 分配开销（build 方向），或子流切片边界检查。

### 6.3 量级标注（L-09 对策）

- 本节所有 ns 级数字是**量级参考**，非精确值（L-09：ns 级效应常在测量噪声内）。
- 实际性能以 Phase 7.3 bench 实测为准（Controlled A/B Test，子进程隔离）。
- 不声称"Seek 节省 ~10ns"等精确值（L-09 对策：ARCH 性能预测章节涉及 ns 级估算时，必须标注"量级参考"）。

## 7. 边界条件清单

### 7.1 Stream 基础设施

| # | 场景 | 输入 | 预期行为 | Python parity |
|---|------|------|---------|--------------|
| S1 | ParseStream.seek_whence whence=Start 负 offset | `at=-1, Whence::Start` | Stream Err（"requires non-negative"） | Python BytesIO 视为 0（差异：Rust 严格报错） |
| S2 | ParseStream.seek_whence whence=Start 超末尾 | `at=100, data.len()=10` | Stream Err（"out of bounds"） | Python BytesIO 允许（read 时才报错，差异） |
| S3 | ParseStream.seek_whence whence=Current 向前 | `tell()=3, at=2` | pos=5 | ✅ |
| S4 | ParseStream.seek_whence whence=Current 向后 | `tell()=5, at=-2` | pos=3 | ✅ |
| S5 | ParseStream.seek_whence whence=Current 溢出 | `tell()=usize::MAX, at=1` | Stream Err（checked_add 失败） | Python 无此场景（Python int 无溢出） |
| S6 | ParseStream.seek_whence whence=End 正 offset | `at=1, Whence::End` | Stream Err（"requires non-positive"） | Python BytesIO 允许（read 时报错，差异） |
| S7 | ParseStream.seek_whence whence=End 负 offset | `data.len()=10, at=-3` | pos=7 | ✅ |
| S8 | ParseStream.seek_whence 重置 bit_pos | bit_pos=4 后 seek | bit_pos=0 | ✅（与现有 seek 一致） |
| S9 | BuildStream.seek whence=Start | `at=5` | pos=5（不扩展 buf） | ✅ |
| S10 | BuildStream.seek 后 write 零填充 | `seek(10)` + `write(b"X")`, buf.len()=3 | buf 变为 `[0,0,0,0,0,0,0,0,0,0,X]`（len=11） | ✅ Python io.BytesIO 行为 |
| S11 | BuildStream.seek 覆盖写 | buf=`[1,2,3]`, `seek(1)` + `write(b"X")` | buf=`[1,X,3]`，pos=2 | ✅ |
| S12 | BuildStream.seek whence=End 正 offset | `buf.len()=5, at=3` | pos=8（下次 write 零填充） | ✅ Python 允许 |
| S13 | BuildStream.seek whence=Current 负 | `pos=5, at=-2` | pos=3 | ✅ |
| S14 | BuildStream.seek whence=Current 负溢出 | `pos=1, at=-5` | Stream Err（new_pos<0） | ✅ |
| S15 | BuildStream.seek 重置 bit_pos/current_byte | bit_pos=4 后 seek | bit_pos=0, current_byte=0 | 文档化（bit 域内 seek 不支持） |

### 7.2 SeekNode

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| SK1 | 常量 at | `Seek(5)` parse on `b"01234x"` | pos=5，返回 PyLong(5) |
| SK2 | 表达式 at | `Seek(off)` parse，off=3 | pos=3，返回 PyLong(3) |
| SK3 | whence=Current | `Seek(2, whence=1)` parse，tell()=3 | pos=5 |
| SK4 | whence=End | `Seek(-2, whence=2)` parse，data.len()=10 | pos=8 |
| SK5 | build 接受 None | `Seek(5).build(None)` | seek 执行，Ok(()) |
| SK6 | sizeof | `Seek(5).sizeof()` | Err（Generic） |
| SK7 | at 表达式字段缺失 | `Seek(missing)` parse | ExprFieldMissing（path 含 "at"） |

### 7.3 PointerNode

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| P1 | 正 offset 绝对定位 | `Pointer(8, Bytes(1))` parse on 12 字节 | seek 到 8，读 1 字节，seek 回 fallback，返回 bytes |
| P2 | 负 offset 从 EOF | `Pointer(-2, Bytes(1))` parse on `b"abcdefgh"` | seek 到 6（len-2），读 "g"，seek 回 fallback |
| P3 | relativeOffset=True | `Pointer(2, Bytes(1), relativeOffset=True)` parse，tell()=3 | seek 到 5（whence=1），读，seek 回 3 |
| P4 | build 零填充 | `Pointer(8, Bytes(1)).build(b"Z")` | 输出 `b'\x00'*8 + b'Z'`（9 字节） |
| P5 | sizeof | `Pointer(8, Bytes(1)).sizeof()` | Ok(0) |
| P6 | subcon.parse 失败仍 seek 回 | subcon EOF（Bytes(10) on 5 字节） | seek 回 fallback 后返回 Stream Err |
| P7 | offset 表达式 | `Pointer(off, Bytes(1))` | 求值 off，seek，subcon，seek 回 |
| P8 | stream 参数非 None | `Pointer(8, Bytes(1), stream=foo)` | 编译期 Compilation Err（已知限制：不支持换流） |

### 7.4 PrefixedNode

| # | 场景 | 输入 | 预期行为 |
|---|------|------|---------|
| PF1 | 基本 parse | `Prefixed(Byte, Bytes(3))` on `b"\x03abc"` | lengthfield=3，读 "abc"，subcon.parse 返回 b"abc" |
| PF2 | subcon 消费少于 length | `Prefixed(Byte, Bytes(1))` on `b"\x05abcde"` | subcon 读 1 字节，剩余 4 字节被忽略（主流 pos 推进 1+5=6） |
| PF3 | subcon 消费多于 length | `Prefixed(Byte, Bytes(10))` on `b"\x03abc"` | subcon 在子流（3 字节）上 Bytes(10) EOF → Stream Err |
| PF4 | includelength=True | `Prefixed(Int16ub, Bytes(N), includelength=True)` | length 减 Int16ub.sizeof(2) 后才是子流字节数 |
| PF5 | lengthfield 负数 | `Prefixed(Int8sb, Bytes(3))` on `b"\xff..."` | Range Err（length=-1） |
| PF6 | lengthfield 非整数 | `Prefixed(Bytes(2), Bytes(3))` | Range Err（"did not produce an integer"） |
| PF7 | build 基本路径 | `Prefixed(Byte, Bytes(3)).build(b"abc")` | 输出 `b"\x03abc"` |
| PF8 | build lengthfield 溢出 | `Prefixed(Byte, Bytes(N))` build N=300 | lengthfield.build(300) 自报错（Range Err，path 含 "lengthfield"） |
| PF9 | sizeof | `Prefixed(Int16ub, Bytes(3)).sizeof()` | Ok(2+3=5) |
| PF10 | sizeof subcon 不可知 | `Prefixed(Byte, GreedyBytes).sizeof()` | Err（subcon.sizeof 失败） |

## 8. 与其他模块的交互

### 8.1 依赖（已设计/已实现）

- **`stream.rs`**（ParseStream/BuildStream）：扩展对象，本设计新增 Whence/seek_whence/seek。
- **`expr.rs`**（ExprProgram + eval_expr_int）：Pointer/Seek 的 offset/at 表达式求值（Phase 2，已实现）。
- **`context.rs`**（Context）：表达式求值的字段读取（Phase 2.5 Vec 化，已实现）。
- **`nodes/prefixed_array.rs`**：Prefixed 的模式参考（子流 + temp build）。**不复用代码**（见 §1.3）。
- **`nodes/peek.rs`**：Pointer 错误路径 seek 回 fallback 的模式参考。
- **`nodes/tell.rs`**：Pointer save/restore 的 fallback 概念来源。
- **`nodes/subconstruct.rs`**（Phase 6.3）：Pointer 是 Subconstruct 的"流定位"特化（持有 inner + 转发 + seek 逻辑）。
- **`compile.rs`**（Descriptor 编译）：3 个新 build_*_node 函数，参照 PascalString/StopIf/PrefixedArray 编译模式。
- **`error.rs`**（ConstructError）：复用现有 Stream/Range/Generic 变体，**不新增变体**。

### 8.2 被依赖（后续 Phase）

- **Phase 7.3 集成测试**：bench + parity 场景依赖本设计的 3 个 Node。
- **Phase 8+**：OffsettedEnd（需 whence=End + 子流）、Restreamed（需 seek 基础设施）、Lazy/LazyBound（需 seek + 子流）。
- **用户面**：常见协议字段（文件头指针、长度前缀记录、流定位填充）。

### 8.3 ADR 关联

- **ADR-014**（RepeatUntil 终止表达式 = Phase 2 表达式）：本设计 Pointer/Seek 的 offset/at 表达式复用同一 ExprProgram 路径，脉络一致。
- **ADR-016**（P0-3 lazy path 错误传播）：Pointer/Prefixed 在 subcon Err 时用 `push_path_segment` 重建 path（"lengthfield"/"offset"）。
- **ADR-017**（Vec 中转 PyList）：本设计无 PyList 构造（Prefixed 是单元素），不适用。
- **ADR-018**（index save/restore）：本设计不涉及 _index（非数组），不适用。

## 9. DEV 实施清单

### 9.1 实施顺序（建议）

1. **Stream 基础设施扩展**（`stream.rs`）
   - 新增 `Whence` enum + `from_python_int`
   - 新增 `ParseStream::seek_whence`
   - `BuildStream` 加 `pos` 字段 + 修改 `write`/`tell` + 新增 `seek`/`written_len`
   - **单元测试**：S1-S15 全部场景
   - **回归测试**：现有 stream.rs 测试全 PASS（验证向后兼容）
   - **关键审计**：`write_bits`/`write_padding_bits` 加 `debug_assert!(self.pos == self.buf.len())`

2. **SeekNode**（`nodes/seek.rs`）—— 最简，直接用扩展后的 seek
   - 实现 SeekNode + SeekOffset
   - SK1-SK7 单元测试
   - 加入 Node enum + has_expressions

3. **PointerNode**（`nodes/pointer.rs`）—— 复用 Peek 模式
   - 实现 PointerNode + PointerOffset + compute_whence
   - P1-P8 单元测试（重点 P6 错误路径 seek 回、P4 build 零填充）
   - 加入 Node enum + has_expressions

4. **PrefixedNode**（`nodes/prefixed.rs`）—— 复用 PrefixedArray 子流 + temp build
   - 实现 PrefixedNode
   - PF1-PF10 单元测试
   - 加入 Node enum + has_expressions

5. **编译系统**（`compile.rs` + Python `_descriptors.py`）
   - Python 侧 3 个 Descriptor 类 + `_extract_and_compile_exprs` 递归
   - Rust 侧 3 个 build_*_node + match 分派
   - 端到端测试：Python 用户面 `Seek`/`Pointer`/`Prefixed` → Rust Node → parse/build

### 9.2 质量门禁（每次出口必过）

- `cargo build` + `cargo clippy`（零 warning）+ `cargo fmt --check`
- `cargo test` 全 PASS（含新增 + 现有回归）
- **关键**：现有 stream.rs 测试（1362 行）必须全 PASS，验证 BuildStream 引入 pos 字段不破坏向后兼容

### 9.3 已知限制（DEV 须在代码注释 + 用户文档标注）

1. Pointer `stream` 参数（换流）不支持（编译期拒绝非 None）
2. Pointer/Seek 在 Bitwise 域内行为未定义（bit API 假设末尾追加）
3. Seek build 不返回新位置（construct-rs build 签名固定）
4. ParseStream.seek_whence whence=End 正 offset 报错（Python BytesIO 允许，差异）

## 10. parity 测试模板（Phase 7.3 用）

### 10.1 Seek parity

```python
from construct import Seek, Bytes, Sequence

# 1. 常量 at，whence=0
d = Sequence(Seek(5), Bytes(1))
assert d.parse(b"01234x")[1] == b"x"

# 2. whence=1（相对）
d = Sequence(Bytes(3), Seek(2, 1), Bytes(1))
assert d.parse(b"abcde")[2] == b"e"  # 跳到 pos=5

# 3. whence=2（EOF）
d = Sequence(Seek(-2, 2), Bytes(1))
assert d.parse(b"abcdefgh")[1] == b"g"  # 从末尾退 2

# 4. build 覆盖
d = Sequence(Bytes(10), Seek(5), Bytes(1))
assert d.build([b"0123456789", None, b"X"]) == b"01234X6789"
```

### 10.2 Pointer parity

```python
from construct import Pointer, Bytes

# 1. 正 offset（绝对）
d = Pointer(8, Bytes(1))
assert d.parse(b"abcdefghijkl") == b"i"
assert d.build(b"Z") == b'\x00\x00\x00\x00\x00\x00\x00\x00Z'

# 2. 负 offset（从 EOF）
d = Pointer(-2, Bytes(1))
assert d.parse(b"abcdefgh") == b"g"

# 3. relativeOffset=True
d = Pointer(2, Bytes(1), relativeOffset=True)
# 在 Sequence 内：先 Bytes(3) 让 tell=3，再 Pointer 相对 +2 → pos=5
ds = Sequence(Bytes(3), Pointer(2, Bytes(1), relativeOffset=True))
assert ds.parse(b"abcdefg")[1] == b"f"

# 4. Struct 内 Pointer 不占主流位置
from construct import Struct
d = Struct("ptr"/Pointer(5, Bytes(1)), "direct"/Bytes(1))
result = d.parse(b"01234Xy")
assert result.ptr == b"X"
assert result.direct == b"0"  # direct 从 pos=0 读（Pointer 已 seek 回）
```

### 10.3 Prefixed parity

```python
from construct import Prefixed, VarInt, GreedyRange, Int32ul, Bytes, Byte

# 1. 基本 parse
d = Prefixed(VarInt, GreedyRange(Int32ul))
assert d.parse(b"\x08abcdefgh") == [1684234849, 1751606885]

# 2. 基本 build
d = Prefixed(Byte, Bytes(3))
assert d.build(b"abc") == b"\x03abc"

# 3. includelength=True
from construct import Int16ub
d = Prefixed(Int16ub, Bytes(3), includelength=True)
# lengthfield=5（3 字节 + 2 字节 lengthfield），读 3 字节
assert d.parse(b"\x00\x05abc") == b"abc"

# 4. subcon 消费少于 length（忽略剩余）
d = Prefixed(Byte, Bytes(1))
assert d.parse(b"\x05abcde") == b"a"  # 只读 1 字节，剩余 4 字节在子流内被忽略

# 5. round trip
d = Prefixed(Byte, Bytes(3))
data = d.build(b"xyz")
assert d.parse(data) == b"xyz"
```

### 10.4 parity 验证范围（Phase 7.3）

| 构造器 | parity 场景数 | 重点验证 |
|--------|------------|---------|
| Seek | 4-6 | whence 三种 + build 覆盖 |
| Pointer | 5-8 | 正/负 offset + relativeOffset + build 零填充 + Struct 内不占位 |
| Prefixed | 5-8 | 基本 + includelength + subcon 不完全消费 + round trip |
| Stream 扩展 | 隐式（通过上述） | seek_whence/seek 行为由 Pointer/Seek 验证 |

---

## 附录 A：BuildStream 向后兼容性审计

BuildStream 引入 `pos` 字段后，现有调用方的审计：

| 调用方 | 使用的方法 | 引入 pos 后行为 | 兼容性 |
|--------|----------|--------------|--------|
| `StructNode::build` | `write` + `tell`（RO 字段取位置） | pos == buf.len()，write 追加，tell 返回末尾 | ✅ 无变更 |
| `BitwiseNode::build` | `write_bits`/`write_padding_bits`（bit API） | bit API 不感知 pos（仍 buf.push 末尾） | ⚠️ 加 `debug_assert!(pos==len)` 守卫 |
| `BytesNode::build` | `write` | pos == buf.len()，追加 | ✅ |
| `FormatFieldNode::build` | `write_bits`（bit 域）/ `write`（字节域） | 字节域 pos == len；bit 域 debug_assert 守卫 | ✅ |
| `TellNode` build no-op | 无 stream 调用 | — | ✅ |
| `PeekNode::build` no-op | 无 stream 调用 | — | ✅ |
| `PrefixedArrayNode::build` | `write`（countfield + 元素） | pos == len，追加 | ✅ |
| **新增 PointerNode/PrefixedNode build** | `seek` + `write` | pos 与 len 可能分离（零填充/覆盖） | 本设计引入 |

**结论**：除 bit 域 API 加 debug_assert 守卫外，现有代码零修改。向后兼容性保证的关键是：**所有现有 build 调用方不调 seek，pos 始终等于 buf.len()**。

## 附录 B：与 Python BytesIO 行为差异汇总

| 行为 | Python io.BytesIO | construct-rs ParseStream/BuildStream | 差异原因 |
|------|------------------|------------------------------------|---------|
| seek 超末尾（parse） | 允许（read 时报错） | ParseStream.seek_whence 报错 | 提前报错更清晰（parse 超末尾无意义） |
| seek 超末尾（build） | 允许（write 时零填充） | BuildStream.seek 允许（write 时零填充） | ✅ 一致 |
| seek whence=End 正 offset | 允许 | ParseStream 报错 / BuildStream 允许 | parse 超末尾无意义；build 允许（对齐 Python） |
| BytesIOWithOffsets parent_offset | 子流 tell() 加父偏移 | construct-rs 子流是独立 ParseStream（tell 从 0 起） | 文档化差异（子流内 tell 是子流相对位置） |

**BytesIOWithOffsets 差异影响**：Python `Prefixed` 子流的 `Tell()` 返回父流绝对偏移。construct-rs 子流 `Tell` 返回子流相对位置（0-based）。这影响在 Prefixed 子流内用 `Tell` 字段的用户代码。

**决策**：文档化为已知差异，不模拟 parent_offset（增加复杂度且极少用）。用户若需父流绝对位置，应在 Prefixed 外层用 Tell。

---

> **设计完成时间**：2026-07-30
> **下一步**：REV 设计检视（重点查 §5 §0 对照表 + BuildStream 向后兼容性审计 + 边界条件 P6 错误路径 seek 回）

