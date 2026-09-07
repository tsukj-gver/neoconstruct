"""探针 6：文档-代码契约（SKILL 附录 import 清单完整性 + README 示例逐字运行）。

用户视角：用户从 README/SKILL 复制代码起步。文档承诺的每个符号都应
可 import 且行为如文档所述；README 首屏示例必须逐字可跑。现有套件的
test_example.py 测的是一个 Modbus 例子（非 README 首屏示例），也没有
"文档承诺符号全部可导入"这类文档契约测试。

运行：.venv/Scripts/python.exe probe_doc_contract.py
"""

from __future__ import annotations

import sys

sys.path.insert(0, r"D:\Project\Github\neoconstruct\neoconstruct\python")

import neoconstruct

# --- 6.1 SKILL 附录「完整 import 速查」逐符号导入 ---
SKILL_SYMBOLS = [
    # 基类
    "StructMixin", "BitStructMixin",
    # 字段声明
    "field", "rfield", "wfield",
    # 整数（最常用）
    "Int8ub", "Int8ul", "Int16ub", "Int16ul", "Int32ub", "Int32ul", "Int64ub", "Int64ul",
    "Byte", "Short", "Int", "Long",
    # 字节
    "Bytes", "GreedyBytes", "BytesInteger",
    # 变长整数
    "VarInt", "ZigZag",
    # 字符串
    "CString", "GreedyString", "PaddedString", "PascalString",
    # bit 域
    "Bit", "Nibble", "Octet", "BitsInteger", "Bitwise", "Padding",
    # 数组
    "Array", "GreedyRange", "PrefixedArray",
    # Adapter
    "Const", "Default", "Check", "Computed", "Peek", "RawCopy", "Rebuild",
    "Enum", "FlagsEnum", "Mapping", "OneOf", "NoneOf", "Union",
    "Hex", "HexDump", "Subconstruct", "Tell", "Terminated", "Probe", "Pass",
    # 条件
    "If", "IfThenElse", "Switch", "Select", "FocusedSeq", "StopIf",
    # 流
    "Seek", "Pointer", "Prefixed",
    # 对齐
    "Aligned", "AlignedStruct",
    # 校验
    "Checksum", "HashAlgo",
    # 结构
    "Sequence", "NamedTuple",
    # 其他
    "Timestamp", "ProcessXor", "ProcessRotateLeft", "Adapter",
    # 异常
    "ConstructError", "ChecksumError", "ConstError", "CheckError",
    "ValidationError", "MappingError", "SelectError", "TerminatedError",
]

missing = [s for s in SKILL_SYMBOLS if not hasattr(neoconstruct, s)]
print(f"SKILL 附录符号: {len(SKILL_SYMBOLS) - len(missing)}/{len(SKILL_SYMBOLS)} 可导入")
if missing:
    print("MISSING:", missing)

# SKILL 3.3 章表格里出现但附录没列的
EXTRA_TABLE_SYMBOLS = [
    "Int8sb", "Int24ub", "Float16b", "Float32l", "Half", "Single", "Double",
    "NullTerminated", "NullStripped", "Bytewise", "BitsSwapped", "ByteSwapped",
    "Renamed", "Index", "Element", "BitStructMixin", "FieldLengthError",
    "FormatFieldError", "FieldValueMissingError", "CompilationError", "StreamError",
]
missing2 = [s for s in EXTRA_TABLE_SYMBOLS if not hasattr(neoconstruct, s)]
print(f"SKILL 表格补充符号: {len(EXTRA_TABLE_SYMBOLS) - len(missing2)}/{len(EXTRA_TABLE_SYMBOLS)} 可导入")
if missing2:
    print("MISSING:", missing2)

# --- 6.2 README 首屏示例逐字执行 ---
from dataclasses import dataclass  # noqa: E402
from neoconstruct import StructMixin, field, Int16ub, Int8ub, Bytes  # noqa: E402


@dataclass
class Header(StructMixin):
    magic: int = field(Int16ub)
    version: int = field(Int8ub)
    payload: bytes = field(Bytes(4))


h = Header(magic=0xCAFE, version=1, payload=b"ABCD")
assert h.build() == b"\xCA\xFE\x01ABCD", h.build()
assert Header.parse(b"\xCA\xFE\x01ABCD") == h
print("README 首屏示例: OK（逐字运行通过）")
