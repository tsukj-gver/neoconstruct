import sys
import importlib.util

# Inject construct_rust as construct
sys.path.insert(0, 'construct-py')
import construct_rust
import construct_rust.gallery
sys.modules["construct"] = construct_rust
sys.modules["gallery"] = construct_rust.gallery

import construct_rust.lib
import construct_rust.lib.containers
import construct_rust.lib.binary
import construct_rust.lib.bitstream
import construct_rust.lib.hex
import construct_rust.lib.py3compat
sys.modules["construct.lib"] = construct_rust.lib
sys.modules["construct.lib.containers"] = construct_rust.lib.containers
sys.modules["construct.lib.binary"] = construct_rust.lib.binary
sys.modules["construct.lib.bitstream"] = construct_rust.lib.bitstream
sys.modules["construct.lib.hex"] = construct_rust.lib.hex
sys.modules["construct.lib.py3compat"] = construct_rust.lib.py3compat

from construct import *
from construct.lib import *

# Test enum_issue_298 sizeof
d = Struct(
    "ctrl" / Enum(Byte, NAK=0x15, STX=0x02),
    Probe(),
    "optional" / If(lambda this: this.ctrl == "NAK", Byte),
)
print('--- enum_issue_298 sizeof ---')
try:
    print('result:', d.sizeof())
except Exception as e:
    print('ERR:', type(e).__name__, e)

# Test from_issue_246
print('\n--- from_issue_246 sizeof ---')
NumVertices = Bitwise(Aligned(8, Struct(
    'numVx4' / BitsInteger(4),
    'numVx8' / If(this.numVx4 == 0, BitsInteger(8)),
    'numVx16' / If(this.numVx4 == 0 & this.numVx8 == 255, BitsInteger(16)),
)))
try:
    print('result:', NumVertices.sizeof())
except Exception as e:
    print('ERR:', type(e).__name__, e)

# Test focusedseq missing
print('\n--- focusedseq missing parse ---')
d = FocusedSeq("missing", Pass)
try:
    print('parse result:', d.parse(b""))
except Exception as e:
    print('ERR:', type(e).__name__, e)
