"""Pytest configuration -- injects ``construct_rust`` as ``construct``.

Importing this conftest *before* any test module executes ``import construct``
causes ``sys.modules['construct']`` to resolve to the :mod:`construct_rust`
package, so the upstream test suite (which does ``from construct import *``)
runs against the Rust-backed implementation without source modifications.

Caveat: the genuine ``construct`` package must not already be importable in the
same environment (uninstall it first), otherwise the injection is shadowed.
"""

import sys

import construct_rust

sys.modules["construct"] = construct_rust