"""``construct.lib.containers`` —— Container / ListContainer 数据结构。

construct-rs Phase 4 子任务 4.5 V-1 修正：RepeatUntil PyCallable 谓词路径
需要构造 Container proxy（支持 attribute + item 双重访问），对齐 Python
原版 ``construct.lib.containers.Container`` 的行为。

本文件是原版 ``construct/construct/lib/containers.py`` 的子集实现：

- 仅保留 ``Container`` 与 ``ListContainer`` 两个核心类；
- 移除对 ``construct.lib.py3compat`` 的依赖（construct-rs 不需要）；
- 保留 ``self.__dict__ = self`` 关键语义（attribute 访问等同于 item 访问），
  这是 RepeatUntil 谓词 ``lambda x, lst, ctx: x > ctx.threshold`` 可用的基础；
- 保留 ``__eq__`` 的"忽略下划线开头的私有字段（如 _index）"语义，对齐
  Python 用户对 ``Container == Container`` 的预期。

设计依据：
- ``docs/模块设计-Array.md`` §2.5 决策 A5 v4 + §4.3.3 ``build_context_proxy``
- ``docs/设计决策记录.md`` Phase 4 决策 4
- 原版参考：``construct/construct/lib/containers.py``
"""

import re


globalPrintFullStrings = False
globalPrintFalseFlags = False
globalPrintPrivateEntries = False


def setGlobalPrintFullStrings(enabled=False):
    """控制 Container __str__ 是否完整输出 bytes/str（默认截断）。"""
    global globalPrintFullStrings
    globalPrintFullStrings = enabled


def setGlobalPrintFalseFlags(enabled=False):
    """控制 FlagsEnum 解析后 __str__ 是否输出为 False 的项。"""
    global globalPrintFalseFlags
    globalPrintFalseFlags = enabled


def setGlobalPrintPrivateEntries(enabled=False):
    """控制 __str__ 是否显示 _index 等私有条目。"""
    global globalPrintPrivateEntries
    globalPrintPrivateEntries = enabled


def recursion_lock(retval="<recursion detected>", lock_name="__recursion_lock__"):
    """内部使用：递归打印保护装饰器。"""
    def decorator(func):
        def wrapper(self, *args, **kw):
            if getattr(self, lock_name, False):
                return retval
            setattr(self, lock_name, True)
            try:
                return func(self, *args, **kw)
            finally:
                delattr(self, lock_name)

        wrapper.__name__ = func.__name__
        return wrapper

    return decorator


def value_to_string(value):
    """值的字符串化（用于 __str__ 美化输出）。"""
    if value.__class__.__name__ == "EnumInteger":
        return "(enum) (unknown) %s" % (value,)

    if value.__class__.__name__ == "EnumIntegerString":
        return "(enum) %s %s" % (value, value.intvalue,)

    if value.__class__.__name__ in ["HexDisplayedBytes", "HexDumpDisplayedBytes"]:
        return str(value)

    if isinstance(value, bytes):
        printingcap = 16
        if len(value) <= printingcap or globalPrintFullStrings:
            return "%s (total %d)" % (repr(value), len(value))
        return "%s... (truncated, total %d)" % (repr(value[:printingcap]), len(value))

    if isinstance(value, str):
        printingcap = 32
        if len(value) <= printingcap or globalPrintFullStrings:
            return "%s (total %d)" % (repr(value), len(value))
        return "%s... (truncated, total %d)" % (repr(value[:printingcap]), len(value))

    return str(value)


class Container(dict):
    # NOTE: be careful when working with these objects. Any method can be shadowed,
    # so instead of doing `self.items()` you should do `dict.items(self)`.
    r"""
    Generic ordered dictionary that allows both key and attribute access, and
    preserves key order by insertion. Adding keys is preferred using \*\*entrieskw.
    Equality does NOT check item order. Also provides regex searching.

    construct-rs 在 RepeatUntil PyCallable 谓词路径中构造此类的实例作为 context
    proxy（包含外层 Struct 字段 + _index），对齐 Python construct core.py L2681/L2697
    中"谓词收到的 context 是 Container"的语义。
    """
    __slots__ = ('__dict__', '__recursion_lock__')

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        # 关键：让 attribute 访问 (c.threshold) 等同于 item 访问 (c['threshold'])
        self.__dict__ = self

    def copy(self, /):
        return self.__class__(self)

    def __copy__(self, /):
        return self.__class__.copy(self)

    def __deepcopy__(self, _, /):
        return self.__class__.copy(self)

    def __dir__(self, /):
        return list(self.__class__.keys(self)) + list(self.__class__.__dict__) + dir(super(Container, self))

    def __eq__(self, other, /):
        if self is other:
            return True
        if not isinstance(other, dict):
            return False

        def isequal(v1, v2):
            return v1 == v2

        for k, v in self.__class__.items(self):
            if isinstance(k, str) and k.startswith("_"):
                continue
            if k not in other or not isequal(v, other[k]):
                return False
        for k, v in other.__class__.items(other):
            if isinstance(k, str) and k.startswith("_"):
                continue
            if k not in self or not isequal(v, self[k]):
                return False
        return True

    def __ne__(self, other, /):
        return not self == other

    @recursion_lock()
    def __repr__(self, /):
        parts = []
        for k, v in self.__class__.items(self):
            if isinstance(k, str) and k.startswith("_"):
                continue
            parts.append(f'{k}={v!r}')
        return "Container(%s)" % ", ".join(parts)

    @recursion_lock()
    def __str__(self, /):
        indentation = "\n    "
        text = ["Container: "]
        isflags = getattr(self, "_flagsenum", False)
        for k, v in self.__class__.items(self):
            if isinstance(k, str) and k.startswith("_") and not globalPrintPrivateEntries:
                continue
            if isflags and not v and not globalPrintFalseFlags:
                continue
            text.extend([indentation, str(k), " = ", indentation.join(value_to_string(v).split("\n"))])
        return "".join(text)

    def _search(self, compiled_pattern, search_all, /):
        items = []
        for key, value in self.__class__.items(self):
            try:
                if isinstance(value, (Container, ListContainer)):
                    ret = value.__class__._search(value, compiled_pattern, search_all)
                    if ret is not None:
                        if search_all:
                            items.extend(ret)
                        else:
                            return ret
                elif compiled_pattern.match(key):
                    if search_all:
                        items.append(value)
                    else:
                        return value
            except Exception:
                pass
        if search_all:
            return items
        else:
            return None

    def search(self, pattern):
        """非递归正则搜索（按 key）。"""
        compiled_pattern = re.compile(pattern)
        return self.__class__._search(self, compiled_pattern, False)

    def search_all(self, pattern):
        """递归正则搜索（按 key）。"""
        compiled_pattern = re.compile(pattern)
        return self.__class__._search(self, compiled_pattern, True)

    def __getstate__(self, /):
        return dict(self)

    def __setstate__(self, state, /):
        self.__class__.clear(self)
        self.__class__.update(self, state)


class ListContainer(list):
    r"""
    Generic container like list. Provides pretty-printing. Also provides regex
    searching.

    construct-rs 当前不直接构造 ListContainer（Array 系列返回原生 list），
    但保留此类以兼容可能导入此模块的用户代码与 isinstance 检查。
    """
    @recursion_lock()
    def __repr__(self, /):
        return "ListContainer(%s)" % (list.__repr__(self),)

    @recursion_lock()
    def __str__(self, /):
        indentation = "\n    "
        text = ["ListContainer: "]
        for k in self:
            text.append(indentation)
            lines = value_to_string(k).split("\n")
            text.append(indentation.join(lines))
        return "".join(text)

    def _search(self, compiled_pattern, search_all, /):
        items = []
        for item in self:
            try:
                ret = item.__class__._search(item, compiled_pattern, search_all)
            except Exception:
                continue
            if ret is not None:
                if search_all:
                    items.extend(ret)
                else:
                    return ret
        if search_all:
            return items
        else:
            return None

    def search(self, pattern):
        compiled_pattern = re.compile(pattern)
        return self._search(compiled_pattern, False)

    def search_all(self, pattern):
        compiled_pattern = re.compile(pattern)
        return self._search(compiled_pattern, True)
