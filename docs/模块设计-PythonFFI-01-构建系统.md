# 模块设计：Python FFI 构建系统（10.1）

## 模块位置
`construct-py/`（新建，仓库根目录下的独立 crate）

## 职责
搭建 PyO3 扩展模块的构建链与 Python 包骨架，使 `maturin develop` + `import construct_rust` 可用，为后续 10.2-10.10 提供注册框架。

## 详细设计

### 1. Cargo.toml（`construct-py/Cargo.toml`）

```toml
[package]
name = "construct-py"
version = "0.1.0"
edition = "2021"
rust-version = "1.70"

[lib]
name = "construct_rust"          # PyO3 模块符号名
crate-type = ["cdylib"]          # Python 扩展必须 cdylib

[dependencies]
construct = { path = "../construct-rs", features = ["byteorder", "bigint", "compression"] }
pyo3 = { version = "0.22", features = ["extension-module"] }

[profile.release]
opt-level = 3
lto = true
```

**要点**：
- `crate-type = ["cdylib"]`：Python 加载 .so/.pyd 必需
- `features` 启用 construct 的全部可选特性（bigint/compression 被 Phase 10 测试需要）
- `pyo3` 不启用 `abi3`：保留调试便利，正式发布可加 `abi3-py37`

### 2. Workspace 改造（需 PM 确认）

总纲要求 construct-py 为 workspace member。当前 `construct-rs/Cargo.toml` 是独立 crate（仓库根无 workspace）。

**推荐方案**：仓库根新建 `Cargo.toml`：
```toml
[workspace]
resolver = "2"
members = ["construct-rs", "construct-py"]
```
> 影响：construct-rs 的 `[profile.release]` 配置迁移到 workspace 根（profile 不允许子 crate 覆盖）。此项改动**超出 ARCH 权限**，需 PM 授权修改 construct-rs/Cargo.toml。

**备选方案**（无需 workspace）：construct-py 用 `path = "../construct-rs"` 直接依赖，不并入 workspace。构建行为无差异，仅 `cargo build` 不能在根目录一次性编译两者。

### 3. pyproject.toml（`construct-py/pyproject.toml`）

```toml
[build-system]
requires = ["maturin>=1.4,<2.0"]
build-backend = "maturin"

[project]
name = "construct_rust"
requires-python = ">=3.8"
description = "Rust-backed port of Python construct"

[tool.maturin]
module-name = "construct_rust._core"   # native lib → construct_rust/_core.so
python-source = "."                    # Python 包源在 construct-py/construct_rust/
features = ["pyo3/extension-module"]
```

**关键**：`module-name = "construct_rust._core"` —— 原生扩展作为 `construct_rust` 包的 `_core` 子模块，Python 层 `__init__.py` 再做 `from ._core import *`，分离原生/纯 Python 代码。

### 4. src/lib.rs 骨架

```rust
use pyo3::prelude::*;

/// PyO3 模块入口。
#[pymodule]
fn _core(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // 10.1 仅注册空占位；后续子任务在此 m.add_class()/add_function()
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    // 注册异常基类（10.2 填充）
    // m.add("ConstructError", ...)?;
    Ok(())
}
```

**注意**：`#[pymodule]` 函数名 `=_core` 与 pyproject 的 `module-name` 尾段一致。各子任务（10.2-10.9）在 `#[pymodule]` 函数体内追加 `m.add_class::<X>()`，不在 lib.rs 内实现类逻辑（类定义放在各自 `src/*.rs` 文件）。

### 5. Python 包结构

**`construct_rust/__init__.py`**：
```python
from ._core import *           # 原生扩展导出
from .version import *         # 版本信息
# 10.7 混合架构：在此补充纯 Python 基类（Adapter/Validator/Subconstruct）
```

**`construct_rust/version.py`**（纯 Python，对齐原版 version.py）：
```python
version = (2, 10, 70)
version_string = "2.10.70"
release_date = "2026.06.15"
```

**`construct_rust/lib/__init__.py`**（re-export，对齐原版）：
```python
from .containers import *      # 10.3: Container/ListContainer（由 _core 再 re-export）
from .binary import *          # 10.9 纯 Python
from .bitstream import *       # 10.9
from .hex import *             # 10.9
from .py3compat import *       # 10.9
```
> Container/ListContainer 在 10.3 由 Rust `#[pyclass]` 实现，导出后通过 `from .._core import Container, ListContainer` 在 lib/__init__.py re-export，保持原版导入路径 `construct.lib.Container` 可用。

### 6. conftest.py（`construct-py/tests/conftest.py`）

```python
import sys
import construct_rust
sys.modules['construct'] = construct_rust
```

**机制**：在任何测试文件 `import construct` 之前执行（pytest 自动加载 conftest.py），将 `construct` 名字指向我们的 `construct_rust` 包。测试代码无需改动，原版 `from construct import *` 直接命中。

**边界**：
- 必须在测试模块顶层 import 完成**之前**注入（conftest.py 满足）
- 若测试同时 import 了真正原版 `construct`（如 doctest），会冲突——本 Phase 不支持，需先卸载原版

## 与 Python 版本对应

| Python 原版 | Rust/PyO3 对应 |
|------------|---------------|
| `construct/__init__.py` 的 `from construct.core import *` | `construct_rust/__init__.py` 的 `from ._core import *` |
| `construct/lib/__init__.py` re-export 链 | `construct_rust/lib/__init__.py` 同构 re-export |
| `construct.version` 模块 | `construct_rust/version.py`（纯 Python） |
| pip install 后 `import construct` | `sys.modules` 注入（测试用） |

## 边界条件清单

- **crate-type 必须 cdylib**：误用 dylib 会导致 Python 无法加载
- **module-name 尾段 = #[pymodule] 函数名**：不一致导致 ImportError
- **python-source 路径**：maturin 据此查找 `construct_rust/` Python 包
- **requires-python >= 3.8**：PyO3 0.22 最低支持 3.7，但 `from __future__` 等用 3.8 语法
- **workspace 改造副作用**：profile 上移可能影响 bench 配置（criterion）
- **多 Python 版本**：`maturin develop` 绑定当前 venv 的解释器，CI 需矩阵测试

## 与其他模块的交互

- **依赖**：`construct` crate（Phase 1-9 全部），`pyo3` 0.22+
- **被依赖**：所有 10.2-10.9 子任务在此框架内追加注册代码
- **前置约束**：无（本任务为 Phase 10 起点）

## 验证标准（DEV 自检 + REF 验证）

1. `cd construct-py && maturin develop` 成功生成 wheel
2. `python -c "import construct_rust; print(construct_rust.__version__)"` 输出版本号
3. `python -c "import construct_rust; print(construct_rust._core.__version__)"` 原生模块可访问
4. `python -c "import construct_rust as c; print(c.lib)"` lib 子包可访问
5. `python -c "import sys; import construct"` 失败（未注入），但在 tests/ 下 conftest 注入后可用
