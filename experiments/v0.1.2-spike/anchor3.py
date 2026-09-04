# -*- coding: utf-8 -*-
"""anchor3：P1 分类缺口（Seek/Peek/Checksum/包装器传播）的原版锚点实测。

任务：v0.1.2-1r [字段值语义框架设计修订]（REV 驳回 P1/P4 补证据）。
环境：.venv-pc（construct 2.10.70），设计文档 §2 锚点表的补行来源。
运行：& neoconstruct/.venv-pc/Scripts/python.exe experiments/v0.1.2-spike/anchor3.py

实测目标（每条输出 OK/ERR + 实际值，供设计 §2 表与 §5.1 分类决策引用）：
  A1  Peek 缺省 build（dict 不含 peek 键）→ no-op 成功
  A2  Peek 显式值 → 被忽略（no-op）
  A3  Peek 缺省 + 后继 Computed 引用 → None 流入表达式报错（哑值=None 实证）
  A4  Peek parse → 存有效值（Union 场景外的字段语境）
  S1  Seek 缺省 build → 移动流指针成功（whence=1 相对）
  S2  Seek parse → 字段值 = seek 后新位置
  C1  Checksum 缺省 build → 重算 hash 写入成功
  C2  Checksum 显式传错误值 → 仍被忽略，写正确 digest
  C3  Checksum parse 校验失败 → ChecksumError
  W1  If(cond, Padding) 缺省 build → 成功（flagbuildnone 传播 all→True 实证）
  W2  If(cond, Byte) 缺省 build → 报错（None 流入 Byte，隐式必填）
  W3  Renamed 透传：'x' / Padding 缺省 build → 成功
  N1  长度字节>剩余（顶层 Bytes(this.n)）→ StreamError
  N2  Prefixed 长度声明不符 → StreamError
  N3  嵌套 Prefixed 内层需求不符 → StreamError（B2 数据负例回归锁定锚点）
"""
import io
import sys

import construct as pc

RESULTS = []


def record(name, fn):
    try:
        value = fn()
        RESULTS.append((name, "OK", repr(value)))
        print(f"{name}: OK  {value!r}")
    except Exception as e:
        RESULTS.append((name, "ERR", f"{type(e).__name__}: {e}"))
        print(f"{name}: ERR {type(e).__name__}: {e}")


# --- A 族：Peek（core.py:4481 flagbuildnone=True；_build(obj) return obj）---

def a1():
    d = pc.Struct("p" / pc.Peek(pc.Int8ub), "b" / pc.Int8ub)
    return d.build(dict(b=1))  # 缺 p：哑值 None → no-op

record("A1 Peek 缺省 build", a1)


def a2():
    d = pc.Struct("p" / pc.Peek(pc.Int8ub), "b" / pc.Int8ub)
    return d.build(dict(p=99, b=1))  # 显式 p：仍 no-op（忽略）

record("A2 Peek 显式值忽略", a2)


def a3():
    d = pc.Struct("p" / pc.Peek(pc.Int8ub), "c" / pc.Computed(pc.this.p + 1))
    return d.build(dict())  # 缺 p：哑值 None 流入表达式

record("A3 Peek 缺省+后继引用", a3)


def a4():
    d = pc.Struct("p" / pc.Peek(pc.Int8ub), "b" / pc.Int8ub)
    return d.parse(b"\x05\x07")  # parse：p 读到 5（流回退后 b 再读 5）

record("A4 Peek parse", a4)


# --- S 族：Seek（core.py:4595 flagbuildnone=True；_build 忽略 obj，返回新位置）---

def s1():
    d = pc.Struct("data" / pc.Bytes(2), "s" / pc.Seek(1, 1), "b" / pc.Int8ub)
    return d.build(dict(data=b"ab", b=9))  # 缺 s：哑值 None → seek(+1) 跳 1 字节

record("S1 Seek 缺省 build", s1)


def s2():
    d = pc.Struct("a" / pc.Int8ub, "s" / pc.Seek(3), "b" / pc.Int8ub)
    return d.parse(b"\x01\x02\x03\x04")  # parse：s 的值 = 新位置 3

record("S2 Seek parse 值=新位置", s2)


# --- C 族：Checksum（core.py:5553 flagbuildnone=True；_build 忽略 obj 重算）---

def _crc16(data):
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return bytes([crc & 0xFF, (crc >> 8) & 0xFF])


def _checksum_struct():
    return pc.Struct(
        "body" / pc.RawCopy(pc.Struct("a" / pc.Int8ub, "b" / pc.Int8ub)),
        "crc" / pc.Checksum(pc.Bytes(2), _crc16, pc.this.body.data),
    )


def c1():
    d = _checksum_struct()
    return d.build(dict(body=dict(value=dict(a=1, b=2))))  # 缺 crc：重算写入

record("C1 Checksum 缺省 build", c1)


def c2():
    d = _checksum_struct()
    # 显式传错误 crc：仍应写正确 digest（忽略 obj）
    return d.build(dict(body=dict(value=dict(a=1, b=2)), crc=b"\x00\x00"))

record("C2 Checksum 显式错误值忽略", c2)


def c3():
    d = _checksum_struct()
    good = d.build(dict(body=dict(value=dict(a=1, b=2))))
    bad = good[:-2] + b"\xde\xad"  # 篡改 crc 字节
    return d.parse(bad)  # → ChecksumError

record("C3 Checksum parse 校验失败", c3)


# --- W 族：包装器传播（If=IfThenElse(cond,then,Pass) and；Renamed/Subconstruct 透传）---

def w1():
    d = pc.Struct("flag" / pc.Int8ub, "pad" / pc.If(pc.this.flag > 0, pc.Padding(2)))
    return d.build(dict(flag=1))  # 缺 pad：If 传播(Padding∧Pass=True) → 哑值 None 可省

record("W1 If(cond,Padding) 缺省", w1)


def w2():
    d = pc.Struct("flag" / pc.Int8ub, "b" / pc.If(pc.this.flag > 0, pc.Int8ub))
    return d.build(dict(flag=1))  # 缺 b：If 传播(Byte∧Pass=False) → None 流入 Byte

record("W2 If(cond,Byte) 缺省", w2)


def w3():
    d = pc.Struct("x" / pc.Padding(2), "b" / pc.Int8ub)  # Renamed 透传 Padding=True
    return d.build(dict(b=7))  # 缺 x

record("W3 Renamed(Padding) 透传", w3)


# --- N 族：B2 数据负例（长度字节声明与 payload 不符 → 显式 StreamError）---

def n1():
    d = pc.Struct("n" / pc.Int8ub, "data" / pc.Bytes(pc.this.n))
    return d.parse(b"\x05abc")  # 声明 5，实际仅 3 字节 → StreamError

record("N1 长度字节>剩余（顶层）", n1)


def n2():
    d = pc.Prefixed(pc.Int8ub, pc.GreedyBytes)
    return d.parse(b"\x05abc")  # Prefixed 声明 5，内层实际 3 → StreamError

record("N2 Prefixed 长度不符", n2)


def n3():
    inner = pc.Struct("x" / pc.Int8ub, "y" / pc.Bytes(pc.this.x + 1))
    d = pc.Prefixed(pc.Int8ub, inner)
    # 外层声明 4；内层 x=1 需 1+1+1=3 字节 → 只剩 2 → StreamError
    return d.parse(b"\x04\x01a")

record("N3 嵌套 Prefixed 内层需求不符", n3)


# --- 汇总 ---
ok = sum(1 for _, s, _ in RESULTS if s == "OK")
err = len(RESULTS) - ok
print(f"\n=== anchor3 汇总: {len(RESULTS)} 条（OK={ok}, ERR={err}）===")
print("ERR 条目即原版『报错形态锚点』（A3/W2 为预期报错，非失败）。")
sys.exit(0)
