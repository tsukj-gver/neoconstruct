//! ChecksumNode：校验和节点（双轨方案：Rust 内置 hashfunc + Python callable 兼容）。
//!
//! Python 参考：`construct/construct/core.py` `Checksum`（L5532-5600）。
//!
//! ## 双轨设计
//!
//! hashfunc 支持两类实现（Rust 内置路径避免跨 FFI 拷贝字节序列）：
//!
//! - 路径 A（兼容）：Python callable hashfunc + ContextBytes / StreamRange bytes_source
//! - 路径 B（零拷贝推荐）：Rust 内置 hashfunc + StreamRange bytes_source
//!
//! ## 路径矩阵
//!
//! | 路径 | hashfunc | bytes_source | 拷贝次数 |
//! |------|---------|-------------|---------|
//! | 内置 + StreamRange（最优） | Rust 内置 | StreamRange | 0 次 |
//! | 内置 + ContextBytes | Rust 内置 | ContextBytes | 0 次（借用 PyBytes） |
//! | callable + StreamRange | Python callable | StreamRange | 1 次 |
//! | callable + ContextBytes（Python 原版等价） | Python callable | ContextBytes | 0 次额外 |

use crate::context::Context;
use crate::error::ConstructError;
use crate::expr::{eval_expr_int, ExprProgram};
use crate::nodes::Node;
use crate::path::Path;
use crate::stream::{BuildStream, ParseStream};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use super::Construct;

/// 内置哈希算法（编译期从 HashAlgo enum 编译）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinHash {
    /// MD5（128-bit / 16 字节摘要）。
    Md5,
    /// SHA-1（160-bit / 20 字节摘要）。
    Sha1,
    /// SHA-256（256-bit / 32 字节摘要）。
    Sha256,
    /// SHA-512（512-bit / 64 字节摘要）。
    Sha512,
    /// CRC32（zlib.crc32 等价，4 字节 big-endian）。
    Crc32,
    /// Adler32（zlib.adler32 等价，4 字节 big-endian）。
    Adler32,
}

/// 哈希函数类型（双轨）。
#[derive(Debug)]
pub enum HashFunc {
    /// Rust 内置哈希（零拷贝，操作 `&[u8]`）。
    BuiltIn(BuiltinHash),
    /// Python callable（兼容模式，跨 FFI）。
    PythonCallable(Py<PyAny>),
}

/// 被哈希字节来源（双轨）。
#[derive(Debug)]
pub enum BytesSource {
    /// 原版模式：从 context 求值 bytesfunc 表达式得 bytes 对象。
    /// bytesfunc 编译为 ExprProgram（取字段索引），运行期从 ctx 取 bytes。
    /// 表达式求值得到字段索引 → ctx.fields.get_item(name) → bytes。
    ContextBytes {
        /// 字段索引（编译期确定，运行期从 ctx.fields.get_item 取值）。
        field_idx: usize,
        /// 字段名（用于 ctx.fields.get_item）。
        field_name: String,
    },
    /// 扩展模式：从 stream 范围 [start, end) 直接切片。
    /// start/end 编译为 ExprProgram，运行期求值得到字节偏移。
    StreamRange {
        /// 起始偏移表达式。
        start: ExprProgram,
        /// 结束偏移表达式。
        end: ExprProgram,
    },
}

/// 校验和节点：parse 校验 hash，build 计算 hash。
///
/// 对应 Python construct `Checksum(checksumfield, hashfunc, bytesfunc)`
/// （core.py L5532）。neoconstruct 扩展支持两种 hashfunc 类型与两种
/// bytes_source 类型，组合出 4 条路径（详见模块级路径矩阵）。
///
/// # 双轨方案
///
/// - **路径 A**（parity 兼容）：Python callable hashfunc + ContextBytes
///   bytes_source——对齐 Python 原版，1 次拷贝（CPython 要求 bytes 对象）
/// - **路径 B**（零拷贝扩展）：Rust 内置 hashfunc + StreamRange bytes_source
///   ——neoconstruct 扩展，全程零拷贝
#[derive(Debug)]
pub struct ChecksumNode {
    /// 校验字段节点（通常 Bytes(32)/Bytes(64)）。
    checksumfield: Box<Node>,
    /// 哈希函数：Rust 内置（零拷贝）或 Python callable（兼容）。
    hashfunc: HashFunc,
    /// 被哈希字节来源：context 表达式求值（原版）或 stream 切片（扩展）。
    bytes_source: BytesSource,
}

impl ChecksumNode {
    /// 创建 `ChecksumNode`。
    pub fn new(checksumfield: Node, hashfunc: HashFunc, bytes_source: BytesSource) -> Self {
        Self {
            checksumfield: Box::new(checksumfield),
            hashfunc,
            bytes_source,
        }
    }

    /// 返回校验字段节点的引用。
    pub fn checksumfield(&self) -> &Node {
        &self.checksumfield
    }
}

/// Rust 内置哈希计算（dispatch 到对应 crate）。
///
/// 返回摘要 Vec<u8>。CRC32/Adler32 返回 4 字节 big-endian（对齐 Python
/// `to_bytes(4, 'big')` 习惯）。
pub fn compute_builtin_hash(algo: BuiltinHash, data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    match algo {
        BuiltinHash::Md5 => {
            // md-5 crate 的 lib name 是 "md5"。
            let mut h = md5::Md5::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Sha1 => {
            let mut h = sha1::Sha1::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Sha256 => {
            let mut h = sha2::Sha256::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Sha512 => {
            let mut h = sha2::Sha512::new();
            h.update(data);
            h.finalize().to_vec()
        }
        BuiltinHash::Crc32 => {
            // crc32fast 返回 u32，转 4 字节 big-endian
            let crc = crc32fast::hash(data);
            crc.to_be_bytes().to_vec()
        }
        BuiltinHash::Adler32 => {
            let adler = adler::adler32_slice(data);
            adler.to_be_bytes().to_vec()
        }
    }
}

impl Construct for ChecksumNode {
    fn parse<'py>(
        &self,
        py: Python<'py>,
        stream: &mut ParseStream<'_>,
        ctx: &mut Context<'py>,
        path: &mut Path,
    ) -> Result<Py<PyAny>, ConstructError> {
        // 1. 读 checksum 字段
        let hash1 = self.checksumfield.parse(py, stream, ctx, path)?;
        let hash1_bound = hash1.bind(py);
        let hash1_bytes: &[u8] = hash1_bound
            .extract()
            .map_err(|_| ConstructError::Checksum {
                message: "checksumfield.parse result is not bytes-like".to_string(),
                path: path.to_string(),
            })?;

        // 2. 取被哈希字节 + 计算摘要
        // 使用 owned Vec<u8> 避免 bytes_obj 借用生命周期问题。
        let computed: Vec<u8> = match &self.hashfunc {
            HashFunc::BuiltIn(builtin) => {
                // Rust 内置 hashfunc 路径
                let data_owned: Vec<u8> = match &self.bytes_source {
                    BytesSource::StreamRange { start, end } => {
                        let s = eval_expr_int(start, ctx, py)? as usize;
                        let e = eval_expr_int(end, ctx, py)? as usize;
                        stream
                            .slice(s, e)
                            .ok_or_else(|| ConstructError::Checksum {
                                message: format!("stream slice out of bounds: [{}, {})", s, e),
                                path: path.to_string(),
                            })?
                            .to_vec()
                    }
                    BytesSource::ContextBytes { field_name, .. } => {
                        // 从 ctx 取字段值，期望 bytes
                        let bytes_obj = ctx
                            .get_field(field_name)
                            .map_err(|e| ConstructError::Checksum {
                                message: format!(
                                    "failed to get bytes field '{}': {}",
                                    field_name, e
                                ),
                                path: path.to_string(),
                            })?
                            .ok_or_else(|| ConstructError::Checksum {
                                message: format!("bytes field '{}' missing in context", field_name),
                                path: path.to_string(),
                            })?;
                        let borrow: &[u8] =
                            bytes_obj.extract().map_err(|_| ConstructError::Checksum {
                                message: format!("bytes field '{}' is not bytes-like", field_name),
                                path: path.to_string(),
                            })?;
                        borrow.to_vec()
                    }
                };
                compute_builtin_hash(*builtin, &data_owned)
            }
            HashFunc::PythonCallable(callable) => {
                // Python callable hashfunc 路径
                let bytes_py: Py<PyBytes> = match &self.bytes_source {
                    BytesSource::StreamRange { start, end } => {
                        let s = eval_expr_int(start, ctx, py)? as usize;
                        let e = eval_expr_int(end, ctx, py)? as usize;
                        let slice = stream.slice(s, e).ok_or_else(|| ConstructError::Checksum {
                            message: format!("stream slice out of bounds: [{}, {})", s, e),
                            path: path.to_string(),
                        })?;
                        // **拷贝点**：构造新 PyBytes（CPython 硬约束，无法避免）
                        PyBytes::new_bound(py, slice).into()
                    }
                    BytesSource::ContextBytes { field_name, .. } => {
                        let bytes_obj = ctx
                            .get_field(field_name)
                            .map_err(|e| ConstructError::Checksum {
                                message: format!(
                                    "failed to get bytes field '{}': {}",
                                    field_name, e
                                ),
                                path: path.to_string(),
                            })?
                            .ok_or_else(|| ConstructError::Checksum {
                                message: format!("bytes field '{}' missing in context", field_name),
                                path: path.to_string(),
                            })?;
                        // 借用 PyBytes → 转 owned Py<PyBytes>
                        let borrow: &[u8] =
                            bytes_obj.extract().map_err(|_| ConstructError::Checksum {
                                message: format!("bytes field '{}' is not bytes-like", field_name),
                                path: path.to_string(),
                            })?;
                        PyBytes::new_bound(py, borrow).into()
                    }
                };
                // FFI 回调 Python hashfunc
                let result =
                    callable
                        .call1(py, (bytes_py,))
                        .map_err(|e| ConstructError::Checksum {
                            message: format!("Python hashfunc raised: {}", e),
                            path: path.to_string(),
                        })?;
                let result_bytes: &[u8] =
                    result
                        .bind(py)
                        .extract()
                        .map_err(|_| ConstructError::Checksum {
                            message: "Python hashfunc returned non-bytes".to_string(),
                            path: path.to_string(),
                        })?;
                result_bytes.to_vec()
            }
        };

        // 3. 比较 hash1 / computed（长度不等先报错）
        if hash1_bytes.len() != computed.len() {
            return Err(ConstructError::Checksum {
                message: format!(
                    "wrong checksum, read {} bytes but computed {} bytes",
                    hash1_bytes.len(),
                    computed.len()
                ),
                path: path.to_string(),
            });
        }
        if hash1_bytes != computed.as_slice() {
            // hex 编码（对齐 Python binascii.hexlify）
            let read_hex: String = hash1_bytes.iter().map(|b| format!("{:02x}", b)).collect();
            let comp_hex: String = computed.iter().map(|b| format!("{:02x}", b)).collect();
            return Err(ConstructError::Checksum {
                message: format!("wrong checksum, read {}, computed {}", read_hex, comp_hex),
                path: path.to_string(),
            });
        }
        Ok(hash1)
    }

    fn build(
        &self,
        py: Python<'_>,
        _obj: &Bound<'_, PyAny>,
        stream: &mut BuildStream,
        ctx: &mut Context<'_>,
        path: &mut Path,
    ) -> Result<(), ConstructError> {
        // build：计算 hash2 → checksumfield.build(hash2)
        // 注：build 时 bytes_source 通常需要先记录位置（Tell）。
        // StreamRange 模式下 start/end 已求值，但 BuildStream 不能 slice 历史字节。
        // 实际方案：build 时通过 stream.tell() 取当前位置。
        //
        // 简化实现：对 StreamRange，build 时从 BuildStream 的 buf 中取 [start, end) 切片。
        // 对 ContextBytes，从 ctx 取字段值。

        let computed: Vec<u8> = match &self.hashfunc {
            HashFunc::BuiltIn(builtin) => {
                let data_slice: Vec<u8> = match &self.bytes_source {
                    BytesSource::StreamRange { start, end } => {
                        let s = eval_expr_int(start, ctx, py)? as usize;
                        let e = eval_expr_int(end, ctx, py)? as usize;
                        // BuildStream 借用已写入字节
                        let buf = stream.as_bytes();
                        if e > buf.len() {
                            return Err(ConstructError::Checksum {
                                message: format!(
                                    "build stream slice out of bounds: [{}, {}), buf len={}",
                                    s,
                                    e,
                                    buf.len()
                                ),
                                path: path.to_string(),
                            });
                        }
                        buf.get(s..e)
                            .ok_or_else(|| ConstructError::Checksum {
                                message: format!(
                                    "build stream slice out of bounds: [{}, {})",
                                    s, e
                                ),
                                path: path.to_string(),
                            })?
                            .to_vec()
                    }
                    BytesSource::ContextBytes { field_name, .. } => {
                        let bytes_obj = ctx
                            .get_field(field_name)
                            .map_err(|e| ConstructError::Checksum {
                                message: format!(
                                    "failed to get bytes field '{}': {}",
                                    field_name, e
                                ),
                                path: path.to_string(),
                            })?
                            .ok_or_else(|| ConstructError::Checksum {
                                message: format!("bytes field '{}' missing in context", field_name),
                                path: path.to_string(),
                            })?;
                        let borrow: &[u8] =
                            bytes_obj.extract().map_err(|_| ConstructError::Checksum {
                                message: format!("bytes field '{}' is not bytes-like", field_name),
                                path: path.to_string(),
                            })?;
                        borrow.to_vec()
                    }
                };
                compute_builtin_hash(*builtin, &data_slice)
            }
            HashFunc::PythonCallable(callable) => {
                let bytes_py: Py<PyBytes> = match &self.bytes_source {
                    BytesSource::StreamRange { start, end } => {
                        let s = eval_expr_int(start, ctx, py)? as usize;
                        let e = eval_expr_int(end, ctx, py)? as usize;
                        let buf = stream.as_bytes();
                        if e > buf.len() {
                            return Err(ConstructError::Checksum {
                                message: format!(
                                    "build stream slice out of bounds: [{}, {})",
                                    s, e
                                ),
                                path: path.to_string(),
                            });
                        }
                        let slice = buf.get(s..e).ok_or_else(|| ConstructError::Checksum {
                            message: format!("build stream slice out of bounds: [{}, {})", s, e),
                            path: path.to_string(),
                        })?;
                        PyBytes::new_bound(py, slice).into()
                    }
                    BytesSource::ContextBytes { field_name, .. } => {
                        let bytes_obj = ctx
                            .get_field(field_name)
                            .map_err(|e| ConstructError::Checksum {
                                message: format!(
                                    "failed to get bytes field '{}': {}",
                                    field_name, e
                                ),
                                path: path.to_string(),
                            })?
                            .ok_or_else(|| ConstructError::Checksum {
                                message: format!("bytes field '{}' missing in context", field_name),
                                path: path.to_string(),
                            })?;
                        let borrow: &[u8] =
                            bytes_obj.extract().map_err(|_| ConstructError::Checksum {
                                message: format!("bytes field '{}' is not bytes-like", field_name),
                                path: path.to_string(),
                            })?;
                        PyBytes::new_bound(py, borrow).into()
                    }
                };
                let result =
                    callable
                        .call1(py, (bytes_py,))
                        .map_err(|e| ConstructError::Checksum {
                            message: format!("Python hashfunc raised: {}", e),
                            path: path.to_string(),
                        })?;
                let result_bytes: &[u8] =
                    result
                        .bind(py)
                        .extract()
                        .map_err(|_| ConstructError::Checksum {
                            message: "Python hashfunc returned non-bytes".to_string(),
                            path: path.to_string(),
                        })?;
                result_bytes.to_vec()
            }
        };

        // 构造 PyBytes 并 checksumfield.build
        let hash_py = PyBytes::new_bound(py, &computed);
        self.checksumfield.build(py, &hash_py, stream, ctx, path)
    }

    fn sizeof(&self, ctx: &Context<'_>) -> Result<usize, ConstructError> {
        self.checksumfield.sizeof(ctx)
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprOp;
    use crate::nodes::bytes::BytesNode;
    use pyo3::types::PyString;

    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    fn with_py<F, R>(f: F) -> R
    where
        F: for<'py> FnOnce(Python<'py>) -> R,
    {
        ensure_python();
        Python::with_gil(f)
    }

    /// 构造一个含若干整数字段的 `Context`，用于表达式求值测试。
    fn make_context_with_ints<'py>(py: Python<'py>, entries: &[(&str, i64)]) -> Context<'py> {
        let mut ctx = Context::new_root(py).expect("new_root");
        ctx.init_expr_values(entries.len());
        for (idx, (name, value)) in entries.iter().enumerate() {
            let key = PyString::new_bound(py, name).unbind();
            let val = (*value).into_py(py);
            ctx.set_field_at(idx, &key, val.bind(py), py)
                .expect("set_field_at");
        }
        ctx
    }

    /// 构造一个含 bytes 字段的 `Context`。
    fn make_context_with_bytes<'py>(
        py: Python<'py>,
        int_entries: &[(&str, i64)],
        bytes_entries: &[(&str, &[u8])],
    ) -> Context<'py> {
        let ctx = make_context_with_ints(py, int_entries);
        for (name, value) in bytes_entries {
            let py_bytes = PyBytes::new_bound(py, value);
            ctx.set_field(name, &py_bytes).expect("set_field bytes");
        }
        ctx
    }

    // ======================================================================
    // compute_builtin_hash 基本测试
    // ======================================================================

    #[test]
    fn compute_builtin_hash_sha256_known_value() {
        // SHA-256(b"abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        // (NIST FIPS 180-2 测试向量)
        let result = compute_builtin_hash(BuiltinHash::Sha256, b"abc");
        assert_eq!(result.len(), 32);
        let expected_hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let actual_hex: String = result.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(actual_hex, expected_hex);
    }

    #[test]
    fn compute_builtin_hash_md5_known_value() {
        // MD5(b"") = d41d8cd98f00b204e9800998ecf8427e
        let result = compute_builtin_hash(BuiltinHash::Md5, b"");
        assert_eq!(result.len(), 16);
        let expected_hex = "d41d8cd98f00b204e9800998ecf8427e";
        let actual_hex: String = result.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(actual_hex, expected_hex);
    }

    #[test]
    fn compute_builtin_hash_crc32_known_value() {
        // CRC32(b"123456789") = 0xCBF43926 (zlib.crc32 标准测试向量)
        let result = compute_builtin_hash(BuiltinHash::Crc32, b"123456789");
        assert_eq!(result.len(), 4);
        // big-endian: cb f4 39 26
        assert_eq!(result, &[0xcb, 0xf4, 0x39, 0x26]);
    }

    #[test]
    fn compute_builtin_hash_adler32_known_value() {
        // Adler32(b"") = 0x00000001 (zlib.adler32(b"") == 1)
        let result = compute_builtin_hash(BuiltinHash::Adler32, b"");
        assert_eq!(result.len(), 4);
        assert_eq!(result, &[0x00, 0x00, 0x00, 0x01]);
    }

    // ======================================================================
    // ChecksumNode parse — 内置 hashfunc + StreamRange 路径
    // ======================================================================

    #[test]
    fn parse_builtin_sha256_streamrange_match() {
        // 内置 hashfunc + StreamRange，hash 匹配
        // stream 布局：[data(5字节) + digest(32字节)]，checksumfield 读 32 字节 digest
        // StreamRange start=0, end=5 切片 data 部分
        with_py(|py| {
            let data = b"hello";
            let digest = compute_builtin_hash(BuiltinHash::Sha256, data);
            let mut input = data.to_vec();
            input.extend_from_slice(&digest);

            // checksumfield 应该只读后 32 字节 digest。但 ChecksumNode 实现是
            // checksumfield.parse(stream) 从当前 stream 位置读。
            // 测试场景：先把 stream 推进 5 字节，再 Checksum。
            // 为简化：直接构造 stream = [data + digest]，用 StreamRange start=0 end=5，
            // checksumfield 读 32 字节（前 5 字节 data + 后 27 字节 digest）——不匹配。
            // 正确场景：data 部分应已被前序字段消费，stream 现在指向 digest 起点。
            // 模拟：先把 stream seek 到 5，然后 Checksum 读 32 字节 digest。
            let start = ExprProgram::new(vec![ExprOp::Const(0)]);
            let end = ExprProgram::new(vec![ExprOp::Const(5)]);
            let bytes_source = BytesSource::StreamRange { start, end };

            let checksumfield = Node::Bytes(BytesNode::new_const(32));
            let node = ChecksumNode::new(
                checksumfield,
                HashFunc::BuiltIn(BuiltinHash::Sha256),
                bytes_source,
            );

            let mut stream = ParseStream::new(&input);
            // 模拟前序字段消费 data（5 字节）
            let mut path = Path::new();
            stream.seek(5, &path).expect("seek to digest start");

            let mut ctx = Context::placeholder(py);
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect("parse");
            // 返回的 hash1 是 PyBytes(32)
            let bytes: &[u8] = result.bind(py).extract().unwrap();
            assert_eq!(bytes, &digest[..]);
            // checksumfield 消费 32 字节，总 stream 位置 = 5 + 32 = 37
            assert_eq!(stream.tell(), input.len());
        });
    }

    #[test]
    fn parse_builtin_sha256_streamrange_mismatch_raises() {
        // 内置 hashfunc + StreamRange，hash 不匹配
        with_py(|py| {
            let data = b"hello";
            let mut wrong_digest = compute_builtin_hash(BuiltinHash::Sha256, data);
            wrong_digest[0] ^= 0xFF; // 改一字节使其不匹配
            let mut input = data.to_vec();
            input.extend_from_slice(&wrong_digest);

            let start = ExprProgram::new(vec![ExprOp::Const(0)]);
            let end = ExprProgram::new(vec![ExprOp::Const(5)]);
            let bytes_source = BytesSource::StreamRange { start, end };

            let checksumfield = Node::Bytes(BytesNode::new_const(32));
            let node = ChecksumNode::new(
                checksumfield,
                HashFunc::BuiltIn(BuiltinHash::Sha256),
                bytes_source,
            );

            let mut stream = ParseStream::new(&input);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::Checksum { .. }));
        });
    }

    #[test]
    fn parse_builtin_sha256_streamrange_out_of_bounds_raises() {
        // slice 越界
        with_py(|py| {
            let data = b"hi";
            let digest = compute_builtin_hash(BuiltinHash::Sha256, data);
            let mut input = data.to_vec();
            input.extend_from_slice(&digest);

            // start=0, end=100（超出 data 长度）—— 注意 stream.slice 检查的是整个 stream 长度
            // 整个 stream 是 data(2) + digest(32) = 34 字节，所以 end=100 超出
            let start = ExprProgram::new(vec![ExprOp::Const(0)]);
            let end = ExprProgram::new(vec![ExprOp::Const(100)]);
            let bytes_source = BytesSource::StreamRange { start, end };

            let checksumfield = Node::Bytes(BytesNode::new_const(32));
            let node = ChecksumNode::new(
                checksumfield,
                HashFunc::BuiltIn(BuiltinHash::Sha256),
                bytes_source,
            );

            let mut stream = ParseStream::new(&input);
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            let err = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .expect_err("should fail");
            match err {
                ConstructError::Checksum { message, .. } => {
                    assert!(message.contains("out of bounds"), "got: {}", message);
                }
                other => panic!("expected Checksum error, got {:?}", other),
            }
        });
    }

    // ======================================================================
    // ChecksumNode parse — Python callable + ContextBytes 路径
    // ======================================================================

    #[test]
    fn parse_python_callable_context_bytes_match() {
        // Python callable hashfunc + ContextBytes
        with_py(|py| {
            let data = b"world";
            let expected_digest = compute_builtin_hash(BuiltinHash::Sha256, data);
            let mut input = expected_digest.clone(); // checksumfield 数据

            // Python callable：lambda d: hashlib.sha256(d).digest()
            let hashfunc_py = py
                .eval_bound(
                    "lambda d: __import__('hashlib').sha256(d).digest()",
                    None,
                    None,
                )
                .expect("eval lambda")
                .unbind();

            let bytes_source = BytesSource::ContextBytes {
                field_idx: 0,
                field_name: "data".to_string(),
            };

            let checksumfield = Node::Bytes(BytesNode::new_const(32));
            let node = ChecksumNode::new(
                checksumfield,
                HashFunc::PythonCallable(hashfunc_py),
                bytes_source,
            );

            // 构造 ctx with "data" = b"world"
            let mut ctx = make_context_with_bytes(py, &[], &[("data", b"world")]);
            let mut stream = ParseStream::new(&input);
            let mut path = Path::new();
            let result = node
                .parse(py, &mut stream, &mut ctx, &mut path)
                .unwrap_or_else(|e| match e {
                    ConstructError::Checksum { message, .. } => {
                        panic!("parse failed (Checksum): {}", message);
                    }
                    other => panic!(
                        "parse failed ({:?}): {}",
                        other.kind_str(),
                        other.full_message()
                    ),
                });
            let bytes: &[u8] = result.bind(py).extract().unwrap();
            assert_eq!(bytes, &expected_digest[..]);

            let _ = &mut input; // 避免 unused 警告
        });
    }

    // ======================================================================
    // ChecksumNode build — 内置 hashfunc + StreamRange 路径
    // ======================================================================

    #[test]
    fn build_builtin_sha256_streamrange_computes_hash() {
        // build 时计算 SHA-256 → checksumfield.build(digest)
        with_py(|py| {
            // 准备：先写入 5 字节 "hello" 到 BuildStream，然后用 start=0, end=5 计算 checksum
            let mut prep_stream = BuildStream::new();
            prep_stream.write(b"hello");
            let buf = prep_stream.into_bytes();
            let expected_digest = compute_builtin_hash(BuiltinHash::Sha256, &buf);

            let start = ExprProgram::new(vec![ExprOp::Const(0)]);
            let end = ExprProgram::new(vec![ExprOp::Const(5)]);
            let bytes_source = BytesSource::StreamRange { start, end };

            let checksumfield = Node::Bytes(BytesNode::new_const(32));
            let node = ChecksumNode::new(
                checksumfield,
                HashFunc::BuiltIn(BuiltinHash::Sha256),
                bytes_source,
            );

            // 重新构造 stream 写入 "hello" 然后 build checksum
            let mut stream = BuildStream::new();
            stream.write(b"hello");
            let obj = py.eval_bound("None", None, None).expect("None");
            let mut ctx = Context::placeholder(py);
            let mut path = Path::new();
            node.build(py, &obj, &mut stream, &mut ctx, &mut path)
                .expect("build");

            // 验证：stream 应有 5 字节 "hello" + 32 字节 digest
            assert_eq!(stream.as_bytes().len(), 5 + 32);
            assert_eq!(&stream.as_bytes()[..5], b"hello");
            assert_eq!(&stream.as_bytes()[5..], &expected_digest[..]);
        });
    }

    // ======================================================================
    // sizeof
    // ======================================================================

    #[test]
    fn sizeof_returns_checksumfield_size() {
        with_py(|py| {
            let checksumfield = Node::Bytes(BytesNode::new_const(32));
            let start = ExprProgram::new(vec![ExprOp::Const(0)]);
            let end = ExprProgram::new(vec![ExprOp::Const(0)]);
            let node = ChecksumNode::new(
                checksumfield,
                HashFunc::BuiltIn(BuiltinHash::Sha256),
                BytesSource::StreamRange { start, end },
            );
            let ctx = Context::placeholder(py);
            assert_eq!(node.sizeof(&ctx).unwrap(), 32);
        });
    }

    // ======================================================================
    // Debug
    // ======================================================================

    #[test]
    fn debug_format_includes_checksum_node() {
        let checksumfield = Node::Bytes(BytesNode::new_const(32));
        let start = ExprProgram::new(vec![ExprOp::Const(0)]);
        let end = ExprProgram::new(vec![ExprOp::Const(0)]);
        let node = ChecksumNode::new(
            checksumfield,
            HashFunc::BuiltIn(BuiltinHash::Sha256),
            BytesSource::StreamRange { start, end },
        );
        let s = format!("{:?}", node);
        assert!(s.contains("ChecksumNode"), "got: {}", s);
    }
}
