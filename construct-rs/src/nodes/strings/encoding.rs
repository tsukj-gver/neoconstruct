//! Strings 编码层：[`Encoding`] enum + decode/encode helper。
//!
//! # 设计摘要
//!
//! - **编码表示**：编译期 [`Encoding`] enum（6 变体），运行时零字符串匹配。
//! - **编解码实现**：混合方案
//!   - utf8/ascii 安全路径（std::str + pyo3 `PyString`）
//!   - utf16/32 raw FFI（unsafe CPython C API）
//! - **无后缀编码**：`from_user_str("utf16")` 等编译期 `Compilation` 错误，
//!   引导用户用 `utf_16_le`/`utf_16_be`。
//!
//! # unsafe 使用说明
//!
//! utf16/32 raw FFI 是首次在**正常路径**（非错误路径）使用 unsafe CPython C API。
//! 本文件每个 unsafe 块附 SAFETY 注释。

use crate::error::ConstructError;
use crate::path::Path;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use std::borrow::Cow;

/// Strings 支持的编码（封闭集合，对齐 Python `possiblestringencodings`）。
///
/// 编码在 Descriptor 编译期（`__init_subclass__`）从用户字符串解析为 enum 变体，
/// 运行时零字符串匹配开销。
///
/// 不在列表内的编码名 → [`ConstructError::Compilation`]（编译期失败，不进入运行时）。
///
/// # 无后缀编码
///
/// **不接受无后缀 `utf16`/`utf_16`/`u16`/`utf32`/`utf_32`/`u32`**。这些在 Python 中
/// 带"本机字节序 BOM"语义（不可移植），construct-rs 强制用户显式指定字节序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// ASCII（unit=1 字节）。Rust 标准库 + 严格 ASCII 校验（字节 < 128）。
    Ascii,
    /// UTF-8（unit=1 字节）。Rust 标准库 `std::str::from_utf8` + `PyString::new_bound`。
    Utf8,
    /// UTF-16 Little Endian（unit=2 字节）。CPython `PyUnicode_DecodeUTF16(byteorder=-1)`。
    Utf16Le,
    /// UTF-16 Big Endian（unit=2 字节）。CPython `PyUnicode_DecodeUTF16(byteorder=+1)`。
    Utf16Be,
    /// UTF-32 Little Endian（unit=4 字节）。CPython `PyUnicode_DecodeUTF32(byteorder=-1)`。
    Utf32Le,
    /// UTF-32 Big Endian（unit=4 字节）。CPython `PyUnicode_DecodeUTF32(byteorder=+1)`。
    Utf32Be,
}

impl Encoding {
    /// 从用户传入的字符串解析为 [`Encoding`]（编译期一次）。
    ///
    /// 对齐 Python `possiblestringencodings` 的**有后缀**编码别名集合
    /// （normalize：去 `-` 改 `_`、转小写）。
    ///
    /// # 无后缀编码
    ///
    /// **不接受无后缀 `utf16`/`utf_16`/`u16`/`utf32`/`utf_32`/`u32`**。这些在 Python 中
    /// 带"本机字节序 BOM"语义（x86 加 LE BOM，Sparc 加 BE BOM），跨平台不可移植。
    /// construct-rs 编译期拒绝，错误信息引导用户改用 `utf_16_le`/`utf_16_be`/
    /// `utf_32_le`/`utf_32_be`。
    ///
    /// # 错误引导文本
    ///
    /// 错误引导文本通过 `contains("16")` 判定 utf16 族 vs utf32 族，避免
    /// `utf_16` 别名（带下划线）被误导向 `utf_32_le`。
    ///
    /// # 参数
    ///
    /// - `s`：用户传入的编码名（如 `"utf8"` / `"UTF-16-LE"`）。
    ///
    /// # 错误
    ///
    /// - [`ConstructError::Compilation`]：未识别或无后缀编码名。Descriptor 应转为
    ///   用户面 `StringError`（保持异常类型对齐 Python）。
    pub fn from_user_str(s: &str) -> Result<Self, ConstructError> {
        let normalized: String = s.replace('-', "_").to_ascii_lowercase();
        match normalized.as_str() {
            "ascii" => Ok(Encoding::Ascii),
            // utf8/utf_8/u8 不含 BOM 语义（CPython str.encode("utf8") 不加 BOM），可安全接受。
            "utf8" | "utf_8" | "u8" => Ok(Encoding::Utf8),
            "utf_16_le" => Ok(Encoding::Utf16Le),
            "utf_16_be" => Ok(Encoding::Utf16Be),
            "utf_32_le" => Ok(Encoding::Utf32Le),
            "utf_32_be" => Ok(Encoding::Utf32Be),
            // 显式拒绝无后缀别名（utf16/utf_16/u16/utf32/utf_32/u32）。
            // 用 contains("16") 判定族别，避免 utf_16 被误导向 utf_32_le。
            "utf16" | "utf_16" | "u16" | "utf32" | "utf_32" | "u32" => {
                let is_utf16_family = normalized.contains('1');
                let (le_hint, be_hint) = if is_utf16_family {
                    ("utf_16_le", "utf_16_be")
                } else {
                    ("utf_32_le", "utf_32_be")
                };
                Err(ConstructError::Compilation {
                    message: format!(
                        "encoding {:?} is ambiguous (Python adds native-endian BOM, \
                         which is non-portable). construct-rs requires explicit byte order: \
                         use {:?} or {:?} instead.",
                        s, le_hint, be_hint,
                    ),
                })
            }
            _ => Err(ConstructError::Compilation {
                message: format!(
                    "encoding {:?} not found among supported encodings \
                     (ascii, utf8/utf_8/u8, utf_16_le, utf_16_be, utf_32_le, utf_32_be). \
                     Note: bare utf16/utf32 are rejected (use explicit _le/_be suffix).",
                    s
                ),
            }),
        }
    }

    /// 编码单元字节数（utf8/ascii=1, utf16=2, utf32=4）。
    ///
    /// 用于 term/pad 字节串的长度推导与多字节 rstrip 算法。
    pub const fn unit_size(&self) -> usize {
        match self {
            Encoding::Ascii | Encoding::Utf8 => 1,
            Encoding::Utf16Le | Encoding::Utf16Be => 2,
            Encoding::Utf32Le | Encoding::Utf32Be => 4,
        }
    }

    /// 默认 term 字节串（unit_size 个 0x00），用于 CString（无显式 term 参数时）。
    ///
    /// 对齐 Python `encodingunit(encoding)` 返回 `bytes(unit_size)`。
    pub fn default_term(&self) -> Vec<u8> {
        vec![0u8; self.unit_size()]
    }

    /// bytes → PyUnicode（parse 方向）。
    ///
    /// # 实现路径（混合方案）
    ///
    /// - Utf8 / Ascii：std::str::from_utf8 → PyString::new_bound（安全路径）
    /// - Utf16Le/Be / Utf32Le/Be：CPython `PyUnicode_DecodeUTF16/32` raw FFI
    ///
    /// # 错误
    ///
    /// 解码失败（非法字节序列、非 ASCII 字节）→ [`ConstructError::String`]，
    /// 对齐 Python `StringError("cannot use encoding ... to decode ...")`。
    pub fn decode<'py>(
        &self,
        py: Python<'py>,
        bytes: &[u8],
        path: &Path,
    ) -> Result<Bound<'py, PyString>, ConstructError> {
        match self {
            Encoding::Utf8 => match std::str::from_utf8(bytes) {
                Ok(s) => Ok(PyString::new_bound(py, s)),
                Err(_) => Err(ConstructError::String {
                    message: format!(
                        "cannot use encoding 'utf8' to decode {} bytes (invalid UTF-8 sequence)",
                        bytes.len()
                    ),
                    path: path.to_string(),
                }),
            },
            Encoding::Ascii => {
                // ASCII = UTF-8 子集；额外校验所有字节 < 128 对齐 Python ascii 编码严格性。
                if bytes.iter().all(|&b| b < 128) {
                    // 上面已校验全部 < 128，from_utf8 必成功。
                    let s = std::str::from_utf8(bytes).map_err(|_| ConstructError::String {
                        message: "ascii decode internal error".to_string(),
                        path: path.to_string(),
                    })?;
                    Ok(PyString::new_bound(py, s))
                } else {
                    Err(ConstructError::String {
                        message: "cannot use encoding 'ascii' to decode (byte >= 128 found)"
                            .to_string(),
                        path: path.to_string(),
                    })
                }
            }
            Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Utf32Le | Encoding::Utf32Be => {
                // CPython raw FFI。
                self.decode_utf16_32_raw(py, bytes, path)
            }
        }
    }

    /// PyUnicode → owned bytes（build 方向）。
    ///
    /// 返回 `Vec<u8>` 而非 `&[u8]`，因 PyUnicode 内部表示可能需转换分配。
    /// 调用方（各 String Node 的 build）随后 `stream.write(&vec)`。
    ///
    /// # encode 分支策略
    ///
    /// encode 按编码分支处理，避免 utf16/32 路径引入 Rust `Cow<str>` 中转层
    /// （保持"build 直接从 PyObject 读"）：
    /// - utf8/ascii（安全路径）：`to_cow()` 转 Rust `Cow<str>`。此路径下 ASCII 主导，
    ///   PyString 内部表示多为 ASCII compact（`to_cow` 返回 `Borrowed`，零分配）。
    /// - utf16/32（raw FFI 路径）：**直接持有原始 `&Bound<PyString>`** 调 CPython
    ///   `PyUnicode_AsUTF16String`/`AsUTF32String`，不经 `Cow<str>`。
    ///
    /// # 输入校验
    ///
    /// 非 `str` 类型 → [`ConstructError::String`]（对齐 Python
    /// `StringError("expected unicode string")`）。
    pub fn encode<'py>(
        &self,
        py: Python<'py>,
        obj: &Bound<'py, PyAny>,
        path: &Path,
    ) -> Result<Vec<u8>, ConstructError> {
        match self {
            Encoding::Utf8 | Encoding::Ascii => {
                // 安全路径：to_cow 在 utf8/ascii 场景可接受。
                let py_str = obj
                    .downcast::<PyString>()
                    .map_err(|_| ConstructError::String {
                        message: format!(
                            "string encoding failed, expected unicode string, got {}",
                            type_name_or_unknown(obj)
                        ),
                        path: path.to_string(),
                    })?;
                let s: Cow<'_, str> = py_str.to_cow().map_err(|e| ConstructError::String {
                    message: format!("string encoding failed: {}", e),
                    path: path.to_string(),
                })?;
                match self {
                    Encoding::Utf8 => Ok(s.into_owned().into_bytes()),
                    Encoding::Ascii => {
                        // 严格 ASCII：所有字节 < 128（对齐 Python ascii 编码严格性）。
                        if s.bytes().all(|b| b < 128) {
                            Ok(s.into_owned().into_bytes())
                        } else {
                            Err(ConstructError::String {
                                message: "cannot use encoding 'ascii' to encode (non-ASCII char)"
                                    .to_string(),
                                path: path.to_string(),
                            })
                        }
                    }
                    _ => unreachable!("Utf8/Ascii branch only"),
                }
            }
            Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Utf32Le | Encoding::Utf32Be => {
                // raw FFI 路径：直接 downcast，不调用 to_cow。
                let py_str = obj
                    .downcast::<PyString>()
                    .map_err(|_| ConstructError::String {
                        message: format!(
                            "string encoding failed, expected unicode string, got {}",
                            type_name_or_unknown(obj)
                        ),
                        path: path.to_string(),
                    })?;
                // 直接传原始 PyString 给 raw FFI（不经 Cow<str>）。
                self.encode_utf16_32_raw(py, py_str, path)
            }
        }
    }

    /// UTF-16/32 解码（CPython raw FFI）。
    ///
    /// # byteorder 取值
    ///
    /// `byteorder` 取值：
    /// - LE → **-1**（强制 LE，不扫描 BOM）
    /// - BE → **+1**（强制 BE，不扫描 BOM）
    ///
    /// CPython `PyUnicode_DecodeUTF16(s, size, errors, &byteorder)` byteorder 语义：
    /// | 值 | 含义 |
    /// |----|------|
    /// | `0` | 自动检测 BOM |
    /// | `-1` | 强制 LE，不检测 BOM |
    /// | `+1` | 强制 BE，不检测 BOM |
    ///
    /// 用户传 `utf_16_le`/`utf_16_be` 期望"纯 LE/BE 无 BOM 语义"，必须用 ±1 而非 0。
    /// 若用 0，数据流开头恰好是 `\xff\xfe`（LE BOM 字节序列）会被误判
    /// 为 BOM 消费掉，破坏 parity（Python `bytes.decode("utf_16_le")` 不消费 BOM）。
    ///
    /// # SAFETY
    ///
    /// unsafe 前置条件：
    ///
    /// 1. **GIL 持有**：`py: Python<'py>` token 在作用域内（FFI 入口已持 GIL）。
    /// 2. **`bytes.as_ptr()` 有效**：来自 `&[u8]` 切片，生命周期覆盖整个 unsafe 块。
    /// 3. **`bytes.len()` 是字节数**：CPython 内部按 unit_size 分组（UTF-16 按 2 字节、
    ///    UTF-32 按 4 字节）；尾部不完整单元 CPython 会拒绝并设置 `PyErr`。
    /// 4. **`&byteorder` 指针**：仅在本函数栈上，CPython 调用结束后不再读取
    ///    （CPython 内部可能修改 byteorder 值，但本函数不依赖调用后的值）。
    /// 5. **返回值所有权**：非 NULL 时所有权转移给 `Bound::from_owned_ptr`；
    ///    NULL 时 `PyErr` 已设置，由 `PyErr::fetch` 取回并转换为
    ///    [`ConstructError::String`]（回退路径）。
    fn decode_utf16_32_raw<'py>(
        &self,
        py: Python<'py>,
        bytes: &[u8],
        path: &Path,
    ) -> Result<Bound<'py, PyString>, ConstructError> {
        // byteorder = -1（LE）或 +1（BE），强制序不扫描 BOM。
        let mut byteorder: std::os::raw::c_int = match self {
            Encoding::Utf16Le | Encoding::Utf32Le => -1,
            Encoding::Utf16Be | Encoding::Utf32Be => 1,
            // 调用方 decode() 已匹配 utf16/32 分支，此处不可达。
            _ => unreachable!("decode_utf16_32_raw only for utf16/32"),
        };
        let encoding_name = match self {
            Encoding::Utf16Le => "utf_16_le",
            Encoding::Utf16Be => "utf_16_be",
            Encoding::Utf32Le => "utf_32_le",
            Encoding::Utf32Be => "utf_32_be",
            _ => unreachable!(),
        };
        let ptr = unsafe {
            // SAFETY:
            // 1. GIL 持有（py token 在作用域内）。
            // 2. bytes.as_ptr() 指向有效内存（&[u8] 切片，生命周期覆盖本块）。
            // 3. bytes.len() 为字节数，CPython 按 unit_size 分组，尾部不完整会设 PyErr。
            // 4. &byteorder 是栈上 mut c_int 指针，CPython 内部 cast away const 修改后不再用。
            // 5. 失败返回 NULL + PyErr 已设置（下方 fetch 转换）。
            if matches!(self, Encoding::Utf16Le | Encoding::Utf16Be) {
                ffi::PyUnicode_DecodeUTF16(
                    bytes.as_ptr() as *const std::os::raw::c_char,
                    bytes.len() as isize,
                    std::ptr::null(),
                    &mut byteorder as *mut std::os::raw::c_int,
                )
            } else {
                ffi::PyUnicode_DecodeUTF32(
                    bytes.as_ptr() as *const std::os::raw::c_char,
                    bytes.len() as isize,
                    std::ptr::null(),
                    &mut byteorder as *mut std::os::raw::c_int,
                )
            }
        };
        if ptr.is_null() {
            // CPython 设置 PyErr（如非法代理对、尾部不完整单元），fetch 取回。
            let py_err = PyErr::fetch(py);
            return Err(ConstructError::String {
                message: format!(
                    "cannot use encoding {:?} to decode {} bytes: {}",
                    encoding_name,
                    bytes.len(),
                    py_err
                ),
                path: path.to_string(),
            });
        }
        // ptr 是有效 PyUnicode 对象（Python 3 中 PyUnicode == str）。
        // Bound::from_owned_ptr 消费指针（不 incref），所有权转移给 Bound<PyAny>。
        let obj = unsafe {
            // SAFETY:
            // 1. GIL 持有。
            // 2. ptr 非 NULL（上面已检查），是 CPython PyUnicode_Decode* 返回的新引用。
            // 3. from_owned_ptr 消费指针（不 incref），转 owned Bound<PyAny>。
            Bound::<PyAny>::from_owned_ptr(py, ptr)
        };
        // PyUnicode_DecodeUTF16/32 必定返回 str（Python 3 中 PyUnicode 即 str）。
        let py_str: Bound<'py, PyString> =
            obj.downcast_into::<PyString>()
                .map_err(|_| ConstructError::String {
                    message: format!(
                        "encoding {:?} decode returned non-str (internal error)",
                        encoding_name
                    ),
                    path: path.to_string(),
                })?;
        Ok(py_str)
    }

    /// UTF-16/32 编码（CPython raw FFI）。
    ///
    /// # 签名说明
    ///
    /// 签名直接接收 `&Bound<PyString>`，不经 Rust `Cow<str>` 中转。
    ///
    /// # 实现要点
    ///
    /// CPython `PyUnicode_AsUTF16String`/`AsUTF32String` 输出**本机字节序 + BOM 前缀**：
    /// - UTF-16：LE 平台返回 `b'\xff\xfe' + payload`，BE 平台返回 `b'\xfe\xff' + payload`
    /// - UTF-32：LE 平台返回 `b'\xff\xfe\x00\x00' + payload`，BE 平台返回 `b'\x00\x00\xfe\xff' + payload`
    ///
    /// 而 Python `str.encode("utf_16_le")`/`"utf_32_le"` **不加 BOM**（强制序）。
    /// 因此本函数：
    /// 1. 调 `AsUTF16/32String` 得到含 BOM 的本机序字节串
    /// 2. 剥离前 `unit_size` 字节 BOM
    /// 3. 若目标序 ≠ 本机序，按 unit_size 分组 swap
    ///
    /// # SAFETY
    ///
    /// 1. **GIL 持有**：`py: Python<'py>` token。
    /// 2. **`obj.as_ptr()` 有效**：来自 downcast 后的 `&Bound<PyString>`，是有效 PyUnicode。
    /// 3. **返回 PyBytes 非 NULL 时所有权转移**给 `Bound::from_owned_ptr`；NULL 时
    ///    `PyErr::fetch` 取回转 [`ConstructError::String`]（回退路径）。
    fn encode_utf16_32_raw<'py>(
        &self,
        py: Python<'py>,
        obj: &Bound<'py, PyString>,
        path: &Path,
    ) -> Result<Vec<u8>, ConstructError> {
        let unit = self.unit_size();
        let encoding_name = match self {
            Encoding::Utf16Le => "utf_16_le",
            Encoding::Utf16Be => "utf_16_be",
            Encoding::Utf32Le => "utf_32_le",
            Encoding::Utf32Be => "utf_32_be",
            _ => unreachable!("encode_utf16_32_raw only for utf16/32"),
        };
        let ptr = unsafe {
            // SAFETY:
            // 1. GIL 持有（py token 在作用域内）。
            // 2. obj.as_ptr() 是有效 PyUnicode（来自 downcast 后的 Bound<PyString>）。
            // 3. 失败返回 NULL + PyErr 已设置（下方 fetch 转换）。
            if unit == 2 {
                ffi::PyUnicode_AsUTF16String(obj.as_ptr())
            } else {
                ffi::PyUnicode_AsUTF32String(obj.as_ptr())
            }
        };
        if ptr.is_null() {
            // 极少触发（编码 Unicode 字符串几乎不失败），fetch 取回异常。
            let py_err = PyErr::fetch(py);
            return Err(ConstructError::String {
                message: format!(
                    "cannot use encoding {:?} to encode unicode string: {}",
                    encoding_name, py_err
                ),
                path: path.to_string(),
            });
        }
        // 持有 PyBytes 所有权（含 BOM 前缀）。
        let py_bytes = unsafe {
            // SAFETY:
            // 1. GIL 持有。
            // 2. ptr 非 NULL（上面已检查），是 AsUTF16/32String 返回的新 PyBytes 引用。
            // 3. from_owned_ptr 消费指针，转 owned Bound<PyAny>。
            let any = Bound::<PyAny>::from_owned_ptr(py, ptr);
            // AsUTF16/32String 必定返回 bytes。
            any.downcast_into::<PyBytes>()
                .map_err(|_| ConstructError::String {
                    message: format!(
                        "encoding {:?} encode returned non-bytes (internal error)",
                        encoding_name
                    ),
                    path: path.to_string(),
                })?
        };
        let all_bytes = py_bytes.as_bytes();
        // 剥离前 unit 字节 BOM（AsUTF16/32String 输出 = unit 字节 BOM + payload）。
        // 极端边界：空字符串编码 → all_bytes.len() == unit（仅 BOM）。
        if all_bytes.len() < unit {
            return Err(ConstructError::String {
                message: format!(
                    "encoding {:?} produced unexpectedly short output (len={})",
                    encoding_name,
                    all_bytes.len()
                ),
                path: path.to_string(),
            });
        }
        let payload = &all_bytes[unit..];

        // 本机序到目标序的 byte-swap（按 unit 分组 reverse）。
        // cfg!(target_endian) 是编译期常量，分支在编译期消除（无运行时开销）。
        let need_swap = match self {
            Encoding::Utf16Le | Encoding::Utf32Le => cfg!(target_endian = "big"),
            Encoding::Utf16Be | Encoding::Utf32Be => cfg!(target_endian = "little"),
            _ => unreachable!(),
        };
        if need_swap {
            let mut swapped = payload.to_vec();
            for chunk in swapped.chunks_mut(unit) {
                chunk.reverse();
            }
            Ok(swapped)
        } else {
            Ok(payload.to_vec())
        }
    }
}

/// 获取 `Bound<PyAny>` 的类型名（失败时返回 `"<unknown>"`）。
///
/// 用于错误信息中显示实际类型（对齐 Python
/// `f"expected unicode string, got {type(obj).__name__}"`）。
fn type_name_or_unknown(obj: &Bound<'_, PyAny>) -> String {
    obj.get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ConstructError;

    // ======================================================================
    // from_user_str：接受有后缀编码
    // ======================================================================

    #[test]
    fn from_user_str_ascii() {
        assert_eq!(Encoding::from_user_str("ascii").unwrap(), Encoding::Ascii);
    }

    #[test]
    fn from_user_str_utf8_aliases() {
        assert_eq!(Encoding::from_user_str("utf8").unwrap(), Encoding::Utf8);
        assert_eq!(Encoding::from_user_str("utf_8").unwrap(), Encoding::Utf8);
        assert_eq!(Encoding::from_user_str("u8").unwrap(), Encoding::Utf8);
    }

    #[test]
    fn from_user_str_utf16_le_be() {
        assert_eq!(
            Encoding::from_user_str("utf_16_le").unwrap(),
            Encoding::Utf16Le
        );
        assert_eq!(
            Encoding::from_user_str("utf_16_be").unwrap(),
            Encoding::Utf16Be
        );
    }

    #[test]
    fn from_user_str_utf32_le_be() {
        assert_eq!(
            Encoding::from_user_str("utf_32_le").unwrap(),
            Encoding::Utf32Le
        );
        assert_eq!(
            Encoding::from_user_str("utf_32_be").unwrap(),
            Encoding::Utf32Be
        );
    }

    #[test]
    fn from_user_str_normalizes_dash_and_case() {
        // 用户传 "UTF-16-LE"（大写 + 短横线）→ normalize 为 utf_16_le
        assert_eq!(
            Encoding::from_user_str("UTF-16-LE").unwrap(),
            Encoding::Utf16Le
        );
        assert_eq!(
            Encoding::from_user_str("UTF-32-BE").unwrap(),
            Encoding::Utf32Be
        );
        assert_eq!(Encoding::from_user_str("ASCII").unwrap(), Encoding::Ascii);
    }

    // ======================================================================
    // 拒绝无后缀编码（utf16/utf_16/u16/utf32/utf_32/u32）
    // ======================================================================

    #[test]
    fn from_user_str_rejects_bare_utf16() {
        let err = Encoding::from_user_str("utf16").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
    }

    #[test]
    fn from_user_str_rejects_bare_utf_16_with_underscore() {
        let err = Encoding::from_user_str("utf_16").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
    }

    #[test]
    fn from_user_str_rejects_u16() {
        let err = Encoding::from_user_str("u16").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
    }

    #[test]
    fn from_user_str_rejects_bare_utf32() {
        let err = Encoding::from_user_str("utf32").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
    }

    #[test]
    fn from_user_str_rejects_bare_utf_32() {
        let err = Encoding::from_user_str("utf_32").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
    }

    #[test]
    fn from_user_str_rejects_u32() {
        let err = Encoding::from_user_str("u32").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
    }

    // ======================================================================
    // 错误引导文本不误导（utf16 族 → utf_16_le，不 → utf_32_le）
    // ======================================================================

    #[test]
    fn p8_utf16_alias_hinted_to_utf_16_le_not_utf_32() {
        // "utf16" 应引导到 utf_16_le / utf_16_be，不能误导向 utf_32_le
        let err = Encoding::from_user_str("utf16").expect_err("reject");
        if let ConstructError::Compilation { message } = err {
            assert!(
                message.contains("utf_16_le"),
                "utf16 引导应含 utf_16_le，got: {}",
                message
            );
            assert!(
                message.contains("utf_16_be"),
                "utf16 引导应含 utf_16_be，got: {}",
                message
            );
            assert!(
                !message.contains("utf_32_le"),
                "utf16 引导不应含 utf_32_le（P8 修正），got: {}",
                message
            );
        } else {
            panic!("expected Compilation error");
        }
    }

    #[test]
    fn p8_utf_16_with_underscore_hinted_to_utf_16_le() {
        // 关键 case：utf_16（带下划线）含 '1'，应识别为 utf16 族
        let err = Encoding::from_user_str("utf_16").expect_err("reject");
        if let ConstructError::Compilation { message } = err {
            assert!(
                message.contains("utf_16_le"),
                "utf_16 引导应含 utf_16_le，got: {}",
                message
            );
            assert!(
                !message.contains("utf_32_le"),
                "utf_16 引导不应含 utf_32_le（P8 修正），got: {}",
                message
            );
        } else {
            panic!("expected Compilation error");
        }
    }

    #[test]
    fn p8_u16_hinted_to_utf_16_le() {
        let err = Encoding::from_user_str("u16").expect_err("reject");
        if let ConstructError::Compilation { message } = err {
            assert!(message.contains("utf_16_le"), "got: {}", message);
            assert!(!message.contains("utf_32_le"), "got: {}", message);
        } else {
            panic!("expected Compilation error");
        }
    }

    #[test]
    fn p8_utf32_alias_hinted_to_utf_32_le() {
        let err = Encoding::from_user_str("utf32").expect_err("reject");
        if let ConstructError::Compilation { message } = err {
            assert!(message.contains("utf_32_le"), "got: {}", message);
            assert!(message.contains("utf_32_be"), "got: {}", message);
            assert!(
                !message.contains("utf_16_le"),
                "utf32 不应误导向 utf_16_le，got: {}",
                message
            );
        } else {
            panic!("expected Compilation error");
        }
    }

    // ======================================================================
    // 未识别编码
    // ======================================================================

    #[test]
    fn from_user_str_rejects_unknown_encoding() {
        let err = Encoding::from_user_str("latin-1").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
        if let ConstructError::Compilation { message } = err {
            // 错误信息显示用户原始输入（"latin-1"），便于用户识别
            assert!(message.contains("latin-1"), "got: {}", message);
        }
    }

    #[test]
    fn from_user_str_rejects_empty() {
        let err = Encoding::from_user_str("").expect_err("should reject");
        assert!(matches!(err, ConstructError::Compilation { .. }));
    }

    // ======================================================================
    // unit_size / default_term
    // ======================================================================

    #[test]
    fn unit_size_returns_correct_bytes() {
        assert_eq!(Encoding::Ascii.unit_size(), 1);
        assert_eq!(Encoding::Utf8.unit_size(), 1);
        assert_eq!(Encoding::Utf16Le.unit_size(), 2);
        assert_eq!(Encoding::Utf16Be.unit_size(), 2);
        assert_eq!(Encoding::Utf32Le.unit_size(), 4);
        assert_eq!(Encoding::Utf32Be.unit_size(), 4);
    }

    #[test]
    fn default_term_returns_zero_filled_unit() {
        assert_eq!(Encoding::Utf8.default_term(), b"\x00");
        assert_eq!(Encoding::Utf16Le.default_term(), b"\x00\x00");
        assert_eq!(Encoding::Utf32Be.default_term(), b"\x00\x00\x00\x00");
    }

    // ======================================================================
    // decode/encode：utf8/ascii 安全路径
    // ======================================================================

    fn ensure_python() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(pyo3::prepare_freethreaded_python);
    }

    #[test]
    fn decode_utf8_valid_bytes() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let s = Encoding::Utf8.decode(py, b"hello", &path).expect("decode");
            assert_eq!(s.to_cow().unwrap().as_ref(), "hello");
        });
    }

    #[test]
    fn decode_utf8_invalid_bytes_returns_string_error() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let err = Encoding::Utf8
                .decode(py, &[0xFF, 0xFE], &path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }

    #[test]
    fn decode_ascii_valid() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let s = Encoding::Ascii.decode(py, b"hello", &path).expect("decode");
            assert_eq!(s.to_cow().unwrap().as_ref(), "hello");
        });
    }

    #[test]
    fn decode_ascii_high_byte_returns_string_error() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let err = Encoding::Ascii
                .decode(py, &[0x80], &path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }

    #[test]
    fn encode_utf8_str_returns_bytes() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("'hello'", None, None).expect("eval");
            let data = Encoding::Utf8.encode(py, &obj, &path).expect("encode");
            assert_eq!(data, b"hello");
        });
    }

    #[test]
    fn encode_ascii_non_ascii_char_returns_string_error() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("'é'", None, None).expect("eval");
            let err = Encoding::Ascii
                .encode(py, &obj, &path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }

    #[test]
    fn encode_non_str_returns_string_error() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("b'bytes'", None, None).expect("eval");
            let err = Encoding::Utf8
                .encode(py, &obj, &path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }

    // ======================================================================
    // decode/encode：utf16/32 raw FFI（验证 byteorder + BOM 剥离）
    // ======================================================================

    #[test]
    fn decode_utf16_le_basic() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            // "h" UTF-16 LE = b'h\x00'
            let s = Encoding::Utf16Le
                .decode(py, b"h\x00", &path)
                .expect("decode");
            assert_eq!(s.to_cow().unwrap().as_ref(), "h");
        });
    }

    #[test]
    fn decode_utf16_be_basic() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            // "h" UTF-16 BE = b'\x00h'
            let s = Encoding::Utf16Be
                .decode(py, b"\x00h", &path)
                .expect("decode");
            assert_eq!(s.to_cow().unwrap().as_ref(), "h");
        });
    }

    #[test]
    fn p2a_decode_utf16_le_does_not_consume_bom() {
        // byteorder=-1 不扫描 BOM。
        // 数据流开头恰好是 \xff\xfe（LE BOM 字节序列），应保留为数据字符 U+FEFF。
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let s = Encoding::Utf16Le
                .decode(py, b"\xff\xfeh\x00", &path)
                .expect("decode");
            let cow = s.to_cow().unwrap();
            let chars: Vec<char> = cow.chars().collect();
            assert_eq!(chars.len(), 2);
            assert_eq!(chars[0], '\u{FEFF}'); // BOM 字符作为数据保留
            assert_eq!(chars[1], 'h');
        });
    }

    #[test]
    fn decode_utf32_le_basic() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let s = Encoding::Utf32Le
                .decode(py, b"X\x00\x00\x00", &path)
                .expect("decode");
            assert_eq!(s.to_cow().unwrap().as_ref(), "X");
        });
    }

    #[test]
    fn encode_utf16_le_strips_bom() {
        // 验证 encode_utf16_32_raw 剥离 BOM
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("'h'", None, None).expect("eval");
            let data = Encoding::Utf16Le.encode(py, &obj, &path).expect("encode");
            // "h" UTF-16 LE = b'h\x00'（无 BOM）
            assert_eq!(data, b"h\x00");
        });
    }

    #[test]
    fn encode_utf16_be_byte_swap() {
        // 验证 encode 本机序到 BE 的 byte-swap
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("'h'", None, None).expect("eval");
            let data = Encoding::Utf16Be.encode(py, &obj, &path).expect("encode");
            // "h" UTF-16 BE = b'\x00h'（无 BOM）
            assert_eq!(data, b"\x00h");
        });
    }

    #[test]
    fn encode_utf32_le_strips_bom() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("'X'", None, None).expect("eval");
            let data = Encoding::Utf32Le.encode(py, &obj, &path).expect("encode");
            assert_eq!(data, b"X\x00\x00\x00");
        });
    }

    #[test]
    fn encode_decode_utf16_le_round_trip() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("'hello world'", None, None).expect("eval");
            let data = Encoding::Utf16Le.encode(py, &obj, &path).expect("encode");
            let s = Encoding::Utf16Le.decode(py, &data, &path).expect("decode");
            assert_eq!(s.to_cow().unwrap().as_ref(), "hello world");
        });
    }

    #[test]
    fn encode_decode_utf32_be_round_trip() {
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let obj = py.eval_bound("'hello world'", None, None).expect("eval");
            let data = Encoding::Utf32Be.encode(py, &obj, &path).expect("encode");
            let s = Encoding::Utf32Be.decode(py, &data, &path).expect("decode");
            assert_eq!(s.to_cow().unwrap().as_ref(), "hello world");
        });
    }

    #[test]
    fn decode_utf16_le_odd_byte_length_returns_string_error() {
        // UTF-16 必须 2 字节对齐，1 字节是尾部不完整单元
        ensure_python();
        Python::with_gil(|py| {
            let path = Path::new();
            let err = Encoding::Utf16Le
                .decode(py, b"h", &path)
                .expect_err("should fail");
            assert!(matches!(err, ConstructError::String { .. }));
        });
    }
}
