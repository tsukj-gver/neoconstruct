"""Parity tests: construct-rs vs Python construct 2.10.70 on the same SCTP wire bytes.

Strategy:
  1. Define reference SCTP packets as raw bytes (hand-crafted, one per chunk type)
  2. Parse with both libraries
  3. Normalize both parsed objects to a plain dict (drop private _* fields,
     strip Container wrappers)
  4. Assert normalized dicts equal
  5. Build with both libraries from a normalized dict
  6. Assert build outputs are byte-for-byte equal

Run with either venv (test imports both libs lazily by subprocess).
Actually, we cannot import both constructs in the same process (name clash).
So we run parity in two subprocesses and compare serialized JSON.
"""
import json
import os
import subprocess
import sys


HERE = os.path.dirname(os.path.abspath(__file__))


# ---------------------------------------------------------------------------
# Reference SCTP packets (raw bytes, hand-crafted)
# ---------------------------------------------------------------------------
def make_data_packet():
    """SCTP packet with a single DATA chunk.

    Common header:
      src_port = 0x04D2 (1234)
      dst_port = 0x162E (5678)
      verify_tag = 0xDEADBEEF
      checksum = 0x00000000 (placeholder; CRC handled externally)
    DATA chunk:
      chunk_type = 0x00
      flags = 0x03 (B=1, E=1, unfragmented)
      chunk_length = 0x001B (27 bytes total = 4 header + 12 fixed + 11 user_data)
      tsn = 0x12345678
      stream_id = 0x0001
      stream_seq = 0x0001
      ppid = 0x00000018
      user_data = b"Hello SCTP!" (11 bytes)
    """
    return bytes.fromhex(
        "04d2162e"          # src_port + dst_port
        "deadbeef"          # verify_tag
        "00000000"          # checksum (placeholder)
        "001b"              # chunk_length = 27
        "00"                # chunk_type = 0 (DATA)
        "03"                # flags = 0x03 (B=1, E=1)
        "12345678"          # tsn
        "0001"              # stream_id
        "0001"              # stream_seq
        "00000018"          # ppid
        "48656c6c6f205343545021"  # "Hello SCTP!"
    )


def make_init_packet():
    """SCTP packet with a single INIT chunk + 1 optional TLV param.

    Common header:
      src_port = 0x08AE (2222)
      dst_port = 0x0D05 (3333)
      verify_tag = 0 (must be 0 for INIT per RFC)
      checksum = 0
    INIT chunk:
      chunk_type = 0x01
      flags = 0x00
      chunk_length = 0x001C (28 = 4 header + 16 fixed + 8 TLV)
      initiate_tag = 0xCAFEBABE
      a_rwnd = 0x00010000 (65536)
      num_outbound_streams = 0x000A (10)
      num_inbound_streams = 0x000A (10)
      initial_tsn = 0x00000001
      params = 8 bytes bogus IPv4 TLV (type=5, length=8, addr=10.0.0.1)
    """
    return bytes.fromhex(
        "08ae0d05"          # src_port + dst_port
        "00000000"          # verify_tag (0 for INIT)
        "00000000"          # checksum
        "001c"              # chunk_length = 28
        "01"                # chunk_type = 1 (INIT)
        "00"                # flags
        "cafebabe"          # initiate_tag
        "00010000"          # a_rwnd
        "000a"              # num_outbound_streams
        "000a"              # num_inbound_streams
        "00000001"          # initial_tsn
        "00050008"          # IPv4 param: type=5, length=8
        "0a000001"          # IPv4 addr 10.0.0.1
    )


def make_sack_packet():
    """SCTP packet with a single SACK chunk.

    Common header: standard fields
    SACK chunk:
      chunk_type = 0x03
      chunk_length = 0x001C (28 = 4 + 12 fixed + 8 gap_blocks + 4 dup_tsns)
      cum_tsn_ack = 0x00000100
      a_rwnd = 0x00020000
      num_gap_blocks = 0x0002
      num_dup_tsns = 0x0001
      gap_blocks: (1,2), (5,6) -> 8 bytes
      dup_tsns: [0x00000200] -> 4 bytes
    """
    return bytes.fromhex(
        "115c15b3"          # src_port + dst_port
        "11112222"          # verify_tag
        "00000000"          # checksum
        "001c"              # chunk_length = 28
        "03"                # chunk_type = 3 (SACK)
        "00"                # flags
        "00000100"          # cum_tsn_ack
        "00020000"          # a_rwnd
        "0002"              # num_gap_blocks
        "0001"              # num_dup_tsns
        "00010002"          # gap_block 1: start=1, end=2
        "00050006"          # gap_block 2: start=5, end=6
        "00000200"          # dup_tsn
    )


def make_multi_chunk_packet():
    """SCTP packet with two DATA chunks (GreedyRange).

    Chunk 1: user_data = "chunk1" (6 bytes), chunk_length = 22 (0x16)
    Chunk 2: user_data = "chunk2-data-longer" (18 bytes), chunk_length = 34 (0x22)
    """
    return bytes.fromhex(
        "22b8270f"          # src + dst
        "55556666"          # verify_tag
        "00000000"          # checksum
        "0016"              # chunk1 length = 22
        "00"                # chunk_type = 0 (DATA)
        "03"                # flags
        "00000001"          # tsn = 1
        "0000"              # stream_id
        "0000"              # stream_seq
        "00000000"          # ppid
        "6368756e6b31"      # "chunk1"
        "0022"              # chunk2 length = 34
        "00"                # chunk_type = 0 (DATA)
        "03"                # flags
        "00000002"          # tsn = 2
        "0000"              # stream_id
        "0001"              # stream_seq
        "00000000"          # ppid
        "6368756e6b322d646174612d6c6f6e676572"  # "chunk2-data-longer"
    )


REFERENCE_PACKETS = {
    "DATA": make_data_packet(),
    "INIT": make_init_packet(),
    "SACK": make_sack_packet(),
    "MULTI": make_multi_chunk_packet(),
}


# ---------------------------------------------------------------------------
# JSON serializable form (shared between crs and py subprocesses)
# ---------------------------------------------------------------------------
def normalize_for_json(obj):
    """Recursively normalize parsed object for JSON serialization.

    Handles:
      - dataclass instances (construct-rs): use __dict__
      - construct Container (Python construct): use dict() but skip _* keys
      - bytes -> hex string
      - list/tuple -> recursively normalize
    """
    # Bytes -> hex string
    if isinstance(obj, (bytes, bytearray)):
        return {"__bytes_hex__": obj.hex()}

    # List/tuple
    if isinstance(obj, (list, tuple)):
        return [normalize_for_json(x) for x in obj]

    # Dict (Python construct Container is dict-like)
    if isinstance(obj, dict):
        return {
            k: normalize_for_json(v)
            for k, v in obj.items()
            if not str(k).startswith("_")  # skip _io, _index, etc.
        }

    # construct-rs dataclass: use __dict__ but skip private
    if hasattr(obj, "__dict__") and not isinstance(obj, type):
        d = {
            k: v for k, v in vars(obj).items()
            if not k.startswith("_")
        }
        if d:
            return normalize_for_json(d)

    # Plain int/str/None/bool
    if isinstance(obj, (int, str, bool)) or obj is None:
        return obj

    # Fallback: try str
    return str(obj)


# ---------------------------------------------------------------------------
# Subprocess scripts (run inside respective venvs)
# ---------------------------------------------------------------------------
CRS_RUNNER = r'''
import json, sys
sys.path.insert(0, {here!r})
from sctp_crs import SCTPPacket

def normalize(obj):
    if isinstance(obj, (bytes, bytearray)):
        return {{"__bytes_hex__": obj.hex()}}
    if isinstance(obj, list):
        return [normalize(x) for x in obj]
    if hasattr(obj, "__dict__") and not isinstance(obj, type):
        d = {{k: v for k, v in vars(obj).items() if not k.startswith("_")}}
        if d:
            return normalize(d)
    if isinstance(obj, dict):
        return {{k: normalize(v) for k, v in obj.items() if not str(k).startswith("_")}}
    return obj

packets = json.loads(sys.stdin.read())
results = {{"parse": {{}}, "build": {{}}}}
for name, hexstr in packets.items():
    raw = bytes.fromhex(hexstr)
    parsed = SCTPPacket.parse(raw)
    results["parse"][name] = normalize(parsed)
    rebuilt = parsed.build()
    results["build"][name] = rebuilt.hex()

print(json.dumps(results))
'''.format(here=HERE)


PY_RUNNER = r'''
import json, sys
sys.path.insert(0, {here!r})
from sctp_parity import sctp_packet

def normalize(obj):
    if isinstance(obj, (bytes, bytearray)):
        return {{"__bytes_hex__": obj.hex()}}
    if isinstance(obj, list):
        return [normalize(x) for x in obj]
    if isinstance(obj, dict):
        return {{k: normalize(v) for k, v in obj.items() if not str(k).startswith("_")}}
    return obj

packets = json.loads(sys.stdin.read())
results = {{"parse": {{}}, "build": {{}}}}
for name, hexstr in packets.items():
    raw = bytes.fromhex(hexstr)
    parsed = sctp_packet.parse(raw)
    results["parse"][name] = normalize(parsed)
    rebuilt = sctp_packet.build(parsed)
    results["build"][name] = rebuilt.hex()

print(json.dumps(results))
'''.format(here=HERE)


def run_in_venv(python_exe, script, input_json):
    """Run script in given venv, pass input_json via stdin, capture stdout."""
    proc = subprocess.run(
        [python_exe, "-c", script],
        input=input_json,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        print("STDERR:", proc.stderr)
        raise RuntimeError(f"subprocess failed: {proc.returncode}")
    return proc.stdout.strip()


def main():
    print("=== Parity: construct-rs vs Python construct 2.10.70 ===\n")

    crs_python = r"<legacy-repo>\construct-rs\.venv\Scripts\python.exe"
    py_python = r"<opencode-temp>\py_parity\venv\Scripts\python.exe"

    # Prepare input
    packets_hex = {name: raw.hex() for name, raw in REFERENCE_PACKETS.items()}
    input_json = json.dumps(packets_hex)
    print(f"Reference packets: {list(packets_hex.keys())}")
    print()

    # Run both subprocesses
    print("[*] Running construct-rs subprocess...")
    crs_out = run_in_venv(crs_python, CRS_RUNNER, input_json)
    crs_results = json.loads(crs_out)

    print("[*] Running Python construct 2.10.70 subprocess...")
    py_out = run_in_venv(py_python, PY_RUNNER, input_json)
    py_results = json.loads(py_out)

    # Compare parse results
    print("\n--- PARSE parity ---")
    parse_pass = 0
    parse_fail = 0
    for name in REFERENCE_PACKETS:
        crs_p = crs_results["parse"][name]
        py_p = py_results["parse"][name]
        if crs_p == py_p:
            parse_pass += 1
            print(f"  [PASS] {name}: parse output identical")
        else:
            parse_fail += 1
            print(f"  [FAIL] {name}: parse output differs")
            print(f"    crs: {json.dumps(crs_p)[:300]}")
            print(f"    py:  {json.dumps(py_p)[:300]}")

    # Compare build results (and verify both equal the original reference)
    print("\n--- BUILD parity ---")
    build_pass = 0
    build_fail = 0
    for name, raw in REFERENCE_PACKETS.items():
        crs_b = crs_results["build"][name]
        py_b = py_results["build"][name]
        expected = raw.hex()
        if crs_b == py_b == expected:
            build_pass += 1
            print(f"  [PASS] {name}: build identical AND matches reference")
        else:
            build_fail += 1
            print(f"  [FAIL] {name}: build differs")
            print(f"    expected: {expected}")
            print(f"    crs:      {crs_b}")
            print(f"    py:       {py_b}")

    print()
    print(f"=== Summary ===")
    print(f"  Parse: {parse_pass} passed, {parse_fail} failed")
    print(f"  Build: {build_pass} passed, {build_fail} failed")
    if parse_fail > 0 or build_fail > 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
