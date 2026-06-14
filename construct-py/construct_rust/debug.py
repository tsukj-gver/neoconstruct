"""Probe and Debugger debugging tools.

Stub implementations ported from construct.debug. Both test_probe and
test_debugger are marked as skipped in Phase 10, so these classes only
need to be importable and instantiable. The printout/handle_exc methods
are ported line-by-line from the original for output fidelity.
"""

import sys
import traceback
import pdb

from ._adapter import Construct, Subconstruct
from .lib.binary import hexlify
from .expr import evaluate


class Probe(Construct):
    r"""
    Probe that dumps the context, and some stream content (peeks into it) to the screen to aid the debugging process. It can optionally limit itself to a single context entry, instead of printing entire context.

    :param into: optional, None by default, or context lambda
    :param lookahead: optional, integer, number of bytes to dump from the stream
    """

    def __init__(self, into=None, lookahead=None):
        super().__init__()
        self.flagbuildnone = True
        self.into = into
        self.lookahead = lookahead

    def _parse(self, stream, context, path):
        self.printout(stream, context, path)

    def _build(self, obj, stream, context, path):
        self.printout(stream, context, path)

    def _sizeof(self, context, path):
        self.printout(None, context, path)
        return 0

    def printout(self, stream, context, path):
        print("--------------------------------------------------")
        print("Probe, path is %s, into is %r" % (path, self.into, ))

        if self.lookahead and stream is not None:
            fallback = stream.tell()
            datafollows = stream.read(self.lookahead)
            stream.seek(fallback)
            if datafollows:
                print("Stream peek: (hexlified) %s..." % (hexlify(datafollows), ))
            else:
                print("Stream peek: EOF reached")

        if context is not None:
            if self.into:
                try:
                    subcontext = evaluate(self.into, context)
                    print(subcontext)
                except Exception:
                    print("Failed to compute %r on the context %r" % (self.into, context, ))
            else:
                print(context)
        print("--------------------------------------------------")


class Debugger(Subconstruct):
    r"""
    PDB-based debugger. When an exception occurs in the subcon, a debugger will appear and allow you to debug the error (and even fix it on-the-fly).

    :param subcon: Construct instance, subcon to debug
    """

    def _parse(self, stream, context, path):
        try:
            return super()._parse(stream, context, path)
        except Exception:
            self.retval = NotImplemented
            self.handle_exc(path, msg="(you can set self.retval, which will be returned from method)")
            if self.retval is NotImplemented:
                raise
            else:
                return self.retval

    def _build(self, obj, stream, context, path):
        try:
            return super()._build(obj, stream, context, path)
        except Exception:
            self.handle_exc(path)

    def _sizeof(self, context, path):
        try:
            return super()._sizeof(context, path)
        except Exception:
            self.handle_exc(path)

    def handle_exc(self, path, msg=None):
        print("--------------------------------------------------")
        print("Debugging exception of %r" % (self.subcon, ))
        print("path is %s" % (path, ))
        print("".join(traceback.format_exception(*sys.exc_info())[1:]))
        if msg:
            print(msg)
        pdb.post_mortem(sys.exc_info()[2])
        print("--------------------------------------------------")
