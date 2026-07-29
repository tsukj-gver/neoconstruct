"""5.6b CSV 更新脚本：基于 5.6a 同会话 Controlled A/B Test 数据更新 perf-scenarios.csv 6 行。

更新行（0-based line index in file, 1-based line number）：
  - 行 2 (idx 1): FormatField B1 parse ModbusRTU
  - 行 4 (idx 3): FormatField B2 parse Int8ub×10
  - 行 6 (idx 5): FormatField B3 parse Int8ub×50
  - 行 8 (idx 7): FormatField B4 parse Int8ub×100
  - 行 146 (idx 145): StopIf S01-4.6 parse
  - 行 150 (idx 149): StopIf S03-4.6 parse

数据源：plans/phase5-struct-ffi/traces/5.6a-基线重建.md §四 汇总表

文件完整性约束（CI L4 check C1/C2/C3/C8/C10）：
  - 17 列 / UTF-8 无 BOM / LF only / speedup_x 与 meets_10x 自洽 / RFC 4180 引号配对
"""

from __future__ import annotations

import sys
import io
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[2]
CSV_PATH = PROJECT_ROOT / "docs" / "perf-scenarios.csv"
ADDED_SOURCE = "plans/phase5-struct-ffi/traces/5.6a-基线重建.md"

# 行号 -> (列名 -> 新值)
# 列顺序：constructor,scenario_id,scenario_desc,direction,element_type,scale,
#         byte_order,branch,path_type,nesting,py_ns_per_call,rs_ns_per_call,
#         speedup_x,meets_10x,data_source,phase,notes
UPDATES = {
    2: {
        "py_ns_per_call": "2923",
        "rs_ns_per_call": "251",
        "speedup_x": "11.64",
        "meets_10x": "true",
        "notes": (
            "P0 优化后 DEV 报告; 00-项目进度.md 给 7.63x (PM 独立复验口径); "
            "5.6a 同会话 Controlled A/B Test 复测 (3轮 mean±stddev 11.64±0.07; "
            "py 3123→2923 / rs 386→251; B 端真实改进 -135ns + A 端环境漂移 -200ns 共同拉高)"
        ),
    },
    4: {
        "py_ns_per_call": "5688",
        "rs_ns_per_call": "416",
        "speedup_x": "13.68",
        "meets_10x": "true",
        "notes": (
            "00-项目进度.md 给 8.83x (不同测量时点); "
            "5.6a 同会话 Controlled A/B Test 复测 (3轮 mean±stddev 13.68±0.30; "
            "py 6068→5688 / rs 628→416; B 端每字段成本下降 + A 端环境漂移共同拉高)"
        ),
    },
    6: {
        "py_ns_per_call": "21220",
        "rs_ns_per_call": "1524",
        "speedup_x": "13.93",
        "meets_10x": "true",
        "notes": (
            "00-项目进度.md 给 13.66x (不同测量时点); "
            "5.6a 同会话 Controlled A/B Test 复测 (3轮 mean±stddev 13.93±0.04 极稳; "
            "py 23314→21220 / rs 2020→1524)"
        ),
    },
    8: {
        "py_ns_per_call": "40103",
        "rs_ns_per_call": "3016",
        "speedup_x": "13.30",
        "meets_10x": "true",
        "notes": (
            "00-项目进度.md 给 13.44x (PM 独立复验 B4=11.77x); "
            "5.6a 同会话 Controlled A/B Test 复测 (3轮 mean±stddev 13.30±0.32; "
            "py 44097→40103 / rs 3656→3016; stddev 在 L-09 噪声阈值边缘但 mean 远超 10x)"
        ),
    },
    146: {
        "py_ns_per_call": "3099",
        "rs_ns_per_call": "252",
        "speedup_x": "12.29",
        "meets_10x": "true",
        "notes": (
            "4.6 O1 (try_eval_simple_cmp fast-path); VET 独立复测; "
            "O1 前 S01 4.4v1 9.65x → O1 后 ≥10x 达标; "
            "5.6a 同会话 Controlled A/B Test 复测 (3轮 mean±stddev 12.29±0.12; "
            "首次补全 py=3099 / rs=252 绝对值; 高于 4.6 VET 复测 10.72x 归因环境漂移)"
        ),
    },
    150: {
        "py_ns_per_call": "2913",
        "rs_ns_per_call": "237",
        "speedup_x": "12.32",
        "meets_10x": "true",
        "notes": (
            "4.6 O1 VET 复测; Always 路径不应命中 O1 但 crate 整体 inlining 收益 (设计 §16.1.6 反例3); "
            "5.6a 同会话 Controlled A/B Test 复测 (3轮 mean±stddev 12.32±0.05 极稳; "
            "首次补全 py=2913 / rs=237 绝对值; 高于 4.6 VET 复测 10.93x 归因环境漂移)"
        ),
    },
}


def parse_csv_row(line: str):
    """简易 RFC 4180 解析（本 CSV 仅 notes 列可能含逗号，且都被引号包围）。

    返回字段列表。
    """
    import csv
    return next(csv.reader([line]))


# 历史风格：scenario_desc (idx 2) 和 notes (idx 16) 总是加引号（即使不含逗号）
FORCE_QUOTE_INDICES = {2, 16}


def _field_needs_quote(field: str) -> bool:
    """RFC 4180 字段是否需要加引号（含逗号/引号/换行）。"""
    return any(c in field for c in (",", '"', "\n", "\r"))


def _encode_field(field: str, force: bool = False) -> str:
    """编码单个字段为 CSV token；force=True 总是加引号。"""
    if force or _field_needs_quote(field):
        return '"' + field.replace('"', '""') + '"'
    return field


def encode_csv_row(fields) -> str:
    """手工拼接单行：scenario_desc / notes 强制加引号（与历史风格一致），
    其他字段按 RFC 4180 规则决定。
    """
    return ",".join(
        _encode_field(fv, force=(ci in FORCE_QUOTE_INDICES))
        for ci, fv in enumerate(fields)
    )


def main() -> int:
    if not CSV_PATH.exists():
        sys.stderr.write(f"CSV not found: {CSV_PATH}\n")
        return 1

    raw = CSV_PATH.read_bytes()
    # 检测 BOM
    if raw.startswith(b"\xef\xbb\xbf"):
        sys.stderr.write("ERROR: file has UTF-8 BOM, aborting\n")
        return 2
    # 检测 CRLF
    if b"\r\n" in raw:
        sys.stderr.write("ERROR: file contains CRLF, aborting\n")
        return 2

    text = raw.decode("utf-8")
    # 按 \n 分隔，保留行尾空字符串（如文件末尾 \n）
    # 注意：splitlines 会消耗尾行信息；这里用 split("\n")
    lines = text.split("\n")

    # 期望行数：155 行数据 + 1 表头 + 末尾空串（如果文件以 \n 结尾）= 156 or 157
    print(f"[info] total split parts: {len(lines)} (expect 156 or 157 with trailing empty)")
    print(f"[info] header: {lines[0][:80]}...")

    # 表头列名
    header_fields = parse_csv_row(lines[0])
    print(f"[info] header column count: {len(header_fields)} (expect 17)")
    if len(header_fields) != 17:
        sys.stderr.write(f"ERROR: header column count != 17: {len(header_fields)}\n")
        return 3

    col_index = {name: i for i, name in enumerate(header_fields)}

    updated_count = 0
    for line_no, updates in UPDATES.items():
        idx = line_no - 1
        if idx >= len(lines):
            sys.stderr.write(f"ERROR: line {line_no} out of range (total {len(lines)})\n")
            return 4
        original_line = lines[idx]
        fields = parse_csv_row(original_line)
        if len(fields) != 17:
            sys.stderr.write(
                f"ERROR: line {line_no} has {len(fields)} columns (expect 17)\n"
            )
            return 5

        before_snapshot = list(fields)

        # 应用更新
        for col_name, new_val in updates.items():
            ci = col_index[col_name]
            fields[ci] = new_val

        # data_source 列追加（若不存在则加）
        ds_idx = col_index["data_source"]
        existing_ds = fields[ds_idx]
        if ADDED_SOURCE not in existing_ds:
            fields[ds_idx] = existing_ds + ";" + ADDED_SOURCE
        else:
            print(f"[warn] line {line_no}: data_source already contains 5.6a source, skip append")

        # 编码回单行
        new_line = encode_csv_row(fields)
        lines[idx] = new_line
        updated_count += 1

        # 打印 diff 摘要
        print(f"\n[update] line {line_no}: {before_snapshot[0]}/{before_snapshot[1]}/{before_snapshot[3]}")
        for col_name in ("py_ns_per_call", "rs_ns_per_call", "speedup_x", "meets_10x"):
            ci = col_index[col_name]
            print(f"    {col_name}: {before_snapshot[ci]!r} -> {fields[ci]!r}")
        print(f"    data_source: ...{existing_ds[-60:]} -> ...{fields[ds_idx][-80:]}")

    # 重新拼回（保持 LF）
    new_text = "\n".join(lines)
    # 写回：UTF-8 无 BOM，不转换换行
    # Python open() 默认 newline 会转换 \n -> os.linesep，需指定 newline=""
    # 然后我们已经手动用 \n，所以保持原样
    with CSV_PATH.open("w", encoding="utf-8", newline="") as f:
        f.write(new_text)

    print(f"\n[done] updated {updated_count} rows in {CSV_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
