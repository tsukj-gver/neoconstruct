"""VET 4.5 验证脚本：PyCallable 谓词的 context proxy 是否包含 Struct 字段。

Python construct 的 RepeatUntil 谓词接收 `(obj, list, context)`，其中 context 是
当前 Struct 的 context，可访问任何前序字段。

例如:
    @dataclass
    class Packet(StructMixin):
        threshold: int = field(Int8ub)
        data: list = field(RepeatUntil(
            lambda x, lst, ctx: x > ctx.threshold,  # 访问 ctx.threshold
            Int8ub,
        ))

此脚本验证 construct-rs 实现是否支持这种 Python 用法。
"""
import sys


def test_python_construct():
    """Python 原版行为：ctx.threshold 可访问。"""
    from construct import Struct, Int8ub, RepeatUntil  # type: ignore

    # 顶层 Struct，threshold 字段在前，data 是 RepeatUntil
    pkt = Struct(
        "threshold" / Int8ub,
        "data" / RepeatUntil(lambda x, lst, ctx: x > ctx.threshold, Int8ub),
    )
    # threshold=5, data: 1,2,3,4,5,6 — 第 6 个 (6>5) 满足
    result = pkt.parse(b"\x05\x01\x02\x03\x04\x05\x06\x07")
    print(f"[Python] threshold={result.threshold}, data={list(result.data)}")
    assert list(result.data) == [1, 2, 3, 4, 5, 6], f"Python: expected [1..6], got {list(result.data)}"
    print("[Python] PASS: ctx.threshold accessible from predicate")


def test_construct_rust():
    """construct-rs 行为：ctx_proxy 仅含 _index，访问 ctx.threshold 会 KeyError。"""
    try:
        sys.path.insert(0, r"<legacy-repo>\construct-rs\python")
        from construct import StructMixin, field, rfield  # type: ignore
        from construct import Int8ub, RepeatUntil  # type: ignore
        from dataclasses import dataclass

        @dataclass
        class Packet(StructMixin):
            threshold: int = rfield(Int8ub)
            data: list = field(RepeatUntil(
                lambda x, lst, ctx: x > ctx["threshold"],  # 用 item 访问
                Int8ub,
            ))

        try:
            result = Packet.parse(b"\x05\x01\x02\x03\x04\x05\x06\x07")
            print(f"[Rust] threshold={result.threshold}, data={list(result.data)}")
            print("[Rust] PASS: ctx['threshold'] accessible from predicate")
        except KeyError as e:
            print(f"[Rust] FAIL (KeyError): predicate cannot access ctx field via item — {e}")
            print("         The proxy is missing the 'threshold' key (only '_index' present).")
        except Exception as e:
            print(f"[Rust] FAIL with {type(e).__name__}: {e}")

        # 测试 2: 仅访问 _index（应工作）
        @dataclass
        class Packet2(StructMixin):
            data: list = field(RepeatUntil(
                lambda x, lst, ctx: ctx["_index"] >= 2,
                Int8ub,
            ))

        try:
            result = Packet2.parse(b"\x01\x02\x03\x04\x05")
            print(f"[Rust] PASS: ctx['_index'] accessible, data={list(result.data)}")
        except Exception as e:
            print(f"[Rust] FAIL (_index test): {type(e).__name__}: {e}")
    except Exception as e:
        print(f"[Rust] SKIP: {type(e).__name__}: {e}")


if __name__ == "__main__":
    print("=" * 70)
    print("VET 4.5 验证：PyCallable 谓词 context 访问范围")
    print("=" * 70)
    print()
    print("--- Python construct (原版行为) ---")
    try:
        test_python_construct()
    except Exception as e:
        print(f"[Python] ERROR: {type(e).__name__}: {e}")
    print()
    print("--- construct-rs (实现行为) ---")
    test_construct_rust()
