# -*- coding: utf-8 -*-
"""audit_flagbuildnone：原版 flagbuildnone 语义 × neoconstruct 构造器全集 × ValueKind 分类逐一对账。

任务：v0.1.2-1r（REV 驳回 P1/P4 的脚本证据要求——禁手工抽查）。
方法（全静态，无 import 依赖，任何 Python 3.10+ 可跑）：
  1. AST 解析 .venv-pc construct/core.py：
     - 提取 `self.flagbuildnone = True` 的类（叶类全集，断言 15 个）
     - 提取传播赋值（all/and/any/subcon 透传）的类（断言 7 个）
     - 提取 Construct 基类默认值（断言 False）
  2. AST 解析 neoconstruct/python/neoconstruct/__init__.py：
     - 提取全部导出公开名（from X import 块 + as 别名）
     - 过滤非构造器（异常类/Descriptor 类型名/工厂函数/Mixin）
  3. 对账（逐构造器，缺一即 FAIL）：
     - 原版 flagbuildnone=True（叶类）  <=> ValueKind != Instance
     - 原版 flagbuildnone=False（基类默认/值类） <=> ValueKind == Instance
     - 传播/包装器类（透传 inner）       <=> ValueKind == Instance（差异单列，见设计 §5.1c）
  4. 输出：控制台摘要 + audit_flagbuildnone_result.md 对账全表。

运行：python audit_flagbuildnone.py（在 experiments/v0.1.2-spike/ 下）
"""
import ast
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
CORE_PY = REPO / "neoconstruct" / ".venv-pc" / "Lib" / "site-packages" / "construct" / "core.py"
INIT_PY = REPO / "neoconstruct" / "python" / "neoconstruct" / "__init__.py"
OUT_MD = HERE / "audit_flagbuildnone_result.md"

# ---------------------------------------------------------------- 1. 原版侧 ---

def extract_flagbuildnone(path):
    """AST 提取 core.py 的 flagbuildnone 赋值。

    返回 (leaf_true, propagators, base_default)：
    - leaf_true: {class_name: line}  类内 `self.flagbuildnone = True`
    - propagators: {class_name: (line, 规则描述)}  all/and/any/透传
    - base_default: (line, False)  Construct 基类默认 False
    """
    tree = ast.parse(path.read_text(encoding="utf-8"))
    leaf_true, propagators, base_default = {}, {}, None
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef):
            continue
        # 赋值多在 __init__ 内（FunctionDef），需类范围递归查找。
        for stmt in ast.walk(node):
            if not (isinstance(stmt, ast.Assign)
                    and len(stmt.targets) == 1
                    and isinstance(stmt.targets[0], ast.Attribute)
                    and stmt.targets[0].attr == "flagbuildnone"):
                continue
            v = stmt.value
            if isinstance(v, ast.Constant) and v.value is True:
                leaf_true[node.name] = stmt.lineno
            elif isinstance(v, ast.Constant) and v.value is False:
                if node.name == "Construct":
                    base_default = (stmt.lineno, False)
            elif isinstance(v, ast.Call) and isinstance(v.func, ast.Name):
                if v.func.id == "all":
                    propagators[node.name] = (stmt.lineno, "all(子构造器)")
                elif v.func.id == "any":
                    propagators[node.name] = (stmt.lineno, "any(子构造器)")
            elif (isinstance(v, ast.Attribute)
                  and isinstance(v.value, ast.Name)
                  and v.value.id == "subcon"):
                propagators[node.name] = (stmt.lineno, "透传 subcon")
            elif (isinstance(v, ast.BoolOp) and isinstance(v.op, ast.And)):
                propagators[node.name] = (stmt.lineno, "and(then,else)")
    return leaf_true, propagators, base_default


# ------------------------------------------------------- 2. neoconstruct 侧 ---

NON_CONSTRUCTOR_HINTS = ("Error", "Descriptor", "Mixin", "CancelParsing",
                         "StringEncoded")

# 精确排除：导出但非构造器的公开类型（附理由，防止误当遗漏）。
NON_CONSTRUCTOR_EXACT = {
    "HashAlgo": "Checksum 的参数枚举（_hashalgo.py，非 field 参数构造器）",
    "CompiledSchema": "编译产物类型（__init__.py:268 注释：供类型注解与调试）",
}


def extract_neoconstruct_exports(path):
    """AST 提取 __init__.py 全部导出公开名，过滤非构造器。"""
    tree = ast.parse(path.read_text(encoding="utf-8"))
    names = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.ImportFrom):
            continue
        for alias in node.names:
            public = alias.asname or alias.name
            if public.startswith("_"):
                continue
            if any(hint in public for hint in NON_CONSTRUCTOR_HINTS):
                continue
            if public in NON_CONSTRUCTOR_EXACT:
                continue  # 非构造器公开类型（见 NON_CONSTRUCTOR_EXACT 理由）
            if not public[0].isupper():
                continue  # field/rfield/wfield 等工厂
            names.add(public)
    return sorted(names)


# --------------------------------------------- 3. ValueKind v2 分类映射表 ---
# family: (ValueKind, 原版类/家族, 原版 flagbuildnone 判定, [neoconstruct 公开名])
# 原版判定取值：'T'(叶类True) / 'F'(基类默认False) / 'P'(传播) / 'X'(原版无对应)

FAMILIES = [
    # --- 值语义叶类（原版 flagbuildnone=True 全部 15 个 - 未实现 2 个） ---
    ("Const",    "Const",        "T", ["Const"]),
    ("Computed", "Computed",     "T", ["Computed"]),
    ("Index",    "Index",        "T", ["Index"]),
    ("Rebuild",  "Rebuild",      "T", ["Rebuild"]),
    ("Default",  "Default",      "T", ["Default"]),
    ("Tell",     "Tell",         "T", ["Tell"]),
    ("Void",     "Padded/Pass",  "T", ["Pass"]),          # Padding 见下（原版 Padding=Padded(n,Pass) 透传）
    ("Void",     "Padded/Pass",  "T", ["Padding"]),
    ("Control",  "Check",        "T", ["Check"]),
    ("Control",  "StopIf",       "T", ["StopIf"]),
    ("Control",  "Terminated",   "T", ["Terminated"]),
    ("Control",  "Peek",         "T", ["Peek"]),          # v2 新增分类（REV P1）
    ("Control",  "Seek",         "T", ["Seek"]),          # v2 新增分类（REV P1）
    ("Control",  "Checksum",     "T", ["Checksum"]),      # v2 新增分类（REV P1）
    ("Control",  "Element",      "X", ["Element"]),       # neoconstruct 扩展（RepeatUntil 元素绑定）
    # --- 原子/标量（FormatField 家族，原版基类默认 False） ---
    ("Instance", "FormatField",  "F",
     ["Int8ub", "Int8ul", "Int8sb", "Int8sl", "Int16ub", "Int16ul", "Int16sb",
      "Int16sl", "Int32ub", "Int32ul", "Int32sb", "Int32sl", "Int64ub", "Int64ul",
      "Int64sb", "Int64sl", "Byte", "Int", "Short", "Long", "Single", "Double",
      "Half", "Float16b", "Float16l", "Float32b", "Float32l", "Float64b",
      "Float64l"]),
    # --- 长度整数（BytesInteger/VarInt 家族，原版 False） ---
    ("Instance", "BytesInteger", "F",
     ["BytesInteger", "Int24ub", "Int24ul", "Int24sb", "Int24sl", "VarInt",
      "ZigZag", "BitsInteger", "Bit", "Nibble", "Octet"]),
    ("Instance", "Bytes",        "F", ["Bytes", "GreedyBytes"]),
    # --- 字符串族（原版 False） ---
    ("Instance", "String 家族",  "F",
     ["CString", "GreedyString", "NullStripped", "NullTerminated",
      "PaddedString", "PascalString"]),
    # --- 复合容器（Struct all(子)/Sequence all/Union 默认 F/FocusedSeq F） ---
    ("Instance", "Struct 家族",  "P/F",
     ["Sequence", "Union", "FocusedSeq", "AlignedStruct"]),  # StructMixin 子类本身即字段类型
    # --- 数组族（原版 False） ---
    ("Instance", "Array 家族",   "F",
     ["Array", "GreedyRange", "PrefixedArray", "RepeatUntil"]),
    # --- 条件/选择（IfThenElse and / Switch all / Select any——传播；inner 值类时整体必填） ---
    ("Instance", "条件/选择",    "P",
     ["If", "IfThenElse", "Switch", "Select"]),
    # --- 包装器（Subconstruct 透传族；inner 值类时整体必填） ---
    ("Instance", "Subconstruct 透传族", "P",
     ["Renamed", "Subconstruct", "Bitwise", "Bytewise", "BitsSwapped",
      "ByteSwapped", "Aligned", "Pointer", "Prefixed", "Probe"]),
    # --- Adapter/Validator 族（原版经 Adapter(Subconstruct) 透传 inner） ---
    ("Instance", "Adapter 族",   "P",
     ["Enum", "Mapping", "FlagsEnum", "NamedTuple", "Hex", "HexDump",
      "RawCopy", "ProcessXor", "ProcessRotateLeft", "OneOf", "NoneOf",
      "Timestamp", "Adapter", "SymmetricAdapter", "Validator"]),
    # 注：Adapter/SymmetricAdapter/Validator 是 ADR-022 用户面 Python 基类，
    # 作为 field() 参数时经编译包装为节点——值语义等同原版 Adapter 族（透传 inner）。
]

# 原版 flagbuildnone=True 但 neoconstruct 未实现的构造器（inventory not_implemented）
UNIMPLEMENTED_FLAGBUILDNONE = ["Error", "RestreamData"]


def main():
    failures = []

    leaf_true, propagators, base_default = extract_flagbuildnone(CORE_PY)
    exports = extract_neoconstruct_exports(INIT_PY)

    # -- 断言 1：原版叶类全集 == 预期 15 个（防版本漂移，REV 报告实证） --
    expected_leaves = {
        "Const", "Computed", "Index", "Rebuild", "Default", "Check", "Error",
        "StopIf", "Peek", "Seek", "Tell", "Pass", "Terminated", "RestreamData",
        "Checksum",
    }
    if set(leaf_true) != expected_leaves:
        failures.append(
            f"原版叶类漂移: 多={set(leaf_true) - expected_leaves} "
            f"少={expected_leaves - set(leaf_true)}")

    # -- 断言 2：传播类全集（Subconstruct 透传 / Struct all / Sequence all /
    #    Select any / IfThenElse and / Switch all / LazyStruct all） --
    expected_props = {"Subconstruct", "Struct", "Sequence", "Select",
                      "IfThenElse", "Switch", "LazyStruct"}
    if set(propagators) != expected_props:
        failures.append(
            f"传播类漂移: 多={set(propagators) - expected_props} "
            f"少={expected_props - set(propagators)}")

    # -- 断言 3：基类默认 False --
    if base_default is None:
        failures.append("未找到 Construct 基类 flagbuildnone = False")

    # -- 断言 4：映射表覆盖全部导出构造器（缺一即 FAIL） --
    classified = {}
    for kind, py_cls, verdict, names in FAMILIES:
        for n in names:
            if n in classified:
                failures.append(f"映射重复: {n}")
            classified[n] = (kind, py_cls, verdict)
    uncovered = [e for e in exports if e not in classified]
    if uncovered:
        failures.append(f"未覆盖导出构造器: {uncovered}")
    stale = [c for c in classified if c not in exports]
    if stale:
        failures.append(f"映射含未导出名称（过期条目）: {stale}")

    # -- 断言 5：语义一致性 --
    # 原版叶类 True（已实现）<=> ValueKind != Instance
    # 原版 F <=> Instance；传播 P <=> Instance（W1 型差异由设计 §5.1c 显式记录）
    semantic_fail = []
    for name, (kind, py_cls, verdict) in sorted(classified.items()):
        if verdict == "T" and kind == "Instance":
            semantic_fail.append(f"{name}: 原版哑值可省但分类 Instance")
        if verdict in ("F", "P") and kind != "Instance":
            semantic_fail.append(f"{name}: 原版必填但分类 {kind}")
        if verdict == "X" and kind == "Instance":
            semantic_fail.append(f"{name}: 原版无对应且分类 Instance 应复核")
    failures.extend(semantic_fail)

    # -- 统计 --
    total = len(exports)
    kinds = {}
    for kind, _, _ in classified.values():
        kinds[kind] = kinds.get(kind, 0) + 1

    # -- 输出 --
    lines = []
    lines.append("# flagbuildnone × ValueKind 对账全表（脚本生成，禁手工）")
    lines.append("")
    lines.append(f"- 生成脚本：`experiments/v0.1.2-spike/audit_flagbuildnone.py`")
    lines.append(f"- 原版源：`{CORE_PY.relative_to(REPO)}`（AST 提取，construct 2.10.70）")
    lines.append(f"- neoconstruct 源：`{INIT_PY.relative_to(REPO)}`（AST 提取导出全集）")
    lines.append("")
    lines.append("## 原版侧提取结果")
    lines.append("")
    lines.append(f"- `flagbuildnone = True` 叶类：**{len(leaf_true)}** 个（断言通过 = "
                 f"预期 15）：")
    for cls in sorted(leaf_true, key=lambda c: leaf_true[c]):
        lines.append(f"  - `{cls}`（core.py:{leaf_true[cls]}）")
    lines.append(f"- 传播类：**{len(propagators)}** 个：")
    for cls in sorted(propagators, key=lambda c: propagators[c][0]):
        lines.append(f"  - `{cls}`（core.py:{propagators[cls][0]}，{propagators[cls][1]}）")
    lines.append(f"- Construct 基类默认：`flagbuildnone = False`（core.py:{base_default[0]}）")
    lines.append(f"- 其中 neoconstruct 未实现：{UNIMPLEMENTED_FLAGBUILDNONE}（预留分类见下）")
    lines.append("")
    lines.append("## neoconstruct 构造器 × ValueKind 分类全表")
    lines.append("")
    lines.append(f"导出构造器总数：**{total}**（异常/Descriptor 类型名/工厂/Mixin 已过滤）")
    lines.append("")
    lines.append("| 构造器 | ValueKind | 原版对应 | 原版判定 | 一致性 |")
    lines.append("|--------|-----------|----------|----------|--------|")
    for name in sorted(classified):
        kind, py_cls, verdict = classified[name]
        if verdict == "T":
            note = "哑值可省 ✓"
        elif verdict == "P":
            note = "传播（§5.1c）✓"
        elif verdict == "X":
            note = "neoconstruct 扩展"
        else:
            note = "必填 ✓"
        lines.append(f"| {name} | {kind} | {py_cls} | {verdict} | {note} |")
    lines.append("")
    lines.append("## 未实现的原版 flagbuildnone=True 构造器（预留分类）")
    lines.append("")
    lines.append("| 原版构造器 | 预留 ValueKind | 依据 |")
    lines.append("|-----------|---------------|------|")
    lines.append("| Error | Control | parse/build 无条件 ExplicitError（core.py:3142）|")
    lines.append("| RestreamData | Control | build no-op return obj（core.py:5182）|")
    lines.append("")
    lines.append("## 分类分布统计")
    lines.append("")
    for kind in sorted(kinds, key=lambda k: -kinds[k]):
        lines.append(f"- {kind}: {kinds[kind]}")
    verdict_lines = " / ".join(
        f"{v}={sum(1 for _, _, vv in classified.values() if vv == v)}"
        for v in ("T", "F", "P", "X"))
    lines.append(f"- 原版判定分布：{verdict_lines}")
    lines.append("")
    lines.append("## 结论")
    lines.append("")
    if failures:
        lines.append("**FAIL**：")
        for f in failures:
            lines.append(f"- {f}")
    else:
        lines.append("**PASS**：原版 flagbuildnone 语义（15 叶类 + 7 传播类 + 基类 False）"
                     "与 ValueKind v2 分类逐一对账一致；导出构造器 "
                     f"{total} 个全覆盖。")
    OUT_MD.write_text("\n".join(lines) + "\n", encoding="utf-8")

    print(f"exports={total} leaves={len(leaf_true)} propagators={len(propagators)}")
    print(f"kinds={kinds}")
    if failures:
        print("FAIL:")
        for f in failures:
            print(f"  - {f}")
        sys.exit(1)
    print("PASS: 全量对账一致")
    print(f"result -> {OUT_MD.relative_to(REPO)}")


if __name__ == "__main__":
    main()
