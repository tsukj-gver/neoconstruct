"""Phase 8.5 HashAlgo Python enum。

设计依据：``docs/design/模块设计/模块设计-Phase8-P0.md`` §3.2.2。

construct-rs 扩展的内置哈希算法 enum。Rust 端在 compile.rs 通过
``type(hashfunc).__name__ == "HashAlgo"`` 识别，按 ``.name`` 取算法名编译为
``BuiltinHash`` 变体（路径 B 零拷贝）。

与 Python ``hashlib`` / ``zlib`` 的等价关系：

- ``MD5`` ↔ ``hashlib.md5(data).digest()``
- ``SHA1`` ↔ ``hashlib.sha1(data).digest()``
- ``SHA256`` ↔ ``hashlib.sha256(data).digest()``
- ``SHA512`` ↔ ``hashlib.sha512(data).digest()``
- ``CRC32`` ↔ ``zlib.crc32(data).to_bytes(4, 'big')`` （4 字节 big-endian）
- ``ADLER32`` ↔ ``zlib.adler32(data).to_bytes(4, 'big')``
"""

from enum import Enum, auto


class HashAlgo(Enum):
    """construct-rs 扩展的内置哈希算法 enum。

    用于 ``Checksum`` 构造器，启用 Rust 内置 hashfunc 零拷贝路径
    （搭配 ``StreamRange`` bytes_source 时全程零拷贝，详见 L-14 教训）。

    使用方式（搭配 StreamRange）::

        from construct import (
            StructMixin, Bytes, Tell, Checksum, HashAlgo, rfield, field
        )
        from dataclasses import dataclass

        @dataclass
        class Packet(StructMixin):
            start: int = rfield(Tell())
            data: bytes = field(Bytes(16))
            end: int = rfield(Tell())
            checksum: bytes = rfield(Checksum(Bytes(32), HashAlgo.SHA256, start, end))

    与 Python callable 的差异：

    - ``HashAlgo.SHA256`` → Rust 内置 sha2 crate 计算（零拷贝，B1 路径）
    - ``lambda d: hashlib.sha256(d).digest()`` → Python callable（FFI 回调，A1 路径）

    CRC32 / ADLER32 注意事项：

    Rust 端返回 4 字节 big-endian（对齐 ``zlib.crc32(d).to_bytes(4, 'big')`` 习惯）。
    用户若用 little-endian，需在 Python 端手动转换。
    """

    MD5 = auto()
    SHA1 = auto()
    SHA256 = auto()
    SHA512 = auto()
    CRC32 = auto()
    ADLER32 = auto()


__all__ = [
    "HashAlgo",
]
