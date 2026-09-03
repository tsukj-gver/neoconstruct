"""pytest 全局配置 + session 级共享 fixtures。

职责：
1. sys.path 操纵：确保项目内 neoconstruct 包优先于 site-packages 中的任何
   安装副本（基准测试可能安装 Python 原版 ``construct==2.10.70`` 作为绝对
   基线；pytest 默认把 site-packages 放在 sys.path 前部，可能遮蔽项目内包）。
2. venv / 路径 fixtures：供 parity helper（子进程隔离）与 bench runner 复用
3. 共享 fixtures：测试数据目录 / 临时文件 / 编码参数化
"""

from __future__ import annotations

import os
import sys
import uuid
from pathlib import Path

import pytest

# ===== 保留：sys.path 操纵（幂等保护） =====
# neoconstruct 的 Python 包源码根目录：tests/ 的上一级 + python/
_CRS_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "python"))

# 把 neoconstruct 的 python/ 目录插入 sys.path 最前部。
# 即使 pyproject.toml 的 pythonpath 配置已起作用，这里也做幂等保护。
if _CRS_ROOT not in sys.path or sys.path[0] != _CRS_ROOT:
    sys.path.insert(0, _CRS_ROOT)

# 清除任何已缓存的 construct 模块（以防之前在其他 conftest 阶段被预导入）。
for _key in list(sys.modules):
    if _key == "construct" or _key.startswith("construct."):
        del sys.modules[_key]

# ===== 路径常量（供 fixture 与 helper 复用） =====
_TESTS_DIR = Path(__file__).resolve().parent
_PROJECT_ROOT = _TESTS_DIR.parent                  # neoconstruct/
_CRS_PYTHON_DIR = _PROJECT_ROOT / "python"         # neoconstruct/python/

# 子进程隔离用的 venv 解析（现行约定，详见仓库 README「测试环境变量」小节）：
#   1. 环境变量 CRS_PYTHON / PC_PYTHON（CI 与换机覆盖用）
#   2. 项目内 venv：neoconstruct/.venv（neoconstruct 扩展）与
#      neoconstruct/.venv-pc（原版参考 construct==2.10.70）
#   3. sys.executable（最后手段，仅当该解释器已装对应包时可用）


def _venv_python(venv_name: str) -> Path | None:
    """返回项目内 venv 的 python 可执行文件路径（兼容 Windows / POSIX 布局）。

    venv_name 为 neoconstruct/ 下的 venv 目录名（如 ``.venv``）；目录不存在
    或未找到 python 可执行文件时返回 None（由调用方走后续 fallback）。
    """
    venv_dir = _PROJECT_ROOT / venv_name
    for rel in ("Scripts/python.exe", "bin/python"):
        candidate = venv_dir / rel
        if candidate.exists():
            return candidate
    return None


# ===== session 级 venv / 路径 fixtures =====

@pytest.fixture(scope="session")
def crs_python_dir() -> str:
    """neoconstruct/python/ 目录绝对路径（子进程 sys.path 注入用）。"""
    return str(_CRS_PYTHON_DIR)


@pytest.fixture(scope="session")
def rs_python() -> str:
    """CRS venv 的 python.exe（含 neoconstruct wheel）。

    解析顺序：
      1. 环境变量 CRS_PYTHON（CI 覆盖用）
      2. 项目内 venv neoconstruct/.venv
      3. fallback sys.executable（最后手段，可能两个 construct 冲突）
    """
    env = os.environ.get("CRS_PYTHON")
    if env and Path(env).exists():
        return env
    default = _venv_python(".venv")
    if default is not None:
        return str(default)
    return sys.executable


@pytest.fixture(scope="session")
def py_python() -> str:
    """PC venv 的 python.exe（含 Python construct 2.10.70）。

    解析顺序同 rs_python，环境变量名 PC_PYTHON，项目内 venv 为 .venv-pc。
    """
    env = os.environ.get("PC_PYTHON")
    if env and Path(env).exists():
        return env
    default = _venv_python(".venv-pc")
    if default is not None:
        return str(default)
    return sys.executable


@pytest.fixture(scope="session")
def venv_pair(rs_python, py_python) -> dict:
    """聚合 fixture：返回 venv 配置字典。

    **硬性契约**：dict 的 key 名与 ``run_parity_case`` /
    ``BenchRunner.__init__`` 的形参名一一对齐，调用方可直接
    ``run_parity_case(impl, case_id, case_definitions, **venv_pair)`` 解包。

    返回：
        {"rs_python": str, "py_python": str, "crs_python_dir": str}
    """
    return {
        "rs_python": rs_python,
        "py_python": py_python,
        "crs_python_dir": str(_CRS_PYTHON_DIR),
    }


# ===== 共享 fixtures（pytest 自动发现） =====

@pytest.fixture(scope="session")
def test_data_dir() -> Path:
    """测试数据目录：tests/_helpers/data/（放二进制样本文件）。

    目录可能尚未创建（按需由测试自行 mkdir）；fixture 仅返回路径约定。
    """
    return _TESTS_DIR / "_helpers" / "data"


@pytest.fixture
def tmp_binary_file(tmp_path) -> Path:
    """临时二进制文件工厂，返回 tmp_path 下随机命名的 .bin 文件路径。

    返回 Path 对象（不创建文件，由测试自行 write）。每次调用生成唯一名。
    """
    return tmp_path / f"{uuid.uuid4().hex}.bin"


@pytest.fixture(params=[
    "utf8", "utf-16-le", "utf-16-be", "utf-32-le", "utf-32-be", "ascii",
])
def encoding(request) -> str:
    """编码参数化 fixture，Strings parity 用。

    注意：使用这些 encoding 字符串时需校验其与 Python construct /
    neoconstruct 侧的编码名解析兼容（如 "utf8" vs "utf-8"）。
    Python ``bytes.decode("utf8")`` 可接受，但 neoconstruct 侧需确认。
    """
    return request.param


@pytest.fixture
def case_template():
    """通用 case 模板工厂：返回一个空 dict 供测试填充 case 结构（保留供未来扩展）。"""
    return {"case_id": "", "desc": "", "data": b"", "expected": None}
