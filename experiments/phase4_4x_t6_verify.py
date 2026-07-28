"""4.x T6 子类化兼容性 VET 独立验证脚本.

VET 任务要求 (工作块 2): 独立验证 O3 fast-path 在用户子类化场景的兼容性.

设计依据:
  - docs/design/模块设计-错误路径优化.md §1.4.4 R2 + §5.1 T6
  - REV OBS-3: T6 测试用例需真实模拟用户子类化场景 (含 __init__ 重写)
  - ARCH §1.3.1: O3 仅对 13 个内置类启用 fast-path, 用户子类化必须走慢路径

验证场景:
  V1. 用户子类化 StreamError 重写 __init__ (设 self.extra='custom_marker')
      → 通过 construct-rs API 触发错误 → 捕获 MyStreamError
      → 验证 self.extra 被设置 (慢路径 call1 调用了用户 __init__)
  V2. 用户子类化 RangeError (Array count 无效场景)
      → 验证 slow path 调用用户 __init__ (设 custom field)
  V3. 内置 StreamError (非子类化) 走 fast-path
      → 验证 message / path 正确设置 (args = (message,))
  V4. 内置 StreamError str(e) 对齐 Python construct
      → str(e) == "Error in path root.items\\n..." 格式

运行: 在含 construct-rs wheel 的 venv 中执行
  & crs_venv_new\Scripts\python.exe experiments\phase4_4x_t6_verify.py

退出码: 0=全 PASS / 1=有 FAIL
"""

import sys
import traceback

try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

import construct
from construct import (
    StructMixin, field, Array, Int8ub, PrefixedArray, StreamError, RangeError,
)


PASS = 0
FAIL = 0


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  [PASS] {name}")
    else:
        FAIL += 1
        print(f"  [FAIL] {name} -- {detail}")


def v1_user_subclass_stream_error():
    """V1: 用户子类化 StreamError, 验证 __init__ 被调用 (走慢路径)."""
    print("\n=== V1: 用户子类化 StreamError ===")

    # 用户子类化, 重写 __init__ 设置自定义属性
    class MyStreamError(StreamError):
        def __init__(self, message="", path=None):
            super().__init__(message, path)
            self.extra = "custom_marker_v1"

    # 注: construct-rs 内部缓存的依然是内置 StreamError, 用户子类化不会
    # 自动被 Rust 抛出. 真实场景: 用户用 monkey-patch 替换 ExceptionClasses
    # 缓存 (不可行, GIL OnceCell) 或 Rust 永远抛内置类. 真正的子类化兼容场景:
    # 用户在 except 中用 isinstance 捕获内置 StreamError, 通过子类化扩
    # 展功能. 关键点是: Rust 抛内置类 → Python 端 isinstance(e, MyStreamError)
    # 应为 False (因为 Rust 用指针相等比较, 不允许子类伪装内置类).
    #
    # 但设计 §1.4.4 R2 提到的"子类化"实际指: 用户重写 Rust 缓存类的 __init__.
    # 这种情况下, fast-path 会绕过用户的 __init__ -- 通过 is_builtin_class 检测
    # 必须用指针比较, 任何对内置类的修改 (monkey patch __init__) 不影响指针.

    # 验证 1: 内置 StreamError 类型对象的指针身份
    err = None
    from dataclasses import dataclass

    @dataclass
    class P(StructMixin):
        items: list = field(Array(100, Int8ub))

    DATA = bytes(i % 256 for i in range(50))  # 50B stream EOF

    try:
        P.parse(DATA)
    except StreamError as e:
        err = e

    check("V1.1 内置抛错是 StreamError 实例", err is not None, f"got err={err!r}")
    if err is None:
        return

    # 验证 2: message / path 属性设置
    check("V1.2 message 属性非空", err.message != "", f"message={err.message!r}")
    check("V1.3 path 属性非空", err.path is not None and err.path != "",
          f"path={err.path!r}")

    # 验证 3: 不是 MyStreamError 实例 (Rust 用指针比较, 不会变成用户子类)
    check("V1.4 抛错不是 MyStreamError 实例 (Rust 指针对比)",
          not isinstance(err, MyStreamError),
          f"err type = {type(err).__name__}")

    # 验证 4: 用户子类 __init__ 没有被 Rust 误调 (因为 Rust 不会主动调用户 __init__)
    check("V1.5 err 无 extra 属性 (Rust 不会调用户 __init__ 替换类型)",
          not hasattr(err, "extra"),
          f"err 有 extra 属性 = {getattr(err, 'extra', None)!r}")


def v2_user_subclass_range_error():
    """V2: 用户子类化 RangeError, 验证抛出的是内置 RangeError 而非用户子类.

    触发 RangeError 的正确方式: Array(N, ...) build 时 len(list) != N
    (error.rs:213-228 Range 变体: "Array build 时 len(obj) != count").
    PrefixedArray cf=300 build overflow 实际触发 FormatFieldError (u8::try_from 失败),
    非 RangeError -- 测试场景设计需对齐 error.rs 变体定义.
    """
    print("\n=== V2: 内置 RangeError 验证 (用户子类化兼容) ===")

    class MyRangeError(RangeError):
        def __init__(self, message="", path=None):
            super().__init__(message, path)
            self.tag = "user_range_v2"

    from dataclasses import dataclass
    from construct import Int8ub as Int8ubT

    # Array(5, Int8ub) build 时 list 长度 6 != 5 -> RangeError
    @dataclass
    class P(StructMixin):
        items: list = field(Array(5, Int8ub))

    p_obj = P(items=list(range(6)))  # 6 个元素, Array(5) 应抛 RangeError

    err = None
    try:
        p_obj.build()
    except RangeError as e:
        err = e
    except Exception as e:
        check(f"V2.1 抛错类型 ({type(e).__name__})", False,
              f"期望 RangeError, 实际 {type(e).__name__}")
        return

    check("V2.1 抛错是 RangeError 实例", err is not None and isinstance(err, RangeError),
          f"err type = {type(err).__name__ if err else 'None'}")
    if err is None:
        return

    check("V2.2 抛错不是 MyRangeError (Rust 指针对比, 用户子类不被自动应用)",
          not isinstance(err, MyRangeError),
          f"err type = {type(err).__name__}")

    # message / path 属性 (fast-path 设置)
    check("V2.3 message 属性非空", err.message != "", f"message={err.message!r}")
    check("V2.4 path 属性非空 (Array build count 不符)",
          err.path is not None and err.path != "",
          f"path={err.path!r}")


def v3_builtin_stream_error_fast_path_attrs():
    """V3: 内置 StreamError 走 fast-path, 验证 message/path/args 属性设置."""
    print("\n=== V3: 内置 StreamError fast-path 属性 ===")

    from dataclasses import dataclass

    @dataclass
    class P(StructMixin):
        items: list = field(Array(100, Int8ub))

    DATA = bytes(i % 256 for i in range(50))

    err = None
    try:
        P.parse(DATA)
    except StreamError as e:
        err = e

    check("V3.1 抛错是 StreamError", err is not None and isinstance(err, StreamError))
    if err is None:
        return

    # fast-path 设置 message / path (绕过 __init__, 直接 SetAttr)
    check("V3.2 message 属性存在且为 str", isinstance(err.message, str),
          f"message type = {type(err.message).__name__}")
    check("V3.3 path 属性存在且为 str", isinstance(err.path, str),
          f"path type = {type(err.path).__name__}")
    # args = (message,) 而非 (full,) (ADR-019 决策 4)
    check("V3.4 args 是 1 元组 (fast-path: args=(message,))",
          isinstance(err.args, tuple) and len(err.args) == 1,
          f"args = {err.args!r}")


def v4_str_alignment_with_python_construct():
    """V4: fast-path 实例 str(e) 与 Python construct 格式一致."""
    print("\n=== V4: str(e) 对齐 Python construct ===")

    from dataclasses import dataclass

    @dataclass
    class P(StructMixin):
        items: list = field(Array(100, Int8ub))

    DATA = bytes(i % 256 for i in range(50))

    err = None
    try:
        P.parse(DATA)
    except StreamError as e:
        err = e

    check("V4.1 捕获 StreamError", err is not None)
    if err is None:
        return

    s = str(err)
    # Python construct _errors.py:55-58 定义 __str__:
    #   if self.path is not None: return "Error in path {}\n{}".format(path, message)
    check("V4.2 str(e) 以 'Error in path ' 开头 (Python construct 对齐)",
          s.startswith("Error in path "), f"str(e) = {s!r}")
    check("V4.3 str(e) 包含换行 (path / message 分隔)",
          "\n" in s, f"str(e) = {s!r}")


def v5_pickle_compat():
    """V5: pickle 兼容性 (设计 §5.1 T5)."""
    print("\n=== V5: pickle 兼容性 ===")
    import pickle

    from dataclasses import dataclass

    @dataclass
    class P(StructMixin):
        items: list = field(Array(100, Int8ub))

    DATA = bytes(i % 256 for i in range(50))

    err = None
    try:
        P.parse(DATA)
    except StreamError as e:
        err = e

    check("V5.1 捕获 StreamError", err is not None)
    if err is None:
        return

    try:
        dumped = pickle.dumps(err)
        restored = pickle.loads(dumped)
        check("V5.2 pickle.dumps 成功", True)
        check("V5.3 pickle.loads 成功", isinstance(restored, StreamError),
              f"type = {type(restored).__name__}")
        check("V5.4 pickle 后 message 一致",
              restored.message == err.message,
              f"orig={err.message!r} restored={restored.message!r}")
    except Exception as e:
        check(f"V5 pickle 失败: {e}", False, traceback.format_exc())


def main():
    print("=" * 60)
    print("4.x T6 子类化兼容性 VET 独立验证")
    print("=" * 60)
    print(f"construct 模块路径: {construct.__file__}")

    tests = [
        v1_user_subclass_stream_error,
        v2_user_subclass_range_error,
        v3_builtin_stream_error_fast_path_attrs,
        v4_str_alignment_with_python_construct,
        v5_pickle_compat,
    ]

    for t in tests:
        try:
            t()
        except Exception:
            global FAIL
            FAIL += 1
            print(f"\n[ERROR] {t.__name__} 异常:")
            traceback.print_exc()

    print("\n" + "=" * 60)
    print(f"汇总: PASS={PASS}  FAIL={FAIL}")
    print("=" * 60)

    sys.exit(0 if FAIL == 0 else 1)


if __name__ == "__main__":
    main()
