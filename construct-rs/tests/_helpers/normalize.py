"""跨实现比对前的规范化工具。

规则要点：
1. ``_`` 开头键过滤：Python construct Container 含 ``_io`` / ``_index`` 等内部状态，
   construct-rs 用户面 dataclass 不暴露这些；过滤后两侧 dict 才能比对。
   construct-rs dataclass 不允许 ``_`` 开头字段（PEP 8 + identifier 限制），故过滤无风险。
2. bytes 包装为 ``{"__bytes__": hex}``：JSON 不能直接序列化 bytes。
3. float 精度：Rust f32/f64 与 Python float 在某些位模式有最后 1 位差异，round(6) 容忍。
4. NaN/Inf 统一字符串哨兵：JSON 不能直接序列化 NaN/Infinity，且 ``NaN != NaN``
   会使 rs==py 永远 False。两侧都转为同字符串后比对成立。

注意：本函数引入的中间形态（``{"__bytes__": hex}`` / ``"__NaN__"``）
仅存在于测试子进程的 JSON 输出，用于跨 rs/py 实现比对；**不进入 construct-rs 的
parse/build 运行时数据路径**（运行时无中间表示层）。
"""

from __future__ import annotations


def normalize(value):
    """规范化输出值，便于跨实现（construct-rs vs Python construct）比较。

    参数：
        value: 任意 parse 结果（dict / list / bytes / int / float / ...）

    返回：
        规范化后的 JSON 可序列化值。

    注：函数故意不加类型注解 —— 本函数源码会通过 ``inspect.getsource`` 注入
    子进程执行，子进程无 ``typing.Any`` 可用，注解会在 def 执行期被求值而
    抛 ``NameError``。docstring 已说明参数/返回类型。

    规则（递归）：
        - dict / Container → 排序后的 dict，过滤以 ``_`` 开头的键（如 _io / _index）
        - list / tuple / ListContainer → list（递归）
        - bytes / bytearray → ``{"__bytes__": hex_str}`` 包装（dict 不能直接装 bytes）
        - float → 特殊值优先：NaN → ``"__NaN__"``、+Inf → ``"__+Inf__"``、
          -Inf → ``"__-Inf__"``；普通 float → round 到 6 位小数
        - 其他（int / bool / None / str）原样返回
    """
    # dict / Container（Python construct Container 是 dict 子类，isinstance 成立）
    if isinstance(value, dict):
        return {
            k: normalize(value[k])
            for k in sorted(value.keys(), key=lambda x: str(x))
            if not str(k).startswith("_")
        }
    # list / tuple / ListContainer
    if isinstance(value, (list, tuple)):
        return [normalize(v) for v in value]
    # bytes / bytearray
    if isinstance(value, (bytes, bytearray)):
        return {"__bytes__": bytes(value).hex()}
    # float（含 NaN/Inf 特殊值）
    if isinstance(value, float):
        if value != value:  # NaN
            return "__NaN__"
        if value == float("inf"):
            return "__+Inf__"
        if value == float("-inf"):
            return "__-Inf__"
        return round(value, 6)
    # int / bool / None / str 原样返回
    return value
