"""parity 测试公共组件：子进程隔离 + 比对 + 断言。

公开 API：
    - ``PARITY_SCRIPT_TEMPLATE``：子进程脚本模板常量（模块顶部）
    - ``run_parity_case(impl, case_id, case_definitions, rs_python, py_python,
        crs_python_dir, *, extra_imports="")``：运行单个 parity case
    - ``assert_parity(rs_result, py_result, case_id, *, desc="", ...)``
    - ``assert_fidelity(result, case_id, *, desc="")``
    - ``make_parity_results_fixture(all_cases, case_definitions, *, scope="module")``

case 定义约定：``case_definitions`` 字符串必须定义两个函数：
    - ``_make_case_rs(case_id)`` → ``(cls, parse_data, build_factory, extract)``
    - ``_make_case_py(case_id)`` → ``(fmt, parse_data, build_input)``
子进程模板执行段写死 ``_make_case_rs(CASE)`` / ``_make_case_py(CASE)``（参数化）。
"""

from __future__ import annotations

import inspect
import json
import subprocess
import textwrap

from .normalize import normalize


# ---------------------------------------------------------------------------
# 子进程脚本模板
# ---------------------------------------------------------------------------
#
# 模板用 ``.format()`` 替换 5 个占位符：impl / case / crs_python_dir /
# normalize_src / case_definitions。
#
# 转义规则：模板内**字面**花括号必须转义为 ``{{`` ``}}``
# （因 .format 解析整个模板）；占位符用单花括号。normalize_src 与 case_definitions
# 作为字符串值注入，其内部的花括号不会被 .format 二次解析（.format 仅替换模板
# 自身的占位符），故 case 定义代码可自由使用 ``{`` ``}``。

PARITY_SCRIPT_TEMPLATE: str = """\
import json
import sys

IMPL = {impl!r}
CASE = {case!r}
CRS_PYTHON_DIR = {crs_python_dir!r}

# ---- sys.path 操纵 ----
if IMPL == 'rs':
    sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]
    sys.path.insert(0, CRS_PYTHON_DIR)
else:
    sys.path[:] = [p for p in sys.path if p != CRS_PYTHON_DIR]
for k in list(sys.modules):
    if k == 'construct' or k.startswith('construct.'):
        del sys.modules[k]

# ---- normalize（从父进程注入，避免重复实现） ----
{normalize_src}

# ---- case 定义（从父进程注入） ----
{case_definitions}

# ---- 执行 ----
parsed = None
built = None
roundtrip_parsed = None

if IMPL == 'rs':
    cls, parse_data, build_factory, extract = _make_case_rs(CASE)
    parsed_obj = cls.parse(parse_data)
    parsed = extract(parsed_obj)
    built = build_factory().build()
    reparsed = cls.parse(built)
    roundtrip_parsed = extract(reparsed)
else:
    fmt, parse_data, build_input = _make_case_py(CASE)
    parsed_obj = fmt.parse(parse_data)
    parsed = dict(parsed_obj) if hasattr(parsed_obj, 'items') else parsed_obj
    built = fmt.build(build_input)
    reparsed = fmt.parse(built)
    roundtrip_parsed = dict(reparsed) if hasattr(reparsed, 'items') else reparsed

print(json.dumps({{
    'impl': IMPL,
    'case': CASE,
    'parsed': normalize(parsed),
    'built': built.hex(),
    'roundtrip_parsed': normalize(roundtrip_parsed),
}}))
"""


# ---------------------------------------------------------------------------
# 公开函数
# ---------------------------------------------------------------------------

def run_parity_case(
    impl,
    case_id,
    case_definitions,
    rs_python,
    py_python,
    crs_python_dir,
    *,
    extra_imports="",
):
    """在子进程中运行单个 parity case，返回结果字典。

    参数：
        impl: "rs" 或 "py"，决定用哪个 venv 跑
        case_id: case 标识（如 "A1" / "F32-1"）
        case_definitions: 子进程内执行的 Python 代码字符串，
            必须定义 _make_case_rs / _make_case_py 两个函数（见模块 docstring 约定）
        rs_python: CRS venv python.exe（impl="rs" 时使用）
        py_python: PC venv python.exe（impl="py" 时使用）
        crs_python_dir: construct-rs/python/ 目录（sys.path 注入用）
        extra_imports: 可选，注入到 case_definitions 之前的额外 import 语句

    返回：
        dict，schema：
            {
                "impl": str,            # "rs" 或 "py"
                "case": str,            # case_id
                "parsed": Any,          # normalize 后的 parse 结果
                "built": str,           # hex 字符串
                "roundtrip_parsed": Any # normalize 后的 roundtrip parse 结果
            }

    异常：
        RuntimeError: 子进程 returncode != 0，或无 stdout，或 JSON 解析失败。
            错误信息含 stderr 末尾 2000 字符（便于调试）。
    """
    if impl == "rs":
        python_exe = rs_python
    elif impl == "py":
        python_exe = py_python
    else:
        raise ValueError(f"impl must be 'rs' or 'py', got: {impl!r}")

    normalize_src = textwrap.dedent(inspect.getsource(normalize))
    # extra_imports 在 case_definitions 之前、normalize 之后注入（如需额外符号）
    injected_case = extra_imports + ("\n" if extra_imports else "") + case_definitions

    code = PARITY_SCRIPT_TEMPLATE.format(
        impl=impl,
        case=case_id,
        crs_python_dir=crs_python_dir,
        normalize_src=normalize_src,
        case_definitions=injected_case,
    )

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
            f"parity 子进程失败 impl={impl} case={case_id}\n"
            f"stderr (tail 2000): {result.stderr[-2000:]}"
        )

    lines = [line for line in result.stdout.strip().splitlines() if line.strip()]
    if not lines:
        raise RuntimeError(
            f"parity 子进程无输出 impl={impl} case={case_id}\n"
            f"stderr (tail 2000): {result.stderr[-2000:]}"
        )

    try:
        return json.loads(lines[-1])
    except json.JSONDecodeError as e:
        raise RuntimeError(
            f"parity JSON 解析失败 impl={impl} case={case_id}\n"
            f"stdout (tail 2000): {result.stdout[-2000:]}\n"
            f"error: {e}"
        )


def assert_parity(
    rs_result,
    py_result,
    case_id,
    *,
    desc="",
    check_parse=True,
    check_build=True,
    check_roundtrip=True,
):
    """断言 rs 与 py 结果一致（三维度比对）。

    参数：
        rs_result / py_result: run_parity_case 返回的 dict
        case_id / desc: 用于失败消息定位
        check_parse / check_build / check_roundtrip: 可分别关闭某维度

    断言失败：AssertionError，消息含 rs/py 两侧的值对比。
    """
    label = f"case {case_id}" + (f" ({desc})" if desc else "")

    if check_parse:
        rs_parsed = rs_result["parsed"]
        py_parsed = py_result["parsed"]
        assert rs_parsed == py_parsed, (
            f"{label} parse 输出不一致\n"
            f"  Rust:   {rs_parsed}\n"
            f"  Python: {py_parsed}"
        )

    if check_build:
        rs_built = rs_result["built"]
        py_built = py_result["built"]
        assert rs_built == py_built, (
            f"{label} build 输出不一致\n"
            f"  Rust:   {rs_built}\n"
            f"  Python: {py_built}"
        )

    if check_roundtrip:
        rs_rt = rs_result["roundtrip_parsed"]
        py_rt = py_result["roundtrip_parsed"]
        assert rs_rt == py_rt, (
            f"{label} roundtrip 解析不一致\n"
            f"  Rust:   {rs_rt}\n"
            f"  Python: {py_rt}"
        )


def assert_fidelity(
    result,
    case_id,
    *,
    desc="",
):
    """断言单 impl 的 parse → build → parse 保真。

    参数：
        result: run_parity_case 返回的 dict
        case_id / desc: 失败消息定位

    断言：result["parsed"] == result["roundtrip_parsed"]

    用途：在 parity 失败时，分别检查 rs / py 各自的往返保真，
        定位是"两实现差异"还是"单实现 bug"。
    """
    label = f"case {case_id}" + (f" ({desc})" if desc else "")
    impl = result.get("impl", "?")
    first = result["parsed"]
    roundtrip = result["roundtrip_parsed"]
    assert first == roundtrip, (
        f"{label} impl={impl} 往返保真失败\n"
        f"  first parse:     {first}\n"
        f"  roundtrip parse: {roundtrip}"
    )


def make_parity_results_fixture(all_cases, case_definitions, *, scope="module"):
    """工厂：生成 parity_results fixture（预跑全部 case × impl 缓存）。

    参数：
        all_cases: [(case_id, desc), ...] 列表，供 parametrize 复用
        case_definitions: 传给 run_parity_case 的子进程脚本
        scope: pytest fixture scope，默认 "module"

    返回：pytest.fixture 装饰的函数，预跑结果 dict：
        {
            case_id: {"rs": <run_parity_result>, "py": <run_parity_result>},
            ...
        }

    用途：消除每个 parity 测试文件都要手写一遍 parity_results fixture 的重复。
    """
    import pytest

    @pytest.fixture(scope=scope)
    def _fixture(venv_pair):
        results = {}
        for case_id, _ in all_cases:
            results[case_id] = {
                "rs": run_parity_case("rs", case_id, case_definitions, **venv_pair),
                "py": run_parity_case("py", case_id, case_definitions, **venv_pair),
            }
        return results

    return _fixture
