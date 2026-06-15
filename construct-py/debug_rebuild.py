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

# Test the failing case
d = Struct(
    "sequence" / Sequence(Computed(1), Computed(2), Computed(3), Computed(4)),
    Check(this.sequence == [1,2,3,4]),
)
obj = Container()
try:
    result = d.build(obj)
    print('Built:', result)
except Exception as e:
    print('ERR:', type(e).__name__, e)

print('\n--- with upfield/nestedstruct ---')
d = Struct(
    "upfield" / Computed(200),
    "nestedstruct" / Struct(
        "nestedfield" / Computed(255),
        Check(this._.upfield == 200),
        Check(this.nestedfield == 255),
    ),
    Check(this.upfield == 200),
    Check(this.nestedstruct.nestedfield == 255),
)
obj = Container()
try:
    result = d.build(obj)
    print('Built:', result)
except Exception as e:
    print('ERR:', type(e).__name__, e)

# Test just Bytes(1) build_effective
print('\n--- Bytes(1) build_effective ---')
d2 = Struct("b" / Bytes(1))
try:
    result = d2.build(dict(b=0))
    print('Built:', result)
except Exception as e:
    print('ERR:', type(e).__name__, e)
