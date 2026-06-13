//! Stream operation and tunneling constructs — Bitwise, Pointer, Peek, RawCopy,
//! Prefixed, Transformed, Restreamed, Compressed, Checksum, ByteSwapped,
//! BitsSwapped, and LazyBound.
//!
//! These constructs manipulate the underlying byte stream in various ways:
//! seeking to absolute positions, wrapping sub-streams, transforming bytes,
//! compressing/decompressing, etc.
//!
//! | Construct | Purpose |
//! |-----------|---------|
//! | [`Bitwise`] | Wraps byte stream into a bit-level stream |
//! | [`Bytewise`] | Restores byte-level access inside a bit-level context |
//! | [`Pointer`] | Reads/writes at an absolute stream position |
//! | [`Peek`] | Parses without advancing the stream position |
//! | [`RawCopy`] | Returns both raw bytes and parsed value |
//! | [`Prefixed`] | Prefixes data with a length field |
//! | [`Transformed`] | Applies a byte-to-byte transformation (fixed-size) |
//! | [`Restreamed`] | Applies a chunk-wise byte transformation (variable-size) |
//! | [`Compressed`] | Compresses/decompresses using zlib/deflate (feature-gated) |
//! | [`Checksum`] | Validates a checksum field |
//! | [`ByteSwapped`] | Reverses byte order within a fixed-size subcon |
//! | [`BitsSwapped`] | Reverses bit order within each byte |
//! | [`LazyBound`] | Binds to a subcon at runtime (for recursive structures) |

use std::io::SeekFrom;

use indexmap::IndexMap;

use crate::binary;
use crate::core::context::Context;
use crate::core::error::{ConstructError, Result};
use crate::core::stream::{ByteStream, Stream};
use crate::core::Construct;
use crate::value::Value;

// ===========================================================================
// Bitwise
// ===========================================================================

/// Wraps a byte stream into a bit-level stream, then parses/builds `subcon`
/// within that context.
///
/// - **parse**: reads bytes from the stream, expands to bits, parses `subcon`
///   from the bit data, then writes back any remaining bits
/// - **build**: builds `subcon` into bits, then packs bits back into bytes
///   and writes to the stream
/// - **sizeof**: returns the subcon's size divided by 8 (rounded up)
///
/// The subcon must consume a multiple of 8 bits.
///
/// Corresponds to Python `Bitwise(subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::Bitwise;
/// use construct::constructs::bytes::Bytes;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Bitwise::new(Box::new(Bytes::new(8)));
/// // Bytes(8) in a Bitwise context reads 8 bits as individual byte values
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\xff").unwrap();
/// assert_eq!(parsed, Value::Bytes(vec![1, 1, 1, 1, 1, 1, 1, 1]));
/// ```
pub struct Bitwise {
    /// The inner construct operating on the bit-level stream.
    pub subcon: Box<dyn Construct>,
}

impl Bitwise {
    /// Creates a new `Bitwise` wrapper around `subcon`.
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        Bitwise { subcon }
    }
}

impl Construct for Bitwise {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // Read remaining bytes from the stream, convert to bits
        let data = stream.read_remaining()?;
        let bits = binary::bytes2bits(&data);
        // Create a sub-stream from the bits and parse subcon
        let mut sub_stream = ByteStream::new_read(&bits);
        let result = self.subcon.parse(&mut sub_stream, ctx)?;
        // Read back any remaining bits and convert to bytes (should be 0)
        let remaining_bits = sub_stream.read_remaining()?;
        let _padding_bits = remaining_bits.len();
        // In the Python version, remaining bits are discarded
        // We don't need to seek back since we consumed from the original stream
        Ok(result)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        // Build subcon into a sub-stream (bits)
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let bits = sub_stream.into_bytes();
        // Convert bits back to bytes
        let bytes = binary::bits2bytes(&bits)?;
        stream.write_bytes(&bytes)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let bits_size = self.subcon.sizeof(ctx)?;
        // Round up to whole bytes
        Ok((bits_size + 7) / 8)
    }
}

// ===========================================================================
// Bytewise
// ===========================================================================

/// Restores byte-level access inside a bit-level context.
///
/// Used inside a [`Bitwise`] context to wrap a subcon that operates at the
/// byte level. The conversion between bits and bytes is automatic.
///
/// - **parse**: reads bits, packs them into bytes, parses `subcon` from bytes
/// - **build**: builds `subcon` into bytes, expands to bits
/// - **sizeof**: returns subcon's byte size multiplied by 8
///
/// Corresponds to Python `Bytewise(subcon)`.
pub struct Bytewise {
    /// The inner construct operating on byte-level data.
    pub subcon: Box<dyn Construct>,
}

impl Bytewise {
    /// Creates a new `Bytewise` wrapper around `subcon`.
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        Bytewise { subcon }
    }
}

impl Construct for Bytewise {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // In a bit context, we read bits and convert to bytes
        let bits = stream.read_remaining()?;
        let bytes = binary::bits2bytes(&bits)?;
        let mut sub_stream = ByteStream::new_read(&bytes);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let bytes = sub_stream.into_bytes();
        let bits = binary::bytes2bits(&bytes);
        stream.write_bytes(&bits)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let byte_size = self.subcon.sizeof(ctx)?;
        Ok(byte_size * 8)
    }
}

// ===========================================================================
// Pointer
// ===========================================================================

/// Reads/writes at an absolute stream position, then seeks back.
///
/// - **parse**: seeks to `offset`, parses `subcon`, seeks back to the original
///   position
/// - **build**: seeks to `offset`, builds `subcon`, seeks back
/// - **sizeof**: returns 0 (the pointer itself consumes no bytes in the
///   sequential stream)
///
/// Corresponds to Python `Pointer(offset, subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::Pointer;
/// use construct::constructs::bytes::Bytes;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Pointer::new(8, Box::new(Bytes::new(1)));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"abcdefghijkl").unwrap();
/// assert_eq!(parsed, Value::Bytes(b"i".to_vec()));
/// ```
pub struct Pointer {
    /// The absolute position to seek to.
    pub offset: i64,
    /// The inner construct to parse/build at the target position.
    pub subcon: Box<dyn Construct>,
}

impl Pointer {
    /// Creates a new `Pointer` that reads/writes `subcon` at the given
    /// absolute `offset`.
    pub fn new(offset: i64, subcon: Box<dyn Construct>) -> Self {
        Pointer { offset, subcon }
    }
}

impl Construct for Pointer {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let fallback = stream.tell()?;
        if self.offset >= 0 {
            stream.seek(self.offset as u64)?;
        } else {
            // Negative offset: from end of stream
            stream.seek_from(SeekFrom::End(self.offset))?;
        }
        let parse_result = self.subcon.parse(stream, ctx);
        // Always seek back
        let seek_result = stream.seek(fallback);
        parse_result.and_then(|v| seek_result.map(|_| v))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let fallback = stream.tell()?;
        if self.offset >= 0 {
            stream.seek(self.offset as u64)?;
        } else {
            stream.seek_from(SeekFrom::End(self.offset))?;
        }
        let build_result = self.subcon.build(data, stream, ctx);
        // Always seek back
        let seek_result = stream.seek(fallback);
        build_result.and(seek_result)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }
}

// ===========================================================================
// Peek
// ===========================================================================

/// Parses without advancing the stream position.
///
/// - **parse**: remembers position, parses `subcon`, seeks back to the saved
///   position. If parsing fails with a non-fatal error, returns `None`.
/// - **build**: does nothing (returns the value unchanged)
/// - **sizeof**: returns 0
///
/// Used internally by `Union` to try parsing each member.
///
/// Corresponds to Python `Peek(subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::Peek;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Peek::new(Box::new(INT8UB));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x2a").unwrap();
/// assert_eq!(parsed, Value::UInt(42));
/// ```
pub struct Peek {
    /// The inner construct to peek-parse.
    pub subcon: Box<dyn Construct>,
}

impl Peek {
    /// Creates a new `Peek` wrapper around `subcon`.
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        Peek { subcon }
    }
}

impl Construct for Peek {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let fallback = stream.tell()?;
        // Try to parse; on error (except Check/StopField), return None
        let result = self.subcon.parse(stream, ctx);
        // Always seek back
        let _ = stream.seek(fallback);
        match result {
            Ok(v) => Ok(v),
            Err(e @ ConstructError::Check { .. } | e @ ConstructError::StopField { .. }) => Err(e),
            Err(_) => Ok(Value::None),
        }
    }

    fn build(&self, _data: &Value, _stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
        // Peek does not build anything
        Ok(())
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Ok(0)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// RawCopy
// ===========================================================================

/// Returns both the raw bytes and the parsed value of the subcon.
///
/// - **parse**: records start/end positions, parses `subcon`, then reads back
///   the raw bytes. Returns a [`Value::Container`] with keys `"data"`,
///   `"value"`, `"offset1"`, `"offset2"`, `"length"`.
/// - **build**: if `"data"` key is present, writes the raw bytes directly;
///   if `"value"` key is present, builds the subcon from that value
/// - **sizeof**: same as subcon
///
/// Corresponds to Python `RawCopy(subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::RawCopy;
/// use construct::constructs::format_field::INT8UB;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = RawCopy::new(Box::new(INT8UB));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\xff").unwrap();
/// let container = parsed.as_container().unwrap();
/// assert_eq!(container.get("value").unwrap(), &Value::UInt(255));
/// assert_eq!(container.get("data").unwrap(), &Value::Bytes(vec![0xff]));
/// ```
pub struct RawCopy {
    /// The inner construct to copy raw bytes from.
    pub subcon: Box<dyn Construct>,
}

impl RawCopy {
    /// Creates a new `RawCopy` wrapper around `subcon`.
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        RawCopy { subcon }
    }
}

impl Construct for RawCopy {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let offset1 = stream.tell()?;
        let obj = self.subcon.parse(stream, ctx)?;
        let offset2 = stream.tell()?;
        let length = offset2.saturating_sub(offset1) as usize;

        // Read back the raw bytes
        stream.seek(offset1)?;
        let data = stream.read_bytes(length)?;

        let mut map = IndexMap::new();
        map.insert("data".to_string(), Value::Bytes(data));
        map.insert("value".to_string(), obj);
        map.insert("offset1".to_string(), Value::UInt(offset1));
        map.insert("offset2".to_string(), Value::UInt(offset2));
        map.insert("length".to_string(), Value::UInt(length as u64));
        Ok(Value::Container(map))
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let container = data.as_container()?;

        if let Some(Value::Bytes(raw_data)) = container.get("data") {
            // Build from raw bytes
            let _offset1 = stream.tell()?;
            stream.write_bytes(raw_data)?;
            return Ok(());
        }

        if let Some(value) = container.get("value") {
            let _offset1 = stream.tell()?;
            self.subcon.build(value, stream, ctx)?;
            return Ok(());
        }

        Err(ConstructError::Generic {
            path: String::new(),
            message: "RawCopy cannot build: both data and value keys are missing".to_string(),
        })
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// Prefixed
// ===========================================================================

/// Prefixes data with a length field.
///
/// - **parse**: parses `length_field` to get the byte count, reads that many
///   bytes into a sub-stream, then parses `subcon` from the sub-stream
/// - **build**: builds `subcon` into a separate buffer, then builds
///   `length_field` with the buffer length, then writes the buffer
/// - **sizeof**: `length_field.sizeof() + subcon.sizeof()`
///
/// Corresponds to Python `Prefixed(lengthfield, subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::Prefixed;
/// use construct::constructs::format_field::INT8UB;
/// use construct::constructs::bytes::GreedyBytes;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = Prefixed::new(Box::new(INT8UB), Box::new(GreedyBytes));
/// // Length byte = 3, then 3 bytes of data
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x03ABC").unwrap();
/// assert_eq!(parsed, Value::Bytes(b"ABC".to_vec()));
/// ```
pub struct Prefixed {
    /// The construct used to parse/build the length prefix.
    pub length_field: Box<dyn Construct>,
    /// The inner construct that processes the prefixed data.
    pub subcon: Box<dyn Construct>,
    /// Whether the length field includes its own size.
    pub include_length: bool,
}

impl Prefixed {
    /// Creates a new `Prefixed` construct with the given length field and
    /// subcon. `include_length` defaults to `false`.
    pub fn new(length_field: Box<dyn Construct>, subcon: Box<dyn Construct>) -> Self {
        Prefixed {
            length_field,
            subcon,
            include_length: false,
        }
    }

    /// Sets whether the length field count includes its own size.
    pub fn with_include_length(mut self, include: bool) -> Self {
        self.include_length = include;
        self
    }
}

impl Construct for Prefixed {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let length_val = self.length_field.parse(stream, ctx)?;
        let mut length = length_val.to_u64()? as usize;

        if self.include_length {
            let length_field_size = self.length_field.sizeof(ctx)?;
            length = length.saturating_sub(length_field_size);
        }

        let data = stream.read_bytes(length)?;
        let mut sub_stream = ByteStream::new_read(&data);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        // Build subcon into a separate buffer
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let built_data = sub_stream.into_bytes();

        let mut length = built_data.len() as u64;
        if self.include_length {
            let length_field_size = self.length_field.sizeof(ctx)?;
            length += length_field_size as u64;
        }

        self.length_field.build(&Value::UInt(length), stream, ctx)?;
        stream.write_bytes(&built_data)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let length_size = self.length_field.sizeof(ctx)?;
        let subcon_size = self.subcon.sizeof(ctx)?;
        Ok(length_size + subcon_size)
    }
}

// ===========================================================================
// Transformed
// ===========================================================================

/// Applies a byte-to-byte transformation to fixed-size data.
///
/// - **parse**: reads `decode_amount` bytes (or all remaining if `None`),
///   applies `decode`, parses `subcon` from the result
/// - **build**: builds `subcon` into a buffer, applies `encode`, writes the
///   result; validates `encode_amount` if specified
/// - **sizeof**: returns `decode_amount` if it equals `encode_amount`
///
/// Used to implement [`ByteSwapped`], [`BitsSwapped`], and similar constructs.
///
/// Corresponds to Python `Transformed(subcon, decodefunc, decodeamount,
/// encodefunc, encodeamount)`.
pub type TransformFunc = Box<dyn Fn(&[u8]) -> Result<Vec<u8>>>;

/// A construct that transforms bytes between the underlying stream and the
/// (fixed-sized) subcon.
///
/// Parsing reads a specified amount (or till EOF), processes data using a
/// bytes-to-bytes decoding function, then parses subcon using those data.
/// Building builds the subcon into separate bytes, then processes it using
/// an encoding bytes-to-bytes function, then writes those data into the
/// main stream.
pub struct Transformed {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// Decoding function applied before parsing subcon.
    pub decode: TransformFunc,
    /// Number of bytes to read before decoding, or `None` to read all.
    pub decode_amount: Option<usize>,
    /// Encoding function applied after building subcon.
    pub encode: TransformFunc,
    /// Expected number of bytes after encoding, or `None` to skip validation.
    pub encode_amount: Option<usize>,
}

impl Transformed {
    /// Creates a new `Transformed` construct.
    pub fn new(
        subcon: Box<dyn Construct>,
        decode: TransformFunc,
        decode_amount: Option<usize>,
        encode: TransformFunc,
        encode_amount: Option<usize>,
    ) -> Self {
        Transformed {
            subcon,
            decode,
            decode_amount,
            encode,
            encode_amount,
        }
    }
}

impl Construct for Transformed {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let data = match self.decode_amount {
            Some(amount) => stream.read_bytes(amount)?,
            None => stream.read_remaining()?,
        };
        let decoded = (self.decode)(&data)?;
        let mut sub_stream = ByteStream::new_read(&decoded);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let encoded = (self.encode)(&built)?;

        if let Some(expected) = self.encode_amount {
            if encoded.len() != expected {
                return Err(ConstructError::Generic {
                    path: String::new(),
                    message: format!(
                        "encoding transformation produced {} bytes instead of expected {}",
                        encoded.len(),
                        expected
                    ),
                });
            }
        }

        stream.write_bytes(&encoded)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        match (self.decode_amount, self.encode_amount) {
            (Some(da), Some(ea)) if da == ea => Ok(ea),
            _ => Err(ConstructError::Sizeof {
                path: String::new(),
                reason: "Transformed size is undefined when decode/encode amounts differ or are \
                         unknown"
                    .to_string(),
            }),
        }
    }
}

// ===========================================================================
// Restreamed
// ===========================================================================

/// Applies a chunk-wise byte transformation for variable-sized data.
///
/// Unlike [`Transformed`], which operates on the entire data at once,
/// `Restreamed` applies the transformation in fixed-size chunks.
///
/// - **parse**: reads from the underlying stream in `decoder_unit` chunks,
///   decodes each chunk, and feeds the result to `subcon`
/// - **build**: collects output from `subcon` in `encoder_unit` chunks,
///   encodes each chunk, and writes to the stream
/// - **sizeof**: uses `size_computer` to convert the subcon's size
///
/// The simplified Rust implementation reads all remaining data, applies the
/// decoder chunk-by-chunk, and then parses the subcon from the result.
///
/// Corresponds to Python `Restreamed(subcon, decoder, decoderunit, encoder,
/// encoderunit, sizecomputer)`.
pub struct Restreamed {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// Chunk decoder function.
    pub decoder: TransformFunc,
    /// Size of chunks fed to the decoder.
    pub decoder_unit: usize,
    /// Chunk encoder function.
    pub encoder: TransformFunc,
    /// Size of chunks fed to the encoder.
    pub encoder_unit: usize,
    /// Function to compute the outer size from the inner size.
    pub size_computer: Option<Box<dyn Fn(usize) -> usize>>,
}

impl Restreamed {
    /// Creates a new `Restreamed` construct.
    pub fn new(
        subcon: Box<dyn Construct>,
        decoder: TransformFunc,
        decoder_unit: usize,
        encoder: TransformFunc,
        encoder_unit: usize,
        size_computer: Option<Box<dyn Fn(usize) -> usize>>,
    ) -> Self {
        Restreamed {
            subcon,
            decoder,
            decoder_unit,
            encoder,
            encoder_unit,
            size_computer,
        }
    }
}

impl Construct for Restreamed {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        // Read all remaining data and decode in chunks
        let raw = stream.read_remaining()?;
        let mut decoded = Vec::new();

        if self.decoder_unit == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "Restreamed decoder_unit must be positive".to_string(),
            });
        }

        for chunk in raw.chunks(self.decoder_unit) {
            let decoded_chunk = (self.decoder)(chunk)?;
            decoded.extend_from_slice(&decoded_chunk);
        }

        let mut sub_stream = ByteStream::new_read(&decoded);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        // Build subcon into buffer
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();

        if self.encoder_unit == 0 {
            return Err(ConstructError::Generic {
                path: String::new(),
                message: "Restreamed encoder_unit must be positive".to_string(),
            });
        }

        // Encode in chunks
        for chunk in built.chunks(self.encoder_unit) {
            let encoded_chunk = (self.encoder)(chunk)?;
            stream.write_bytes(&encoded_chunk)?;
        }

        Ok(())
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        match &self.size_computer {
            Some(computer) => {
                let inner_size = self.subcon.sizeof(ctx)?;
                Ok(computer(inner_size))
            }
            None => Err(ConstructError::Sizeof {
                path: String::new(),
                reason: "Restreamed cannot calculate size without a size_computer".to_string(),
            }),
        }
    }
}

// ===========================================================================
// Compressed (feature-gated)
// ===========================================================================

/// Compresses/decompresses data using zlib or raw deflate.
///
/// This is a tunnel construct: it reads all remaining bytes from the stream,
/// decompresses them, and parses `subcon` from the result. On build, it
/// builds `subcon` into a buffer and compresses the result.
///
/// - **parse**: reads remaining bytes, decompresses, parses `subcon`
/// - **build**: builds `subcon`, compresses, writes
/// - **sizeof**: undefined
///
/// Only available when the `compression` feature is enabled.
///
/// Corresponds to Python `Compressed(subcon, "zlib")`.
#[cfg(feature = "compression")]
pub struct Compressed {
    /// The inner construct operating on decompressed data.
    pub subcon: Box<dyn Construct>,
    /// The compression algorithm to use.
    pub algorithm: CompressionAlgorithm,
}

/// Supported compression algorithms.
#[cfg(feature = "compression")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    /// zlib compression (header + deflate + checksum).
    Zlib,
    /// Raw deflate compression (no header/checksum).
    Deflate,
}

#[cfg(feature = "compression")]
impl Compressed {
    /// Creates a new `Compressed` construct using the specified algorithm.
    pub fn new(subcon: Box<dyn Construct>, algorithm: CompressionAlgorithm) -> Self {
        Compressed { subcon, algorithm }
    }

    fn decode(&self, data: &[u8]) -> Result<Vec<u8>> {
        use std::io::Read;
        match self.algorithm {
            CompressionAlgorithm::Zlib => {
                let mut decoder = flate2::read::ZlibDecoder::new(data);
                let mut decoded = Vec::new();
                decoder
                    .read_to_end(&mut decoded)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("zlib decompression failed: {e}"),
                    })?;
                Ok(decoded)
            }
            CompressionAlgorithm::Deflate => {
                let mut decoder = flate2::read::DeflateDecoder::new(data);
                let mut decoded = Vec::new();
                decoder
                    .read_to_end(&mut decoded)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("deflate decompression failed: {e}"),
                    })?;
                Ok(decoded)
            }
        }
    }

    fn encode(&self, data: &[u8]) -> Result<Vec<u8>> {
        use std::io::Read;
        match self.algorithm {
            CompressionAlgorithm::Zlib => {
                let mut encoder =
                    flate2::read::ZlibEncoder::new(data, flate2::Compression::default());
                let mut encoded = Vec::new();
                encoder
                    .read_to_end(&mut encoded)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("zlib compression failed: {e}"),
                    })?;
                Ok(encoded)
            }
            CompressionAlgorithm::Deflate => {
                let mut encoder =
                    flate2::read::DeflateEncoder::new(data, flate2::Compression::default());
                let mut encoded = Vec::new();
                encoder
                    .read_to_end(&mut encoded)
                    .map_err(|e| ConstructError::Generic {
                        path: String::new(),
                        message: format!("deflate compression failed: {e}"),
                    })?;
                Ok(encoded)
            }
        }
    }
}

#[cfg(feature = "compression")]
impl Construct for Compressed {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let data = stream.read_remaining()?;
        let decoded = self.decode(&data)?;
        let mut sub_stream = ByteStream::new_read(&decoded);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let encoded = self.encode(&built)?;
        stream.write_bytes(&encoded)
    }

    fn sizeof(&self, _ctx: &Context) -> Result<usize> {
        Err(ConstructError::Sizeof {
            path: String::new(),
            reason: "Compressed has undefined size".to_string(),
        })
    }
}

// ===========================================================================
// Checksum
// ===========================================================================

/// Function type for computing a checksum from raw bytes.
pub type ChecksumFunc = Box<dyn Fn(&[u8]) -> Vec<u8>>;

/// Function type for obtaining the bytes to checksum from the context.
pub type ChecksumBytesFunc = Box<dyn Fn(&Context) -> Result<Vec<u8>>>;

/// Validates or computes a checksum field.
///
/// - **parse**: parses `checksum_field`, computes the checksum from
///   `bytes_func(ctx)`, compares them; raises [`ConstructError::Check`] on
///   mismatch
/// - **build**: computes the checksum from `bytes_func(ctx)` and builds it
///   via `checksum_field`
/// - **sizeof**: same as `checksum_field`
///
/// Corresponds to Python `Checksum(checksumfield, hashfunc, bytesfunc)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::Checksum;
/// use construct::constructs::bytes::Bytes;
/// use construct::core::Construct;
/// use construct::value::Value;
/// use construct::core::context::Context;
///
/// // Simple checksum: sum of bytes modulo 256
/// let d = Checksum::new(
///     Box::new(Bytes::new(1)),
///     Box::new(|data: &[u8]| vec![data.iter().fold(0u8, |a, &b| a.wrapping_add(b))]),
///     Box::new(|ctx: &Context| {
///         // In a real use case, this would read bytes from a RawCopy field
///         Ok(vec![0x01, 0x02, 0x03])
///     }),
/// );
/// // Checksum of [1, 2, 3] = 6
/// let c: &dyn Construct = &d;
/// let built = c.build_bytes(&Value::None).unwrap();
/// assert_eq!(built, vec![6]);
/// ```
pub struct Checksum {
    /// The construct used to parse/build the checksum value.
    pub checksum_field: Box<dyn Construct>,
    /// Function that computes the checksum from raw bytes.
    pub hash_func: ChecksumFunc,
    /// Function that extracts the bytes to be checksummed from the context.
    pub bytes_func: ChecksumBytesFunc,
}

impl Checksum {
    /// Creates a new `Checksum` construct.
    pub fn new(
        checksum_field: Box<dyn Construct>,
        hash_func: ChecksumFunc,
        bytes_func: ChecksumBytesFunc,
    ) -> Self {
        Checksum {
            checksum_field,
            hash_func,
            bytes_func,
        }
    }
}

impl Construct for Checksum {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let parsed = self.checksum_field.parse(stream, ctx)?;
        let raw_bytes = (self.bytes_func)(ctx)?;
        let expected = (self.hash_func)(&raw_bytes);

        // Compare parsed checksum with computed
        let parsed_bytes = parsed.as_bytes().unwrap_or(&[]).to_vec();
        if parsed_bytes != expected {
            return Err(ConstructError::Check {
                path: String::new(),
                message: format!(
                    "wrong checksum: read {:02x?}, computed {:02x?}",
                    parsed_bytes, expected
                ),
            });
        }

        Ok(parsed)
    }

    fn build(&self, _data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let raw_bytes = (self.bytes_func)(ctx)?;
        let checksum = (self.hash_func)(&raw_bytes);
        self.checksum_field
            .build(&Value::Bytes(checksum), stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.checksum_field.sizeof(ctx)
    }

    fn flagbuildnone(&self) -> bool {
        true
    }
}

// ===========================================================================
// ByteSwapped
// ===========================================================================

/// Reverses the byte order within the boundaries of a fixed-size subcon.
///
/// - **parse**: reads `sizeof(subcon)` bytes, reverses them, parses `subcon`
/// - **build**: builds `subcon`, reverses the bytes, writes
/// - **sizeof**: same as subcon (requires fixed size)
///
/// Corresponds to Python `ByteSwapped(subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::ByteSwapped;
/// use construct::constructs::bytes_integer::BytesInteger;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = ByteSwapped::new(Box::new(BytesInteger::new(4, false, false)));
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\x01\x02\x03\x04").unwrap();
/// // Bytes swapped: [4, 3, 2, 1] interpreted as big-endian = 0x04030201
/// assert_eq!(parsed, Value::UInt(0x04030201));
/// ```
pub struct ByteSwapped {
    /// The inner construct whose byte output is to be reversed.
    pub subcon: Box<dyn Construct>,
}

impl ByteSwapped {
    /// Creates a new `ByteSwapped` wrapper.
    ///
    /// The subcon must have a fixed, known size.
    pub fn new(subcon: Box<dyn Construct>) -> Self {
        ByteSwapped { subcon }
    }
}

impl Construct for ByteSwapped {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let size = self.subcon.sizeof(ctx)?;
        let data = stream.read_bytes(size)?;
        let swapped = binary::swapbytes(&data);
        let mut sub_stream = ByteStream::new_read(&swapped);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let swapped = binary::swapbytes(&built);
        stream.write_bytes(&swapped)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// BitsSwapped
// ===========================================================================

/// Reverses the bit order within each byte.
///
/// Unlike [`ByteSwapped`], this does not require a fixed-size subcon.
/// It applies `swapbitsinbytes` transformation.
///
/// - **parse**: reads the subcon's data, reverses bits in each byte, parses
/// - **build**: builds subcon, reverses bits in each byte, writes
/// - **sizeof**: same as subcon (for fixed size); falls back to Restreamed
///   for variable size
///
/// Corresponds to Python `BitsSwapped(subcon)`.
///
/// # Examples
///
/// ```
/// use construct::constructs::stream_ops::BitsSwapped;
/// use construct::constructs::bytes::Bytes;
/// use construct::core::Construct;
/// use construct::value::Value;
///
/// let d = BitsSwapped::new_fixed(Box::new(Bytes::new(1)));
/// // 0b10101010 → swapbits → 0b01010101
/// let c: &dyn Construct = &d;
/// let parsed = c.parse_bytes(b"\xaa").unwrap();
/// assert_eq!(parsed, Value::Bytes(vec![0x55]));
/// ```
pub struct BitsSwapped {
    /// The inner construct.
    pub subcon: Box<dyn Construct>,
    /// Whether the subcon has a fixed known size.
    pub fixed_size: bool,
}

/// Reverses the bit order within each byte of `data`.
fn swapbitsinbytes(data: &[u8]) -> Vec<u8> {
    data.iter()
        .map(|&b| {
            // Reverse bits in a byte
            let mut result = 0u8;
            for i in 0..8 {
                if b & (1 << i) != 0 {
                    result |= 1 << (7 - i);
                }
            }
            result
        })
        .collect()
}

impl BitsSwapped {
    /// Creates a new `BitsSwapped` for a fixed-size subcon.
    ///
    /// Uses the `Transformed` approach: reads exactly `sizeof(subcon)` bytes,
    /// swaps bits, and parses.
    pub fn new_fixed(subcon: Box<dyn Construct>) -> Self {
        BitsSwapped {
            subcon,
            fixed_size: true,
        }
    }

    /// Creates a new `BitsSwapped` for a variable-size subcon.
    ///
    /// Uses a simplified approach that reads remaining data.
    pub fn new_variable(subcon: Box<dyn Construct>) -> Self {
        BitsSwapped {
            subcon,
            fixed_size: false,
        }
    }
}

impl Construct for BitsSwapped {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let data = if self.fixed_size {
            let size = self.subcon.sizeof(ctx)?;
            stream.read_bytes(size)?
        } else {
            stream.read_remaining()?
        };
        let swapped = swapbitsinbytes(&data);
        let mut sub_stream = ByteStream::new_read(&swapped);
        self.subcon.parse(&mut sub_stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let mut sub_stream = ByteStream::new_write();
        self.subcon.build(data, &mut sub_stream, ctx)?;
        let built = sub_stream.into_bytes();
        let swapped = swapbitsinbytes(&built);
        stream.write_bytes(&swapped)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        self.subcon.sizeof(ctx)
    }
}

// ===========================================================================
// LazyBound
// ===========================================================================

/// Binds to a subcon at runtime (during parse/build, not construction).
///
/// Useful for recursive data structures where a construct needs to refer to
/// itself. The subcon is obtained by calling `subcon_func()` each time parse
/// or build is invoked.
///
/// - **parse**: calls `subcon_func()`, delegates parse
/// - **build**: calls `subcon_func()`, delegates build
/// - **sizeof**: calls `subcon_func()`, delegates sizeof
///
/// Corresponds to Python `LazyBound(subconfunc)`.
pub struct LazyBound {
    /// Function that returns the construct to use.
    pub subcon_func: Box<dyn Fn() -> Box<dyn Construct>>,
}

impl LazyBound {
    /// Creates a new `LazyBound` with the given subcon factory function.
    pub fn new(subcon_func: Box<dyn Fn() -> Box<dyn Construct>>) -> Self {
        LazyBound { subcon_func }
    }
}

impl Construct for LazyBound {
    fn parse(&self, stream: &mut dyn Stream, ctx: &mut Context) -> Result<Value> {
        let subcon = (self.subcon_func)();
        subcon.parse(stream, ctx)
    }

    fn build(&self, data: &Value, stream: &mut dyn Stream, ctx: &mut Context) -> Result<()> {
        let subcon = (self.subcon_func)();
        subcon.build(data, stream, ctx)
    }

    fn sizeof(&self, ctx: &Context) -> Result<usize> {
        let subcon = (self.subcon_func)();
        subcon.sizeof(ctx)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constructs::bytes::{Bytes, GreedyBytes};
    use crate::constructs::format_field::INT8UB;

    // -- Helper constructs for testing ---------------------------------------

    /// A simple 2-byte big-endian unsigned integer for testing.
    struct U16Big;

    impl Construct for U16Big {
        fn parse(&self, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<Value> {
            let bytes = stream.read_bytes(2)?;
            let val = u16::from_be_bytes([bytes[0], bytes[1]]);
            Ok(Value::UInt(val as u64))
        }

        fn build(&self, data: &Value, stream: &mut dyn Stream, _ctx: &mut Context) -> Result<()> {
            let val = data.to_u64()? as u16;
            stream.write_bytes(&val.to_be_bytes())
        }

        fn sizeof(&self, _ctx: &Context) -> Result<usize> {
            Ok(2)
        }
    }

    // ======================================================================
    // Bitwise tests
    // ======================================================================

    #[test]
    fn bitwise_parse_basic() {
        // Bitwise(Bytes(8)) in Python: input \x01\x03 → bytes2bits → 16 bits as bytes
        // Bytes(8) reads 8 items (bits) from the bit stream.
        // For \xff: bytes2bits([0xff]) = [1,1,1,1,1,1,1,1]
        let d = Bitwise::new(Box::new(Bytes::new(8)));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\xff").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![1, 1, 1, 1, 1, 1, 1, 1]));
    }

    #[test]
    fn bitwise_build_basic() {
        // Build: bits [1,1,1,1,1,1,1,1] → bits2bytes → [0xff]
        let d = Bitwise::new(Box::new(Bytes::new(8)));
        let c: &dyn Construct = &d;
        let built = c
            .build_bytes(&Value::Bytes(vec![1, 1, 1, 1, 1, 1, 1, 1]))
            .unwrap();
        assert_eq!(built, vec![0xff]);
    }

    #[test]
    fn bitwise_roundtrip() {
        let d = Bitwise::new(Box::new(Bytes::new(8)));
        let c: &dyn Construct = &d;
        // The subcon sees bits as bytes, so the value is a byte array of bits
        let original = Value::Bytes(vec![1, 1, 1, 1, 1, 1, 1, 1]);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bitwise_sizeof() {
        let d = Bitwise::new(Box::new(Bytes::new(8)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 1); // 8 bits = 1 byte
    }

    #[test]
    fn bitwise_sizeof_rounds_up() {
        // 12 bits should round up to 2 bytes
        let d = Bitwise::new(Box::new(Bytes::new(12)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 2);
    }

    // ======================================================================
    // Bytewise tests
    // ======================================================================

    #[test]
    fn bytewise_parse_basic() {
        let d = Bytewise::new(Box::new(Bytes::new(2)));
        let bits: Vec<u8> = vec![1, 0, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 0];
        let mut stream = ByteStream::new_read(&bits);
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        // bits2bytes of [1,0,1,0,1,0,1,0] = 0xAA, [1,1,0,0,1,1,0,0] = 0xCC
        assert_eq!(result, Value::Bytes(vec![0xAA, 0xCC]));
    }

    #[test]
    fn bytewise_sizeof() {
        let d = Bytewise::new(Box::new(Bytes::new(2)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 16); // 2 bytes = 16 bits
    }

    // ======================================================================
    // Pointer tests
    // ======================================================================

    #[test]
    fn pointer_parse_absolute_offset() {
        let d = Pointer::new(8, Box::new(Bytes::new(1)));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"abcdefghijkl").unwrap();
        assert_eq!(parsed, Value::Bytes(b"i".to_vec()));
    }

    #[test]
    fn pointer_build_absolute_offset() {
        let d = Pointer::new(8, Box::new(Bytes::new(1)));
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::Bytes(b"Z".to_vec())).unwrap();
        assert_eq!(built.len(), 9); // 8 nulls + Z
        assert_eq!(built[8], b'Z');
    }

    #[test]
    fn pointer_sizeof_is_zero() {
        let d = Pointer::new(5, Box::new(Bytes::new(2)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn pointer_roundtrip() {
        let d = Pointer::new(4, Box::new(U16Big));
        let c: &dyn Construct = &d;
        let original = Value::UInt(0x0102);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn pointer_parse_negative_offset() {
        let d = Pointer::new(-2, Box::new(Bytes::new(1)));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"abcdefghijkl").unwrap();
        // len=12, offset=-2 from end → position 10, read 1 byte = 'k'
        assert_eq!(parsed, Value::Bytes(b"k".to_vec()));
    }

    // ======================================================================
    // Peek tests
    // ======================================================================

    #[test]
    fn peek_parse_does_not_advance() {
        let d = Peek::new(Box::new(INT8UB));
        let data = b"\x2a\xFF";
        let mut stream = ByteStream::new_read(data);
        let mut ctx = Context::new();

        let parsed = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(parsed, Value::UInt(0x2a));
        // Position should still be 0
        assert_eq!(stream.tell().unwrap(), 0);

        // Now read the byte normally
        let byte = stream.read_bytes(1).unwrap();
        assert_eq!(byte, vec![0x2a]);
    }

    #[test]
    fn peek_parse_on_error_returns_none() {
        let d = Peek::new(Box::new(Bytes::new(100)));
        let data = b"\x01\x02";
        let mut stream = ByteStream::new_read(data);
        let mut ctx = Context::new();
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::None);
        assert_eq!(stream.tell().unwrap(), 0);
    }

    #[test]
    fn peek_build_does_nothing() {
        let d = Peek::new(Box::new(INT8UB));
        let mut stream = ByteStream::new_write();
        let mut ctx = Context::new();
        d.build(&Value::UInt(42), &mut stream, &mut ctx).unwrap();
        assert!(stream.into_bytes().is_empty());
    }

    #[test]
    fn peek_sizeof_is_zero() {
        let d = Peek::new(Box::new(INT8UB));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 0);
    }

    #[test]
    fn peek_flagbuildnone() {
        let d = Peek::new(Box::new(INT8UB));
        assert!(d.flagbuildnone());
    }

    // ======================================================================
    // RawCopy tests
    // ======================================================================

    #[test]
    fn rawcopy_parse_returns_container() {
        let d = RawCopy::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\xff").unwrap();
        let container = parsed.as_container().unwrap();
        assert_eq!(container.get("value").unwrap(), &Value::UInt(255));
        assert_eq!(container.get("data").unwrap(), &Value::Bytes(vec![0xff]));
        assert_eq!(container.get("offset1").unwrap(), &Value::UInt(0));
        assert_eq!(container.get("offset2").unwrap(), &Value::UInt(1));
        assert_eq!(container.get("length").unwrap(), &Value::UInt(1));
    }

    #[test]
    fn rawcopy_build_from_data() {
        let d = RawCopy::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let mut map = IndexMap::new();
        map.insert("data".to_string(), Value::Bytes(vec![0x42]));
        let built = c.build_bytes(&Value::Container(map)).unwrap();
        assert_eq!(built, vec![0x42]);
    }

    #[test]
    fn rawcopy_build_from_value() {
        let d = RawCopy::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let mut map = IndexMap::new();
        map.insert("value".to_string(), Value::UInt(0x42));
        let built = c.build_bytes(&Value::Container(map)).unwrap();
        assert_eq!(built, vec![0x42]);
    }

    #[test]
    fn rawcopy_build_missing_keys_returns_error() {
        let d = RawCopy::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let result = c.build_bytes(&Value::Container(IndexMap::new()));
        assert!(result.is_err());
    }

    #[test]
    fn rawcopy_roundtrip() {
        let d = RawCopy::new(Box::new(INT8UB));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\xab").unwrap();
        let built = c.build_bytes(&parsed).unwrap();
        assert_eq!(built, vec![0xab]);
    }

    #[test]
    fn rawcopy_sizeof() {
        let d = RawCopy::new(Box::new(INT8UB));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 1);
    }

    // ======================================================================
    // Prefixed tests
    // ======================================================================

    #[test]
    fn prefixed_parse_basic() {
        let d = Prefixed::new(Box::new(INT8UB), Box::new(GreedyBytes));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x03ABC").unwrap();
        assert_eq!(parsed, Value::Bytes(b"ABC".to_vec()));
    }

    #[test]
    fn prefixed_build_basic() {
        let d = Prefixed::new(Box::new(INT8UB), Box::new(GreedyBytes));
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::Bytes(b"ABC".to_vec())).unwrap();
        assert_eq!(built, b"\x03ABC");
    }

    #[test]
    fn prefixed_roundtrip() {
        let d = Prefixed::new(Box::new(INT8UB), Box::new(GreedyBytes));
        let c: &dyn Construct = &d;
        let original = Value::Bytes(b"hello".to_vec());
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn prefixed_sizeof() {
        let d = Prefixed::new(Box::new(INT8UB), Box::new(Bytes::new(4)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 5); // 1 (length) + 4 (data)
    }

    #[test]
    fn prefixed_with_include_length() {
        let d = Prefixed::new(Box::new(INT8UB), Box::new(GreedyBytes)).with_include_length(true);
        let c: &dyn Construct = &d;
        let original = Value::Bytes(b"AB".to_vec());
        let built = c.build_bytes(&original).unwrap();
        // Length includes itself: 2 (data) + 1 (length field) = 3
        assert_eq!(built[0], 3);
        assert_eq!(&built[1..], b"AB");

        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // Transformed tests
    // ======================================================================

    #[test]
    fn transformed_parse_basic() {
        let d = Transformed::new(
            Box::new(Bytes::new(2)),
            Box::new(|data: &[u8]| Ok(binary::swapbytes(data))),
            Some(2),
            Box::new(|data: &[u8]| Ok(binary::swapbytes(data))),
            Some(2),
        );
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x01\x02").unwrap();
        // Swap: [1,2] → subcon reads [2,1]
        assert_eq!(parsed, Value::Bytes(vec![0x02, 0x01]));
    }

    #[test]
    fn transformed_build_basic() {
        let d = Transformed::new(
            Box::new(Bytes::new(2)),
            Box::new(|data: &[u8]| Ok(binary::swapbytes(data))),
            Some(2),
            Box::new(|data: &[u8]| Ok(binary::swapbytes(data))),
            Some(2),
        );
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::Bytes(vec![0x01, 0x02])).unwrap();
        // Build [1,2], then swap: [2,1]
        assert_eq!(built, vec![0x02, 0x01]);
    }

    #[test]
    fn transformed_roundtrip() {
        let d = Transformed::new(
            Box::new(Bytes::new(4)),
            Box::new(|data: &[u8]| Ok(binary::swapbytes(data))),
            Some(4),
            Box::new(|data: &[u8]| Ok(binary::swapbytes(data))),
            Some(4),
        );
        let c: &dyn Construct = &d;
        let original = Value::Bytes(vec![0x01, 0x02, 0x03, 0x04]);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn transformed_sizeof_equal_amounts() {
        let d = Transformed::new(
            Box::new(Bytes::new(2)),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            Some(2),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            Some(2),
        );
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 2);
    }

    #[test]
    fn transformed_sizeof_unequal_amounts_errors() {
        let d = Transformed::new(
            Box::new(Bytes::new(2)),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            Some(2),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            Some(3),
        );
        let ctx = Context::new();
        assert!(d.sizeof(&ctx).is_err());
    }

    #[test]
    fn transformed_read_all_when_amount_is_none() {
        let d = Transformed::new(
            Box::new(GreedyBytes),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            None,
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            None,
        );
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x01\x02\x03").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn transformed_encode_amount_mismatch_errors() {
        let d = Transformed::new(
            Box::new(Bytes::new(2)),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            Some(2),
            Box::new(|_data: &[u8]| Ok(vec![0u8; 5])), // produces 5 bytes
            Some(2),                                   // but expects 2
        );
        let c: &dyn Construct = &d;
        let result = c.build_bytes(&Value::Bytes(vec![0x01, 0x02]));
        assert!(result.is_err());
    }

    // ======================================================================
    // Restreamed tests
    // ======================================================================

    #[test]
    fn restreamed_parse_chunk_decode() {
        // Decode each byte by XORing with 0xFF
        let d = Restreamed::new(
            Box::new(GreedyBytes),
            Box::new(|chunk: &[u8]| Ok(chunk.iter().map(|&b| b ^ 0xFF).collect())),
            1,
            Box::new(|chunk: &[u8]| Ok(chunk.iter().map(|&b| b ^ 0xFF).collect())),
            1,
            None,
        );
        let c: &dyn Construct = &d;
        // [0x00, 0xFF] XOR 0xFF per byte → [0xFF, 0x00]
        let parsed = c.parse_bytes(b"\x00\xff").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![0xFF, 0x00]));
    }

    #[test]
    fn restreamed_build_chunk_encode() {
        let d = Restreamed::new(
            Box::new(GreedyBytes),
            Box::new(|chunk: &[u8]| Ok(chunk.iter().map(|&b| b ^ 0xFF).collect())),
            1,
            Box::new(|chunk: &[u8]| Ok(chunk.iter().map(|&b| b ^ 0xFF).collect())),
            1,
            None,
        );
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::Bytes(vec![0xFF, 0x00])).unwrap();
        assert_eq!(built, vec![0x00, 0xFF]);
    }

    #[test]
    fn restreamed_sizeof_with_computer() {
        let d = Restreamed::new(
            Box::new(Bytes::new(4)),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            1,
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            1,
            Some(Box::new(|n: usize| n)), // identity
        );
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn restreamed_sizeof_without_computer_errors() {
        let d = Restreamed::new(
            Box::new(Bytes::new(4)),
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            1,
            Box::new(|data: &[u8]| Ok(data.to_vec())),
            1,
            None,
        );
        let ctx = Context::new();
        assert!(d.sizeof(&ctx).is_err());
    }

    #[test]
    fn restreamed_roundtrip() {
        let d = Restreamed::new(
            Box::new(Bytes::new(3)),
            Box::new(|chunk: &[u8]| Ok(chunk.iter().map(|&b| !b).collect())),
            1,
            Box::new(|chunk: &[u8]| Ok(chunk.iter().map(|&b| !b).collect())),
            1,
            Some(Box::new(|n: usize| n)),
        );
        let c: &dyn Construct = &d;
        let original = Value::Bytes(vec![0x01, 0x02, 0x03]);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // Compressed tests (feature-gated)
    // ======================================================================

    #[cfg(feature = "compression")]
    mod compression_tests {
        use super::*;

        #[test]
        fn compressed_zlib_roundtrip() {
            let d = Compressed::new(Box::new(GreedyBytes), CompressionAlgorithm::Zlib);
            let c: &dyn Construct = &d;
            let original = Value::Bytes(b"hello world, this is a compression test!".to_vec());
            let built = c.build_bytes(&original).unwrap();
            let parsed = c.parse_bytes(&built).unwrap();
            assert_eq!(parsed, original);
        }

        #[test]
        fn compressed_deflate_roundtrip() {
            let d = Compressed::new(Box::new(GreedyBytes), CompressionAlgorithm::Deflate);
            let c: &dyn Construct = &d;
            let original = Value::Bytes(b"test data for deflate".to_vec());
            let built = c.build_bytes(&original).unwrap();
            let parsed = c.parse_bytes(&built).unwrap();
            assert_eq!(parsed, original);
        }

        #[test]
        fn compressed_sizeof_errors() {
            let d = Compressed::new(Box::new(GreedyBytes), CompressionAlgorithm::Zlib);
            let ctx = Context::new();
            assert!(d.sizeof(&ctx).is_err());
        }

        #[test]
        fn compressed_zlib_actually_compresses() {
            let d = Compressed::new(Box::new(GreedyBytes), CompressionAlgorithm::Zlib);
            let c: &dyn Construct = &d;
            // 100 zero bytes should compress very well
            let original = Value::Bytes(vec![0u8; 100]);
            let built = c.build_bytes(&original).unwrap();
            assert!(
                built.len() < 100,
                "Compressed data ({}) should be smaller than original (100)",
                built.len()
            );
        }
    }

    // ======================================================================
    // Checksum tests
    // ======================================================================

    #[test]
    fn checksum_build_computes_and_writes() {
        let d = Checksum::new(
            Box::new(Bytes::new(1)),
            Box::new(|data: &[u8]| vec![data.iter().fold(0u8, |a, &b| a.wrapping_add(b))]),
            Box::new(|_ctx: &Context| Ok(vec![0x01, 0x02, 0x03])),
        );
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::None).unwrap();
        // Sum of [1,2,3] = 6
        assert_eq!(built, vec![6]);
    }

    #[test]
    fn checksum_parse_validates() {
        let d = Checksum::new(
            Box::new(Bytes::new(1)),
            Box::new(|data: &[u8]| vec![data.iter().fold(0u8, |a, &b| a.wrapping_add(b))]),
            Box::new(|_ctx: &Context| Ok(vec![0x01, 0x02, 0x03])),
        );

        // Valid checksum: 1+2+3=6
        let mut ctx = Context::new();
        let mut stream = ByteStream::new_read(b"\x06");
        let result = d.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(result, Value::Bytes(vec![6]));
    }

    #[test]
    fn checksum_parse_rejects_invalid() {
        let d = Checksum::new(
            Box::new(Bytes::new(1)),
            Box::new(|data: &[u8]| vec![data.iter().fold(0u8, |a, &b| a.wrapping_add(b))]),
            Box::new(|_ctx: &Context| Ok(vec![0x01, 0x02, 0x03])),
        );

        // Invalid checksum: should be 6, not 99
        let mut ctx = Context::new();
        let mut stream = ByteStream::new_read(b"\x63");
        let result = d.parse(&mut stream, &mut ctx);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConstructError::Check { .. }));
    }

    #[test]
    fn checksum_sizeof() {
        let d = Checksum::new(
            Box::new(Bytes::new(4)),
            Box::new(|data: &[u8]| data.to_vec()),
            Box::new(|_ctx: &Context| Ok(vec![])),
        );
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 4);
    }

    #[test]
    fn checksum_flagbuildnone() {
        let d = Checksum::new(
            Box::new(Bytes::new(1)),
            Box::new(|data: &[u8]| data.to_vec()),
            Box::new(|_ctx: &Context| Ok(vec![])),
        );
        assert!(d.flagbuildnone());
    }

    // ======================================================================
    // ByteSwapped tests
    // ======================================================================

    #[test]
    fn byteswapped_parse_reverses_bytes() {
        let d = ByteSwapped::new(Box::new(Bytes::new(4)));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x01\x02\x03\x04").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![0x04, 0x03, 0x02, 0x01]));
    }

    #[test]
    fn byteswapped_build_reverses_bytes() {
        let d = ByteSwapped::new(Box::new(Bytes::new(4)));
        let c: &dyn Construct = &d;
        let built = c
            .build_bytes(&Value::Bytes(vec![0x01, 0x02, 0x03, 0x04]))
            .unwrap();
        assert_eq!(built, vec![0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn byteswapped_roundtrip() {
        let d = ByteSwapped::new(Box::new(Bytes::new(4)));
        let c: &dyn Construct = &d;
        let original = Value::Bytes(vec![0xAA, 0xBB, 0xCC, 0xDD]);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn byteswapped_sizeof() {
        let d = ByteSwapped::new(Box::new(Bytes::new(4)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 4);
    }

    // ======================================================================
    // BitsSwapped tests
    // ======================================================================

    #[test]
    fn bitsswapped_parse_reverses_bits() {
        let d = BitsSwapped::new_fixed(Box::new(Bytes::new(1)));
        let c: &dyn Construct = &d;
        // 0xAA = 10101010 → reversed bits = 01010101 = 0x55
        let parsed = c.parse_bytes(b"\xaa").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![0x55]));
    }

    #[test]
    fn bitsswapped_build_reverses_bits() {
        let d = BitsSwapped::new_fixed(Box::new(Bytes::new(1)));
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::Bytes(vec![0x55])).unwrap();
        assert_eq!(built, vec![0xAA]);
    }

    #[test]
    fn bitsswapped_roundtrip() {
        let d = BitsSwapped::new_fixed(Box::new(Bytes::new(2)));
        let c: &dyn Construct = &d;
        let original = Value::Bytes(vec![0xAA, 0x55]);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn bitsswapped_variable_size() {
        let d = BitsSwapped::new_variable(Box::new(GreedyBytes));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\xf0\x0f").unwrap();
        // 0xF0 = 11110000 → 00001111 = 0x0F
        // 0x0F = 00001111 → 11110000 = 0xF0
        assert_eq!(parsed, Value::Bytes(vec![0x0F, 0xF0]));
    }

    #[test]
    fn bitsswapped_sizeof() {
        let d = BitsSwapped::new_fixed(Box::new(Bytes::new(2)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 2);
    }

    // ======================================================================
    // swapbitsinbytes helper tests
    // ======================================================================

    #[test]
    fn swapbitsinbytes_identity() {
        // 0x00 and 0xFF are palindromic in bits
        assert_eq!(swapbitsinbytes(&[0x00]), vec![0x00]);
        assert_eq!(swapbitsinbytes(&[0xFF]), vec![0xFF]);
    }

    #[test]
    fn swapbitsinbytes_swap() {
        // 0xAA = 10101010 → 01010101 = 0x55
        assert_eq!(swapbitsinbytes(&[0xAA]), vec![0x55]);
        // 0x55 = 01010101 → 10101010 = 0xAA
        assert_eq!(swapbitsinbytes(&[0x55]), vec![0xAA]);
    }

    #[test]
    fn swapbitsinbytes_double_swap_is_identity() {
        let data = vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF];
        let swapped = swapbitsinbytes(&data);
        let double_swapped = swapbitsinbytes(&swapped);
        assert_eq!(double_swapped, data);
    }

    // ======================================================================
    // LazyBound tests
    // ======================================================================

    #[test]
    fn lazybound_parse_delegates() {
        let d = LazyBound::new(Box::new(|| Box::new(INT8UB)));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x2a").unwrap();
        assert_eq!(parsed, Value::UInt(42));
    }

    #[test]
    fn lazybound_build_delegates() {
        let d = LazyBound::new(Box::new(|| Box::new(INT8UB)));
        let c: &dyn Construct = &d;
        let built = c.build_bytes(&Value::UInt(42)).unwrap();
        assert_eq!(built, vec![42]);
    }

    #[test]
    fn lazybound_sizeof_delegates() {
        let d = LazyBound::new(Box::new(|| Box::new(INT8UB)));
        let ctx = Context::new();
        assert_eq!(d.sizeof(&ctx).unwrap(), 1);
    }

    #[test]
    fn lazybound_roundtrip() {
        let d = LazyBound::new(Box::new(|| Box::new(INT8UB)));
        let c: &dyn Construct = &d;
        let original = Value::UInt(200);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    // ======================================================================
    // Convenience round-trip tests
    // ======================================================================

    #[test]
    fn pointer_convenience_roundtrip() {
        let d = Pointer::new(0, Box::new(U16Big));
        let c: &dyn Construct = &d;
        let original = Value::UInt(0x1234);
        let built = c.build_bytes(&original).unwrap();
        let parsed = c.parse_bytes(&built).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn peek_convenience_peek_twice() {
        let peek1 = Peek::new(Box::new(INT8UB));
        let data = b"\x0a\x0b";

        let mut stream = ByteStream::new_read(data);
        let mut ctx = Context::new();
        let first = peek1.parse(&mut stream, &mut ctx).unwrap();
        let second = peek1.parse(&mut stream, &mut ctx).unwrap();
        assert_eq!(first, Value::UInt(0x0a));
        assert_eq!(second, Value::UInt(0x0a));
    }

    #[test]
    fn prefixed_empty_data() {
        let d = Prefixed::new(Box::new(INT8UB), Box::new(GreedyBytes));
        let c: &dyn Construct = &d;
        let parsed = c.parse_bytes(b"\x00").unwrap();
        assert_eq!(parsed, Value::Bytes(vec![]));
    }
}
