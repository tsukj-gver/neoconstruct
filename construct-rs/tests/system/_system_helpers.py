"""系统测试共享工具：协议 CRC 计算、子进程 parity runner。

设计依据：
- Phase 9 总纲（集成测试，不引入新构造器）
- 已有 parity helper 在 tests/_helpers/parity.py（通用框架，本模块提供更轻量的封装）

本模块直接被各 test_*.py 导入，无 pytest 依赖（pure functions）。
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

# ---------------------------------------------------------------------------
# 路径常量（与 tests/conftest.py 保持一致）
# ---------------------------------------------------------------------------

_TEST_DIR = Path(__file__).resolve().parent          # construct-rs/tests/system
_PROJECT_ROOT = _TEST_DIR.parent.parent              # construct-rs/
_CRS_PYTHON_DIR = _PROJECT_ROOT / "python"           # construct-rs/python/


def _venv_python(venv_name: str) -> Path | None:
    """返回项目内 venv 的 python 可执行文件路径（兼容 Windows / POSIX 布局）。

    venv_name 为 construct-rs/ 下的 venv 目录名；未找到时返回 None
    （由调用方走后续 fallback）。
    """
    venv_dir = _PROJECT_ROOT / venv_name
    for rel in ("Scripts/python.exe", "bin/python"):
        candidate = venv_dir / rel
        if candidate.exists():
            return candidate
    return None


# 项目内 venv（与 tests/conftest.py 同一约定，详见 README「测试环境变量」）
_RS_PYTHON = _venv_python(".venv")     # construct-rs 扩展
_PY_PYTHON = _venv_python(".venv-pc")  # 原版参考 construct==2.10.70


def get_rs_python() -> str:
    """CRS venv python.exe（含 construct-rs wheel）。

    解析顺序（与 tests/conftest.py 的 ``rs_python`` fixture 一致，P8 修复）：
      1. 环境变量 CRS_PYTHON（路径存在时优先）
      2. 项目内 venv construct-rs/.venv
      3. sys.executable（最后手段，可能两个 construct 冲突）
    """
    env = os.environ.get("CRS_PYTHON")
    if env and Path(env).exists():
        return env
    if _RS_PYTHON is not None:
        return str(_RS_PYTHON)
    return sys.executable


def get_py_python() -> str:
    """PC venv python.exe（含 Python construct 2.10.70）。

    解析顺序同 ``get_rs_python``，环境变量名 PC_PYTHON，项目内 venv 为 .venv-pc。
    """
    env = os.environ.get("PC_PYTHON")
    if env and Path(env).exists():
        return env
    if _PY_PYTHON is not None:
        return str(_PY_PYTHON)
    return sys.executable


def get_crs_python_dir() -> str:
    """construct-rs/python/ 目录。"""
    return str(_CRS_PYTHON_DIR)


# ---------------------------------------------------------------------------
# CRC / 校验和工具
# ---------------------------------------------------------------------------


def modbus_crc16(data: bytes) -> bytes:
    """Modbus RTU CRC-16（polynomial 0xA001 reversed, init 0xFFFF）。

    返回 2 字节 little-endian（与 Modbus 协议一致）。

    >>> modbus_crc16(bytes([0x01, 0x04, 0x02, 0xFF, 0xFF])).hex()
    '8028'
    """

    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 0x0001:
                crc = (crc >> 1) ^ 0xA001
            else:
                crc >>= 1
    # Modbus little-endian: low byte first
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])


def ipv4_checksum(data: bytes) -> bytes:
    """IPv4 header checksum (RFC 1071 ones-complement sum, big-endian 2 bytes)。

    算法：以 16-bit word 为单位求和，进位回卷，取反。
    长度为奇数时末尾补 0x00。
    返回 **big-endian** 2 字节（与 IPv4 报文头字节序一致）。

    >>> # header bytes: 45 00 00 3c 1c 46 40 00 40 06 [chksum=0] ac 10 0a 63 ac 10 0a 01
    >>> hdr = bytes.fromhex('4500003c1c46400040060000ac100a63ac100a01')
    >>> ipv4_checksum(hdr).hex()
    'b1f1'
    """

    if len(data) % 2 == 1:
        data = data + b"\x00"
    total = 0
    for i in range(0, len(data), 2):
        word = (data[i] << 8) | data[i + 1]
        total += word
        total = (total & 0xFFFF) + (total >> 16)
    checksum = (~total) & 0xFFFF
    # big-endian: high byte first
    return bytes([(checksum >> 8) & 0xFF, checksum & 0xFF])


# ---------------------------------------------------------------------------
# 子进程 parity runner（轻量版）
# ---------------------------------------------------------------------------


def run_parity_snippet(
    impl: str,
    rs_code: str,
    py_code: str,
) -> dict:
    """在子进程中运行 parity 代码片段，返回 JSON 结果。

    参数：
        impl: "rs" 或 "py"
        rs_code: impl="rs" 时执行的 Python 代码字符串（已含 import 与逻辑），
                 最后必须 `print(json.dumps({...}))` 输出结果
        py_code: impl="py" 时执行的 Python 代码字符串（同上约定）

    返回：
        dict：子进程 stdout 末行 JSON 解析结果

    异常：
        RuntimeError：子进程失败或 JSON 解析失败
    """
    if impl == "rs":
        python_exe = get_rs_python()
        # rs 子进程：把 CRS_PYTHON_DIR 注入 sys.path 最前
        prologue = (
            "import sys\n"
            f"CRS_DIR = {get_crs_python_dir()!r}\n"
            "sys.path[:] = [p for p in sys.path if p != CRS_DIR]\n"
            "sys.path.insert(0, CRS_DIR)\n"
            "for k in list(sys.modules):\n"
            "    if k == 'construct' or k.startswith('construct.'):\n"
            "        del sys.modules[k]\n"
        )
        code = prologue + rs_code
    elif impl == "py":
        python_exe = get_py_python()
        prologue = (
            "import sys\n"
            f"CRS_DIR = {get_crs_python_dir()!r}\n"
            "sys.path[:] = [p for p in sys.path if p != CRS_DIR]\n"
            "for k in list(sys.modules):\n"
            "    if k == 'construct' or k.startswith('construct.'):\n"
            "        del sys.modules[k]\n"
        )
        code = prologue + py_code
    else:
        raise ValueError(f"impl must be 'rs' or 'py', got: {impl!r}")

    result = subprocess.run(
        [python_exe, "-c", code],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )

    if result.returncode != 0:
        raise RuntimeError(
            f"parity 子进程失败 impl={impl}\n"
            f"stderr (tail 2000): {result.stderr[-2000:]}"
        )

    lines = [line for line in result.stdout.strip().splitlines() if line.strip()]
    if not lines:
        raise RuntimeError(
            f"parity 子进程无输出 impl={impl}\n"
            f"stderr (tail 2000): {result.stderr[-2000:]}"
        )

    try:
        return json.loads(lines[-1])
    except json.JSONDecodeError as e:
        raise RuntimeError(
            f"parity JSON 解析失败 impl={impl}\n"
            f"stdout (tail 2000): {result.stdout[-2000:]}\n"
            f"error: {e}"
        )


def run_parity_function(
    rs_factory: str,
    py_factory: str,
    *,
    test_data_expr: str,
    extra_setup: str = "",
) -> dict:
    """通用 parity 模板：构造协议、parse 数据、build、roundtrip。

    参数：
        rs_factory: 字符串形式的 Python 代码，定义名为 `make_protocol()` 的函数，
                    返回 construct-rs 的 StructMixin 子类。字符串内含 import 等。
        py_factory: 字符串形式的 Python 代码，定义 Python construct 的 Struct。
        test_data_expr: Python 表达式（str），评估为 bytes，用作 parse 输入。
        extra_setup: 额外的 setup 代码（如常量定义），rs 与 py 子进程都执行。

    返回：
        dict: {"rs": {...}, "py": {...}}，每项为子进程 JSON 输出
            {
                "parsed": <normalize 后的 parse 结果>,
                "built": <hex 字符串>,
                "roundtrip": <normalize 后的 parse(build(parse(x))) 结果>,
            }
    """
    normalize_src = textwrap.dedent(_NORMALIZE_SRC)

    rs_code = f"""
import json
{normalize_src}
{extra_setup}
{rs_factory}

DATA = {test_data_expr}
proto = make_protocol()
parsed = proto.parse(DATA)
_built = proto.build(parsed) if hasattr(proto, 'build') else None
# For construct-rs StructMixin, build is on instance:
if _built is None and hasattr(parsed, 'build'):
    _built = parsed.build()
roundtrip = proto.parse(_built) if hasattr(proto, 'parse') else None

def _conv(o):
    # dataclass → dict
    if hasattr(o, '__dataclass_fields__'):
        return {{k: _conv(getattr(o, k)) for k in o.__dataclass_fields__}}
    if isinstance(o, (list, tuple)):
        return [_conv(x) for x in o]
    return o

print(json.dumps({{
    'parsed': normalize(_conv(parsed)),
    'built': _built.hex() if _built else '',
    'roundtrip': normalize(_conv(roundtrip)),
}}))
"""

    py_code = f"""
import json
{normalize_src}
{extra_setup}
{py_factory}

DATA = {test_data_expr}
proto = make_protocol()
parsed = proto.parse(DATA)
_built = proto.build(parsed)
roundtrip = proto.parse(_built)

def _conv(o):
    if hasattr(o, 'items'):
        return {{k: _conv(v) for k, v in o.items() if not str(k).startswith('_')}}
    if isinstance(o, (list, tuple)):
        return [_conv(x) for x in o]
    return o

print(json.dumps({{
    'parsed': normalize(_conv(parsed)),
    'built': _built.hex() if _built else '',
    'roundtrip': normalize(_conv(roundtrip)),
}}))
"""

    return {
        "rs": run_parity_snippet("rs", rs_code, py_code),
        "py": run_parity_snippet("py", rs_code, py_code),
    }


# normalize 函数源码（注入子进程用）— 简化版，过滤 _ 开头键 + bytes hex
_NORMALIZE_SRC = '''
def normalize(value):
    """规范化输出值，便于跨实现比对。"""
    if isinstance(value, dict):
        return {
            k: normalize(value[k])
            for k in sorted(value.keys(), key=lambda x: str(x))
            if not str(k).startswith("_")
        }
    if isinstance(value, (list, tuple)):
        return [normalize(v) for v in value]
    if isinstance(value, (bytes, bytearray)):
        return {"__bytes__": bytes(value).hex()}
    if isinstance(value, float):
        if value != value:
            return "__NaN__"
        if value == float("inf"):
            return "__+Inf__"
        if value == float("-inf"):
            return "__-Inf__"
        return round(value, 6)
    return value
'''
