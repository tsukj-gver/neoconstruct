"""v0.1.2-1 锚点实测：Const/Default/Rebuild/Computed/Padding × parse/build/缺省。

运行环境：construct 2.10.70（.venv-pc）。所有期望值以本脚本输出为准（禁止凭记忆）。

维度：
  - 值语义构造器：Const / Default / Rebuild / Computed / Padding / Tell
  - 时机：parse 结果（Container 内字段值） / build 缺省（Container 无该键） / build 显式给值
  - 附加：表达式 length 字段（Bytes(this.x+1)）在顶层/嵌套（Prefixed）的 parse/build 行为
  - 附加：Container 缺键时 Struct 传给 subcon 的 obj 是什么（None 语义）
"""
from construct import (
    Struct, Const, Default, Rebuild, Computed, Padding, Tell, Pass,
    Int8ub, Byte, Bytes, Prefixed, GreedyBytes, this, Container,
)

CASES = []


def case(name, fn):
    CASES.append((name, fn))
    return fn


# ---------------------------------------------------------------- Const
case("C1 Const parse: 值入 Container", lambda: (
    Struct("c" / Const(b"Z", Bytes(1))).parse(b"Z")))
case("C2 Const parse mismatch -> ConstError", lambda: (
    Struct("c" / Const(b"Z", Bytes(1))).parse(b"X")))
case("C3 Const build 缺省键 -> 用 value", lambda: (
    Struct("c" / Const(b"Z", Bytes(1))).build(Container())))
case("C4 Const build 空dict", lambda: (
    Struct("c" / Const(5, Byte)).build({})))
case("C5 Const build 显式等值", lambda: (
    Struct("c" / Const(5, Byte)).build({"c": 5})))
case("C6 Const build 显式不等值 -> ConstError", lambda: (
    Struct("c" / Const(5, Byte)).build({"c": 6})))
case("C7 Const build 显式 None（键存在值 None）", lambda: (
    Struct("c" / Const(5, Byte)).build({"c": None})))

# ---------------------------------------------------------------- Default
case("D1 Default parse: 转发 inner", lambda: (
    Struct("d" / Default(Byte, 0)).parse(b"\x07")))
case("D2 Default build 缺省键 -> 默认值", lambda: (
    Struct("d" / Default(Byte, 0)).build({})))
case("D3 Default build 显式值 -> 用显式值", lambda: (
    Struct("d" / Default(Byte, 0)).build({"d": 9})))
case("D4 Default build 键存在但值 None -> 用默认值", lambda: (
    Struct("d" / Default(Byte, 0)).build({"d": None})))
case("D5 Default(this.x+1) build 缺省 -> 表达式求值", lambda: (
    Struct("x" / Byte, "d" / Default(Byte, this.x + 1)).build({"x": 4})))
case("D6 Default(this.x+1) build 显式值 -> 用显式值", lambda: (
    Struct("x" / Byte, "d" / Default(Byte, this.x + 1)).build({"x": 4, "d": 9})))

# ---------------------------------------------------------------- Rebuild
case("R1 Rebuild parse: 转发 inner", lambda: (
    Struct("n" / Byte, "r" / Rebuild(Byte, this.n * 2)).parse(b"\x03\xff")))
case("R2 Rebuild build 缺省 -> 表达式重算", lambda: (
    Struct("n" / Byte, "r" / Rebuild(Byte, this.n * 2)).build({"n": 3})))
case("R3 Rebuild build 显式值 -> 忽略，仍重算", lambda: (
    Struct("n" / Byte, "r" / Rebuild(Byte, this.n * 2)).build({"n": 3, "r": 99})))

# ---------------------------------------------------------------- Computed
case("T1 Computed parse: 表达式值入 Container", lambda: (
    Struct("x" / Byte, "c" / Computed(this.x * 2)).parse(b"\x05")))
case("T2 Computed build 缺省 -> 不写不校验（no-op）", lambda: (
    Struct("x" / Byte, "c" / Computed(this.x * 2)).build({"x": 5})))
case("T3 Computed build 显式值 -> 忽略（no-op）", lambda: (
    Struct("x" / Byte, "c" / Computed(this.x * 2)).build({"x": 5, "c": 123})))
case("T4 Computed 常量 parse", lambda: (
    Struct("c" / Computed(42)).parse(b"")))

# ---------------------------------------------------------------- Padding
case("P1 Padding parse: 消费字节后 Container 里存什么", lambda: (
    Struct("x" / Byte, "p" / Padding(2), "y" / Byte).parse(b"\x01\x00\x00\x02")))
case("P2 Padding build 缺省 -> 写 pattern 字节", lambda: (
    Struct("x" / Byte, "p" / Padding(2), "y" / Byte).build({"x": 1, "y": 2})))
case("P3 Padding build 显式给值（非 None）-> 行为", lambda: (
    Struct("x" / Byte, "p" / Padding(2), "y" / Byte).build({"x": 1, "p": b"zz", "y": 2})))

# ---------------------------------------------------------------- Tell
case("L1 Tell parse/build", lambda: (
    (Struct("a" / Tell(), "x" / Byte, "b" / Tell()).parse(b"\x05"),
     Struct("a" / Tell(), "x" / Byte, "b" / Tell()).build({"x": 5}))))

# ---------------------------------------------------------------- Pass
case("S1 Pass parse/build（无值字段对照）", lambda: (
    (Struct("x" / Byte, "n" / Pass, "y" / Byte).parse(b"\x01\x02"),
     Struct("x" / Byte, "n" / Pass, "y" / Byte).build({"x": 1, "y": 2}))))

# ------------------------------------------------- 表达式 length × 嵌套（B2 同构）
case("E1 顶层 Bytes(this.x+1) parse", lambda: (
    Struct("x" / Byte, "y" / Bytes(this.x + 1)).parse(b"\x01\x02ab")))
case("E2 顶层 Bytes(this.x+1) build", lambda: (
    Struct("x" / Byte, "y" / Bytes(this.x + 1)).build({"x": 1, "y": b"ab"})))
case("E3 嵌套 Prefixed(Int8ub, Bytes(this.x+1)) parse（B2 同构）", lambda: (
    Struct("x" / Byte, "y" / Prefixed(Int8ub, Bytes(this.x + 1))).parse(b"\x01\x02ab")))
case("E4 嵌套 Prefixed(Int8ub, Bytes(this.x + 1)) build", lambda: (
    Struct("x" / Byte, "y" / Prefixed(Int8ub, Bytes(this.x + 1))).build({"x": 1, "y": b"ab"})))
case("E5 Const x + Bytes(this.x+1) build 缺省 y（B1 同构）", lambda: (
    Struct("x" / Const(1, Byte), "y" / Bytes(this.x + 1)).build(Container())))
case("E6 Const x + Bytes(this.x+1) build 缺省 x 和 y", lambda: (
    Struct("x" / Const(1, Byte), "y" / Bytes(this.x + 1)).build({})))

# ------------------------------------------------- Container 缺键时的 obj 语义
case("M1 普通字段缺键 build -> obj 是 None（报错形态）", lambda: (
    Struct("y" / Bytes(2)).build({})))
case("M2 Int 字段缺键 build -> 报错形态", lambda: (
    Struct("y" / Byte).build({})))


def main():
    for name, fn in CASES:
        try:
            r = fn()
            print(f"[OK ] {name}: {r!r}")
        except Exception as e:
            print(f"[ERR] {name}: {type(e).__name__}: {str(e)[:160]}")


if __name__ == "__main__":
    main()
