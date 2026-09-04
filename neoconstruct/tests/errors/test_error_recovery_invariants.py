"""错误恢复不变量：parse/build 失败后的系统状态。

锁定三条不变量：
1. parse 失败后无残留状态——同类正确数据再次 parse 成功且值正确
2. parse 失败不修改输入数据（可变 bytearray 输入字节不变）
3. build 失败不影响后续实例的 build（实例间相互独立）

期望值来源标注：[自然]=最小惊讶（失败操作不留副作用）；[基线]=实测绿灯行为。
"""

from dataclasses import dataclass

from neoconstruct import (
    Bytes,
    FieldLengthError,
    FormatFieldError,
    Int8ub,
    StreamError,
    StructMixin,
    field,
)


def test_parse_failure_then_success_no_residual_state():
    """parse 失败 → 同类正确数据立即 parse 成功，值逐一正确。[自然]"""

    @dataclass
    class P(StructMixin):
        a: int = field(Int8ub)
        b: bytes = field(Bytes(2))

    # 第一次失败（b 需 2 字节，仅给 1）
    try:
        P.parse(b"\x01\x02")
        raise AssertionError("should have raised StreamError")
    except StreamError:
        pass

    # 第二次正确数据 → 成功且值正确（无残留上下文/流状态）
    pkt = P.parse(b"\x01\x02\x03")
    assert pkt.a == 1
    assert pkt.b == b"\x02\x03"

    # 第三次再次失败（确认失败路径可重复）
    try:
        P.parse(b"\x01")
        raise AssertionError("should have raised StreamError")
    except StreamError:
        pass


def test_parse_failure_leaves_input_bytes_unmodified():
    """parse 失败不修改输入数据；非 bytes 输入被显式拒绝。[基线]

    输入类型契约：parse 仅接受 bytes（bytearray/str → TypeError）。
    失败后断言原输入内容不变。
    """

    @dataclass
    class P(StructMixin):
        a: int = field(Int8ub)
        b: int = field(Int8ub)

    original = bytes([0x01])
    data = bytes(original)

    try:
        P.parse(data)
        raise AssertionError("should have raised StreamError")
    except StreamError:
        pass

    assert data == original

    # 非 bytes 输入 → 显式 TypeError（输入类型契约）
    for bad_input in (bytearray(b"\x01\x02"), "string", None):
        try:
            P.parse(bad_input)
            raise AssertionError("should have raised TypeError for {!r}".format(bad_input))
        except TypeError:
            pass


def test_build_failure_does_not_affect_subsequent_builds():
    """实例 A build 失败不影响实例 B build（实例独立性）。[自然]"""

    @dataclass
    class P(StructMixin):
        x: int = field(Int8ub)
        payload: bytes = field(Bytes(2))

    # A：长度不匹配 → FieldLengthError
    try:
        P(x=1, payload=b"toolong").build()
        raise AssertionError("should have raised FieldLengthError")
    except FieldLengthError:
        pass

    # B：正确实例 build 正常
    built = P(x=1, payload=b"ok").build()
    assert built == b"\x01ok"

    # C：值超范围 → FormatFieldError（失败面独立可重复）
    try:
        P(x=999, payload=b"ok").build()  # type: ignore[arg-type]
        raise AssertionError("should have raised FormatFieldError")
    except FormatFieldError:
        pass
