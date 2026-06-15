"""Pure-Python fallback for FocusedSeq with dynamic parsebuildfrom.

Used when ``parsebuildfrom`` is a context lambda (Path) instead of a string.
The Rust-backed FocusedSeq only accepts a static string selector.
"""

import io

from ._adapter import Construct, _filter_context
from .lib.containers import Container
from .expr import evaluate


class _FocusedSeqExpr(Construct):
    """FocusedSeq with dynamic parsebuildfrom (context lambda)."""

    def __init__(self, parsebuildfrom, subcons):
        super().__init__()
        self.parsebuildfrom = parsebuildfrom
        self.subcons = list(subcons)
        self._subcons = Container(
            (getattr(sc, "name", None), sc)
            for sc in self.subcons
            if getattr(sc, "name", None) is not None
        )

    def __getattr__(self, name):
        if name == "_subcons":
            raise AttributeError
        subc = self.__dict__.get("_subcons", {})
        if name in subc:
            return subc[name]
        raise AttributeError(name)

    def _make_subcontext(self, context, stream):
        return Container(
            _=context,
            _params=context.get("_params", context),
            _root=None,
            _parsing=context.get("_parsing", True),
            _building=context.get("_building", False),
            _sizing=context.get("_sizing", False),
            _subcons=self._subcons,
            _io=stream,
            _index=context.get("_index", None),
        )

    def _parse(self, stream, context, path):
        context = self._make_subcontext(context, stream)
        context._root = context._.get("_root", context)
        parsebuildfrom = evaluate(self.parsebuildfrom, context)
        finalret = None
        for i, sc in enumerate(self.subcons):
            if hasattr(sc, "_parsereport"):
                parseret = sc._parsereport(stream, context, path)
            else:
                kw = _filter_context(context)
                parseret = sc.parse_stream(stream, **kw)
            name = getattr(sc, "name", None)
            if name:
                context[name] = parseret
            if name == parsebuildfrom:
                finalret = parseret
        return finalret

    def _build(self, obj, stream, context, path):
        context = self._make_subcontext(context, stream)
        context._root = context._.get("_root", context)
        parsebuildfrom = evaluate(self.parsebuildfrom, context)
        context[parsebuildfrom] = obj
        finalret = None
        for i, sc in enumerate(self.subcons):
            name = getattr(sc, "name", None)
            buildval = obj if name == parsebuildfrom else None
            if hasattr(sc, "_build"):
                buildret = sc._build(buildval, stream, context, path)
            else:
                if getattr(sc, "flagbuildnone", False) or buildval is not None:
                    kw = _filter_context(context)
                    sc.build_stream(buildval, stream, **kw)
                    buildret = buildval
                else:
                    buildret = None
            if name:
                context[name] = buildret
            if name == parsebuildfrom:
                finalret = buildret
        return finalret

    def _sizeof(self, context, path):
        from . import SizeofError

        subcontext = self._make_subcontext(context, None)
        subcontext._root = subcontext._.get("_root", subcontext)
        try:
            total = 0
            for sc in self.subcons:
                if hasattr(sc, "_sizeof"):
                    total += sc._sizeof(subcontext, path)
                else:
                    kw = _filter_context(subcontext)
                    total += sc.sizeof(**kw)
            return total
        except (KeyError, AttributeError):
            raise SizeofError(
                "cannot calculate size, key not found in context in path %s" % str(path)
            )


def _focused_seq_fallback(parsebuildfrom, subcons):
    """Create a Python-side FocusedSeq with dynamic selector."""
    return _FocusedSeqExpr(parsebuildfrom, subcons)
