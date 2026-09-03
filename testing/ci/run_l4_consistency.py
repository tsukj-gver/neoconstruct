"""L4 一致性核查（META-CI-1a）.

设计依据：
- docs/design/基础设施/CI冒烟门禁设计.md §4.4（L4 核查清单）
- harness/extensions/auditor-extension.md §第 7 类审计项（10 项 check 规范来源）

10 项 check（按 auditor-extension §第 7 类）：

    C1  CSV 格式合规（每行 13 列 inventory / 17 列 perf-scenarios）
    C2  UTF-8 without BOM
    C3  LF 换行（无 CRLF）
    C4  inventory.csv status=implemented 的 impl_module 指向的源文件确实存在
    C5  inventory.csv perf_data_source 指针（文件:行号）能定位到真实数据
    C6  perf-scenarios.csv data_source 指针能定位到真实数据
    C7  已实现构造器无遗漏（与 nodes/mod.rs + _descriptors.py 比对）
    C8  perf-scenarios.csv meets_10x 与 speedup_x 数值自洽
    C9  status 字段已同步（无"代码已实现但 status 仍为 not_implemented"）
    C10 RFC 4180 引号转义合规

输出：每项 check 给 PASS/FAIL + 详细证据（如 broken ref 列表 / 不一致项列表）。
退出码：0=PASS / 1=FAIL。

编码规范：PEP 484 类型注解 + PEP 257 docstring + 无裸 except / 无 print 调试残留。
"""

from __future__ import annotations

import csv
import io
import json
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

# ---------------------------------------------------------------------
# 项目根定位与目标文件
# ---------------------------------------------------------------------

# run_l4_consistency.py 位于 <root>/testing/ci/
PROJECT_ROOT = Path(__file__).resolve().parents[2]

INVENTORY_CSV = PROJECT_ROOT / "docs" / "constructors-inventory.csv"
PERF_SCENARIOS_CSV = PROJECT_ROOT / "docs" / "perf-scenarios.csv"
NODES_MOD_RS = PROJECT_ROOT / "neoconstruct" / "src" / "nodes" / "mod.rs"
DESCRIPTORS_PY = (
    PROJECT_ROOT / "neoconstruct" / "python" / "neoconstruct" / "_descriptors.py"
)
# 用户面分散到多个模块（_conditional.py / _adapters.py），
# 仅查 _descriptors.py 会漏报。统一扫描 neoconstruct/python/neoconstruct/ 下
# 所有 __all__ 定义。
PYTHON_PKG_DIR = (
    PROJECT_ROOT / "neoconstruct" / "python" / "neoconstruct"
)

INVENTORY_EXPECTED_COLUMNS = 13
PERF_SCENARIOS_EXPECTED_COLUMNS = 17


# ---------------------------------------------------------------------
# 通用数据结构
# ---------------------------------------------------------------------


@dataclass
class CheckResult:
    """单条 check 的结果。"""

    check_id: str
    name: str
    passed: bool
    evidence: List[str] = field(default_factory=list)
    """具体证据列表（如 broken ref 列表 / 不一致项列表）。"""

    def to_dict(self) -> Dict[str, Any]:
        return {
            "check_id": self.check_id,
            "name": self.name,
            "passed": self.passed,
            "evidence": self.evidence,
        }


@dataclass
class L4Report:
    """L4 全部 check 汇总报告。"""

    passed: bool
    checks: List[CheckResult]
    summary: Dict[str, int]

    def to_dict(self) -> Dict[str, Any]:
        return {
            "level": "L4",
            "passed": self.passed,
            "summary": self.summary,
            "checks": [c.to_dict() for c in self.checks],
        }


# ---------------------------------------------------------------------
# 工具函数
# ---------------------------------------------------------------------


def _read_raw_bytes(path: Path) -> bytes:
    """读取原始字节（用于 BOM / CRLF 检测）。"""
    with path.open("rb") as f:
        return f.read()


def _parse_csv_with_header(
    path: Path, expected_columns: int
    ) -> Tuple[Optional[List[str]], List[List[str]], List[str]]:
    """用 Python csv 模块解析 CSV。

    返回 (header, rows, malformed)：
    - header: 表头字段列表（若空文件则为 None）
    - rows: 数据行列表（每行为字段列表）
    - malformed: 行号（1-based，相对文件起点）列表，列数不符的行
    """
    malformed: List[str] = []
    header: Optional[List[str]] = None
    rows: List[List[str]] = []

    text = _read_raw_bytes(path).decode("utf-8", errors="replace")
    # csv.reader 默认按 RFC 4180 解析（含引号转义）
    reader = csv.reader(io.StringIO(text))
    for line_no, fields in enumerate(reader, start=1):
        if header is None:
            header = fields
            continue
        # 跳过空行（csv 模块对末尾 "\n" 返回 []，对真正空行返回 [""]）
        if len(fields) == 0:
            continue
        if len(fields) == 1 and fields[0] == "":
            continue
        rows.append(fields)
        if len(fields) != expected_columns:
            malformed.append(str(line_no))

    return header, rows, malformed


def _parse_pointer(pointer: str) -> List[Tuple[str, List[int]]]:
    """解析 perf_data_source / data_source 字段的指针列表。

    格式（iter9 起支持两种）：
      - 行号格式（历史 / 兼容）：``path:line`` 或 ``path:line-line``，多个用 ``;`` 分隔
      - 文件格式（iter9 引入）：``path``（纯文件路径，无 ``:line``）

    返回 [(file_path, [line_numbers]), ...]。
    - 行号格式：返回行号列表供行级校验
    - 文件格式：返回空行号列表 ``[]``，调用方按文件存在性校验
    - 文件不存在的条目也会返回，供调用方按行号定位核查。
    """
    results: List[Tuple[str, List[int]]] = []
    if not pointer or pointer.strip() == "-":
        return results
    parts = [p.strip() for p in pointer.split(";") if p.strip()]
    for part in parts:
        # 兼容 "harness/MEMORY.md:109" / "plans/xxx.md:60-63" / "neoconstruct/src/.../x.rs:1"
        m = re.match(r"^(.+):(\d+)(?:-(\d+))?$", part)
        if not m:
            # iter9+: 纯文件路径格式（无 :line）—— 作为文件级指针返回空行号列表
            # 调用方应区分：lines==[] 且 part 是合理路径 → 文件存在性校验
            results.append((part, []))
            continue
        file_path = m.group(1)
        start = int(m.group(2))
        end_str = m.group(3)
        if end_str is not None:
            end = int(end_str)
            lines = list(range(start, end + 1))
        else:
            lines = [start]
        results.append((file_path, lines))
    return results


def _resolve_impl_module_path(file_part: str) -> Optional[Path]:
    """把 impl_module 字段的文件部分解析到实际路径。

    impl_module 可能指向：
      - ``nodes/xxx.rs`` / ``descriptors/xxx.rs`` -> ``neoconstruct/src/<part>``
      - ``python/neoconstruct/xxx.py``             -> ``neoconstruct/<part>``
      - 其他相对路径                                -> 依次尝试多个候选根

    返回首个存在的候选路径；都不存在则返回 None。
    """
    candidates = [
        PROJECT_ROOT / "neoconstruct" / "src" / file_part,
        PROJECT_ROOT / "neoconstruct" / file_part,
        PROJECT_ROOT / file_part,
    ]
    for c in candidates:
        if c.exists():
            return c
    return None


# ---------------------------------------------------------------------
# Check 1: CSV 格式合规（列数）
# ---------------------------------------------------------------------


def check_c1_csv_column_counts() -> CheckResult:
    """C1: inventory.csv 13 列 / perf-scenarios.csv 17 列。"""
    evidence: List[str] = []

    _, _, inv_bad = _parse_csv_with_header(
        INVENTORY_CSV, INVENTORY_EXPECTED_COLUMNS
    )
    if inv_bad:
        evidence.append(
            f"inventory.csv rows with wrong column count "
            f"(expected {INVENTORY_EXPECTED_COLUMNS}): lines {inv_bad}"
        )

    _, _, perf_bad = _parse_csv_with_header(
        PERF_SCENARIOS_CSV, PERF_SCENARIOS_EXPECTED_COLUMNS
    )
    if perf_bad:
        evidence.append(
            f"perf-scenarios.csv rows with wrong column count "
            f"(expected {PERF_SCENARIOS_EXPECTED_COLUMNS}): lines {perf_bad}"
        )

    return CheckResult(
        check_id="C1",
        name="CSV column counts (inventory=13 / perf-scenarios=17)",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 2: UTF-8 without BOM
# ---------------------------------------------------------------------


def check_c2_utf8_no_bom() -> CheckResult:
    """C2: 两份 CSV 必须为 UTF-8 编码且无 BOM。"""
    evidence: List[str] = []
    bom = b"\xef\xbb\xbf"
    for path in (INVENTORY_CSV, PERF_SCENARIOS_CSV):
        head = _read_raw_bytes(path)[:3]
        if head.startswith(bom):
            evidence.append(f"{path.name}: starts with UTF-8 BOM (EF BB BF)")
    return CheckResult(
        check_id="C2",
        name="UTF-8 without BOM",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 3: LF line endings (no CRLF)
# ---------------------------------------------------------------------


def check_c3_lf_only() -> CheckResult:
    """C3: 两份 CSV 必须为 LF 换行（无 CRLF）。"""
    evidence: List[str] = []
    for path in (INVENTORY_CSV, PERF_SCENARIOS_CSV):
        data = _read_raw_bytes(path)
        if b"\r\n" in data:
            # 统计 CRLF 出现次数
            count = data.count(b"\r\n")
            evidence.append(
                f"{path.name}: contains {count} CRLF (\\r\\n) sequence(s)"
            )
    return CheckResult(
        check_id="C3",
        name="LF line endings (no CRLF)",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 4: inventory.csv impl_module 指向的源文件确实存在
# ---------------------------------------------------------------------


def check_c4_impl_module_exists() -> CheckResult:
    """C4: status=implemented 的 impl_module 字段每个文件路径必须存在。

    impl_module 字段格式：``nodes/xxx.rs:1;descriptors/mod.rs:1``
    （多个用 ``;`` 分隔，每个含 ``:行号``）
    """
    evidence: List[str] = []
    _, rows, _ = _parse_csv_with_header(
        INVENTORY_CSV, INVENTORY_EXPECTED_COLUMNS
    )
    for row in rows:
        if len(row) < INVENTORY_EXPECTED_COLUMNS:
            continue  # 已由 C1 报告
        name = row[1]
        status = row[3]
        impl_module = row[5]
        if status != "implemented":
            continue
        if not impl_module or impl_module.strip() == "-":
            evidence.append(
                f"{name}: status=implemented but impl_module is empty"
            )
            continue
        # 多个文件用 ; 分隔，每个含 :行号
        for entry in impl_module.split(";"):
            entry = entry.strip()
            if not entry:
                continue
            # 取 : 之前的部分作为文件路径
            file_part = entry.split(":")[0]
            resolved = _resolve_impl_module_path(file_part)
            if resolved is None:
                evidence.append(
                    f"{name}: impl_module file not found: {file_part} "
                    f"(tried neoconstruct/src/, neoconstruct/, project root)"
                )
    return CheckResult(
        check_id="C4",
        name="inventory.csv status=implemented -> impl_module files exist",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 5: inventory.csv perf_data_source 指针可达
# ---------------------------------------------------------------------


def check_c5_inventory_perf_data_source_reachable() -> CheckResult:
    """C5: inventory.csv 中所有 perf_data_source 指针能定位到真实数据。

    perf_data_source 字段格式（iter9 起支持两种）：
      - 行号格式：``plans/xxx.md:60-63;plans/yyy.md:2242-2265``
      - 文件格式：``plans/xxx/traces/X.md``（iter9 拆分后）
    """
    evidence: List[str] = []
    _, rows, _ = _parse_csv_with_header(
        INVENTORY_CSV, INVENTORY_EXPECTED_COLUMNS
    )
    for row in rows:
        if len(row) < INVENTORY_EXPECTED_COLUMNS:
            continue  # 已由 C1 报告
        name = row[1]
        perf_data_source = row[10]
        if not perf_data_source or perf_data_source.strip() == "-":
            continue
        pointers = _parse_pointer(perf_data_source)
        for file_path, lines in pointers:
            full = PROJECT_ROOT / file_path
            if not full.exists():
                evidence.append(
                    f"{name}: perf_data_source file not found: {file_path}"
                )
                continue
            # iter9+: 文件格式（lines==[]）只校验文件存在性，跳过行号校验
            if not lines:
                continue
            # 行号可达性：读文件，看行数是否覆盖
            try:
                with full.open("r", encoding="utf-8", errors="replace") as f:
                    file_lines = f.readlines()
            except OSError as exc:
                evidence.append(
                    f"{name}: cannot read {file_path}: {exc}"
                )
                continue
            total = len(file_lines)
            for ln in lines:
                if ln < 1 or ln > total:
                    evidence.append(
                        f"{name}: perf_data_source line {ln} out of range "
                        f"in {file_path} (total {total} lines)"
                    )
    return CheckResult(
        check_id="C5",
        name="inventory.csv perf_data_source pointers reachable",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 6: perf-scenarios.csv data_source 指针可达
# ---------------------------------------------------------------------


def check_c6_perf_scenarios_data_source_reachable() -> CheckResult:
    """C6: perf-scenarios.csv 中所有 data_source 指针能定位到真实数据。

    data_source 字段格式（iter9 起支持两种）：
      - 行号格式：``plans/xxx.md:5005`` 或多个用 ``;`` 分隔
      - 文件格式：``plans/xxx/traces/X.md``（iter9 拆分后）
    """
    evidence: List[str] = []
    _, rows, _ = _parse_csv_with_header(
        PERF_SCENARIOS_CSV, PERF_SCENARIOS_EXPECTED_COLUMNS
    )
    for row_idx, row in enumerate(rows, start=2):
        if len(row) < PERF_SCENARIOS_EXPECTED_COLUMNS:
            continue  # 已由 C1 报告
        constructor = row[0]
        scenario_id = row[1]
        direction = row[3]
        data_source = row[14]
        if not data_source or data_source.strip() == "-":
            continue
        pointers = _parse_pointer(data_source)
        for file_path, lines in pointers:
            full = PROJECT_ROOT / file_path
            if not full.exists():
                evidence.append(
                    f"row {row_idx} [{constructor}/{scenario_id}/{direction}]: "
                    f"data_source file not found: {file_path}"
                )
                continue
            # iter9+: 文件格式（lines==[]）只校验文件存在性，跳过行号校验
            if not lines:
                continue
            try:
                with full.open("r", encoding="utf-8", errors="replace") as f:
                    file_lines = f.readlines()
            except OSError as exc:
                evidence.append(
                    f"row {row_idx} [{constructor}/{scenario_id}/{direction}]: "
                    f"cannot read {file_path}: {exc}"
                )
                continue
            total = len(file_lines)
            for ln in lines:
                if ln < 1 or ln > total:
                    evidence.append(
                        f"row {row_idx} [{constructor}/{scenario_id}/{direction}]: "
                        f"data_source line {ln} out of range in {file_path} "
                        f"(total {total} lines)"
                    )
    return CheckResult(
        check_id="C6",
        name="perf-scenarios.csv data_source pointers reachable",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 7: 已实现构造器无遗漏（与 nodes/mod.rs + _descriptors.py 比对）
# ---------------------------------------------------------------------


def _load_nodes_mod_modules() -> List[str]:
    """从 nodes/mod.rs 提取 ``pub mod <name>;`` 列表。"""
    if not NODES_MOD_RS.exists():
        return []
    text = NODES_MOD_RS.read_text(encoding="utf-8")
    return re.findall(r"^\s*pub\s+mod\s+(\w+)\s*;", text, flags=re.MULTILINE)


def _load_descriptor_exports() -> List[str]:
    """从 _descriptors.py 的 __all__ 列表提取导出符号。

    Phase 7+ 扩展：用户面分散到多个模块（_conditional.py / _adapters.py），
    统一扫描 PYTHON_PKG_DIR 下所有 .py 文件的 __all__ 列表。
    """
    exports: List[str] = []
    # 优先保留对 DESCRIPTORS_PY 的引用（向后兼容）
    candidate_files = [DESCRIPTORS_PY] if DESCRIPTORS_PY.exists() else []
    # Phase 7+: 扫描整个 Python 包目录下所有 .py 文件
    if PYTHON_PKG_DIR.exists():
        for py_file in sorted(PYTHON_PKG_DIR.glob("*.py")):
            if py_file not in candidate_files:
                candidate_files.append(py_file)

    for py_file in candidate_files:
        text = py_file.read_text(encoding="utf-8")
        # 匹配 __all__ = [ ... ] 块（每个文件最多 1 个）
        m = re.search(r"__all__\s*=\s*\[(.*?)\]", text, flags=re.DOTALL)
        if not m:
            continue
        body = m.group(1)
        exports.extend(re.findall(r'"([^"]+)"', body))
    return exports


def _map_module_to_constructors(module_list: List[str]) -> set:
    """nodes/mod.rs 的 module 名映射为 inventory name 候选集合。

    例如 array / greedy_range / repeat_until / prefixed_array / format_field 等。
    用于覆盖性比对。
    """
    # 已知的 module -> inventory name 候选映射
    # 对没有映射的 module，按 module 名本身加入集合（用于后续报告）
    known: Dict[str, List[str]] = {
        "format_field": ["FormatField"],
        "bytes": ["Bytes"],
        "greedy_bytes": ["GreedyBytes"],
        "struct_node": ["Struct"],
        "struct_ref": ["StructRef"],
        "tell": ["Tell"],
        "computed": ["Computed"],
        "bits_integer": ["BitsInteger", "Bit", "Nibble", "Octet"],
        "bitwise": ["Bitwise", "BitStruct"],
        "bytewise": ["Bytewise"],
        "transform": ["BitsSwapped", "ByteSwapped"],
        "bit_padding": ["Padding"],
        "padding": ["Padding"],
        "array": ["Array"],
        "greedy_range": ["GreedyRange"],
        "prefixed_array": ["PrefixedArray"],
        "repeat_until": ["RepeatUntil"],
        "index": ["Index"],
        "stop_if": ["StopIf"],
        "element": ["Element"],
    }
    result: set = set()
    for mod in module_list:
        result.update(known.get(mod, []))
    return result


def check_c7_no_missing_implemented() -> CheckResult:
    """C7: 已实现构造器无遗漏。

    比对方向：
    1. inventory 中 status=implemented 的构造器 name 必须 ∈ nodes/mod.rs 映射集合
       或 _descriptors.py __all__ 集合（覆盖性，避免漏注册）
    2. perf-scenarios.csv 的 constructor 字段必须 ∈ inventory.csv 的 name 集合
       （设计文档 C10）
    """
    evidence: List[str] = []
    _, inv_rows, _ = _parse_csv_with_header(
        INVENTORY_CSV, INVENTORY_EXPECTED_COLUMNS
    )
    inv_implemented_names: set = set()
    inv_all_names: set = set()
    for row in inv_rows:
        if len(row) < INVENTORY_EXPECTED_COLUMNS:
            continue
        name = row[1]
        inv_all_names.add(name)
        if row[3] == "implemented":
            inv_implemented_names.add(name)

    nodes_modules = _load_nodes_mod_modules()
    node_ctor_set = _map_module_to_constructors(nodes_modules)
    desc_exports = set(_load_descriptor_exports())

    # 比对 1：inventory.implemented - node_ctor_set - desc_exports 应为空
    # （即 inventory 中标 implemented 的，要么在 nodes/mod.rs 注册，
    #   要么在 _descriptors.py 导出，要么二者都有）
    not_registered = inv_implemented_names - node_ctor_set - desc_exports
    for name in sorted(not_registered):
        evidence.append(
            f"inventory name '{name}' is status=implemented but not found in "
            f"nodes/mod.rs modules or _descriptors.py __all__"
        )

    # 比对 2：perf-scenarios.csv constructor 必须 ∈ inventory.csv name 集合
    _, perf_rows, _ = _parse_csv_with_header(
        PERF_SCENARIOS_CSV, PERF_SCENARIOS_EXPECTED_COLUMNS
    )
    perf_constructors: set = set()
    for row in perf_rows:
        if len(row) < 1:
            continue
        perf_constructors.add(row[0])
    unknown_in_perf = perf_constructors - inv_all_names
    for name in sorted(unknown_in_perf):
        evidence.append(
            f"perf-scenarios.csv references constructor '{name}' "
            f"not present in inventory.csv"
        )

    return CheckResult(
        check_id="C7",
        name="No missing implemented (inventory vs nodes/mod.rs + _descriptors.py)",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 8: perf-scenarios.csv meets_10x 与 speedup_x 自洽
# ---------------------------------------------------------------------


def check_c8_meets_10x_self_consistent() -> CheckResult:
    """C8: ``speedup_x >= 10.0`` 必须与 ``meets_10x == true`` 严格对应。

    空值规则：
    - speedup_x 为空 → meets_10x 应为 false（或为空时按 false 处理）
    - meets_10x 为空 → 视为 false
    """
    evidence: List[str] = []
    _, rows, _ = _parse_csv_with_header(
        PERF_SCENARIOS_CSV, PERF_SCENARIOS_EXPECTED_COLUMNS
    )
    for row_idx, row in enumerate(rows, start=2):
        if len(row) < PERF_SCENARIOS_EXPECTED_COLUMNS:
            continue  # 已由 C1 报告
        speedup_str = row[12].strip()
        meets_str = row[13].strip().lower()
        constructor = row[0]
        scenario_id = row[1]
        direction = row[3]

        meets_expected = (meets_str == "true")

        if not speedup_str:
            # 空值允许 meets=false
            if meets_expected:
                evidence.append(
                    f"row {row_idx} [{constructor}/{scenario_id}/{direction}]: "
                    f"speedup_x is empty but meets_10x=true"
                )
            continue

        try:
            speedup = float(speedup_str)
        except ValueError:
            evidence.append(
                f"row {row_idx} [{constructor}/{scenario_id}/{direction}]: "
                f"speedup_x '{speedup_str}' is not a valid float"
            )
            continue

        if (speedup >= 10.0) != meets_expected:
            evidence.append(
                f"row {row_idx} [{constructor}/{scenario_id}/{direction}]: "
                f"speedup_x={speedup:.2f} but meets_10x={meets_str} "
                f"(expected meets_10x={'true' if speedup >= 10.0 else 'false'})"
            )
    return CheckResult(
        check_id="C8",
        name="perf-scenarios.csv meets_10x <-> speedup_x self-consistent",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 9: status 字段已同步（无"代码已实现但 status 仍为 not_implemented"）
# ---------------------------------------------------------------------


def check_c9_status_synced() -> CheckResult:
    """C9: 反向检查——代码已实现但 inventory 仍标 not_implemented。

    实现策略：从 _descriptors.py __all__ 中提取"实际用户面构造器名"
    （剔除 Descriptor 后缀与内部符号），与 inventory 中 status=not_implemented
    的 name 集合做交集。交集即为"代码已导出但 inventory 未同步"。

    nodes/mod.rs 注册是描述符实现细节（通过 compile_schema 间接调用），
    一致性已由 C7 覆盖；本 check 聚焦用户面导出符号与 inventory 的同步。
    """
    evidence: List[str] = []
    _, inv_rows, _ = _parse_csv_with_header(
        INVENTORY_CSV, INVENTORY_EXPECTED_COLUMNS
    )
    inv_not_implemented: set = set()
    for row in inv_rows:
        if len(row) < INVENTORY_EXPECTED_COLUMNS:
            continue
        if row[3] == "not_implemented":
            inv_not_implemented.add(row[1])

    desc_exports = set(_load_descriptor_exports())

    # 剔除内部符号：以 Descriptor 结尾的类名（实现细节，非用户面构造器名）
    user_facing = {
        n for n in desc_exports if not n.endswith("Descriptor")
    }

    # 已知 not_implemented 但代码确实未实现（不在 __all__）的不会被报告
    inconsistent = user_facing & inv_not_implemented
    for name in sorted(inconsistent):
        evidence.append(
            f"constructor '{name}' is exported in _descriptors.py __all__ "
            f"but inventory.csv status=not_implemented "
            f"(possible stale inventory entry)"
        )
    return CheckResult(
        check_id="C9",
        name="inventory.csv status synced with code (no stale not_implemented)",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# Check 10: RFC 4180 引号转义合规
# ---------------------------------------------------------------------


def check_c10_rfc4180_quoting() -> CheckResult:
    """C10: RFC 4180 引号合规。

    规则：
    - 每行（原始文本）的双引号数量必须为偶数（成对）
    - 含 ``,""`` 之外的双引号且未成对的行视为违规
    - Python csv 模块能正确解析 = 隐式合规（C1 已覆盖列数）；
      本 check 进一步在 raw 文本层面核查每行引号配对
    """
    evidence: List[str] = []
    for path in (INVENTORY_CSV, PERF_SCENARIOS_CSV):
        data = _read_raw_bytes(path)
        # 解码后按通用换行分割
        text = data.decode("utf-8", errors="replace")
        # 不依赖 csv 模块，逐物理行扫描
        for line_no, raw_line in enumerate(text.split("\n"), start=1):
            # 去掉行尾 \r（防止误算）
            line = raw_line.rstrip("\r")
            # 物理行引号计数（CRLF / LF 已分别由 C3 / 文件本身处理）
            quote_count = line.count('"')
            if quote_count % 2 != 0:
                evidence.append(
                    f"{path.name} line {line_no}: odd number of '\"' "
                    f"({quote_count}); RFC 4180 requires balanced quoting"
                )
    return CheckResult(
        check_id="C10",
        name="RFC 4180 quote escaping compliance",
        passed=(len(evidence) == 0),
        evidence=evidence,
    )


# ---------------------------------------------------------------------
# 主入口
# ---------------------------------------------------------------------


def run_all_checks() -> L4Report:
    """顺序执行 C1-C10，汇总为 L4Report。"""
    checks: List[CheckResult] = [
        check_c1_csv_column_counts(),
        check_c2_utf8_no_bom(),
        check_c3_lf_only(),
        check_c4_impl_module_exists(),
        check_c5_inventory_perf_data_source_reachable(),
        check_c6_perf_scenarios_data_source_reachable(),
        check_c7_no_missing_implemented(),
        check_c8_meets_10x_self_consistent(),
        check_c9_status_synced(),
        check_c10_rfc4180_quoting(),
    ]
    passed_count = sum(1 for c in checks if c.passed)
    failed_count = len(checks) - passed_count
    summary: Dict[str, int] = {
        "total": len(checks),
        "passed": passed_count,
        "failed": failed_count,
    }
    return L4Report(
        passed=(failed_count == 0),
        checks=checks,
        summary=summary,
    )


def _format_console(report: L4Report) -> str:
    """生成人工可读的控制台文本（无颜色——PowerShell 端会处理颜色）。"""
    lines: List[str] = []
    lines.append("==== L4 Consistency Gate ====")
    lines.append(
        f"summary: {report.summary['passed']}/{report.summary['total']} passed"
    )
    for c in report.checks:
        verdict = "PASS" if c.passed else "FAIL"
        lines.append(f"  [{c.check_id}] {verdict}  {c.name}")
        if not c.passed:
            for ev in c.evidence:
                lines.append(f"        - {ev}")
    return "\n".join(lines)


def main(argv: Optional[Sequence[str]] = None) -> int:
    """CLI 入口。

    :param argv: 命令行参数（默认为 sys.argv[1:]）。
    :return: 退出码（0=PASS / 1=FAIL）。
    """
    argv = list(sys.argv[1:] if argv is None else argv)
    report_path: Optional[str] = None
    for arg in argv:
        if arg.startswith("--report="):
            report_path = arg.split("=", 1)[1]
        elif arg in ("-h", "--help"):
            sys.stdout.write(
                "Usage: run_l4_consistency.py [--report=PATH]\n"
                "Exit code: 0=PASS / 1=FAIL\n"
            )
            return 0

    # 前置检查：目标文件必须存在
    missing_files: List[str] = []
    for label, path in (
        ("INVENTORY_CSV", INVENTORY_CSV),
        ("PERF_SCENARIOS_CSV", PERF_SCENARIOS_CSV),
        ("NODES_MOD_RS", NODES_MOD_RS),
        ("DESCRIPTORS_PY", DESCRIPTORS_PY),
    ):
        if not path.exists():
            missing_files.append(f"{label}={path}")
    if missing_files:
        sys.stderr.write(
            "L4 Consistency Gate: required files missing:\n  "
            + "\n  ".join(missing_files)
            + "\n"
        )
        return 1

    report = run_all_checks()
    sys.stdout.write(_format_console(report) + "\n")
    sys.stdout.flush()

    if report_path:
        out_path = Path(report_path)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        with out_path.open("w", encoding="utf-8") as f:
            json.dump(report.to_dict(), f, ensure_ascii=False, indent=2)
        sys.stdout.write(f"L4 report written: {out_path}\n")

    return 0 if report.passed else 1


if __name__ == "__main__":
    sys.exit(main())
