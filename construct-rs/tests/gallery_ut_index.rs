//! Integration tests for UTIndex gallery construct.

use construct::core::Construct;
use construct::gallery::ut_index::UTIndex;
use construct::value::Value;

fn c() -> &'static dyn Construct {
    // Leak to get a 'static reference for convenience in tests
    Box::leak(Box::new(UTIndex::new()))
}

// =========================================================================
// Parse tests
// =========================================================================

#[test]
fn parse_zero() {
    assert_eq!(c().parse_bytes(&[0x00]).unwrap(), Value::Int(0));
}

#[test]
fn parse_single_byte_values() {
    for v in 0..=63i64 {
        let byte = v as u8;
        assert_eq!(
            c().parse_bytes(&[byte]).unwrap(),
            Value::Int(v),
            "failed for byte 0x{byte:02X}"
        );
    }
}

#[test]
fn parse_two_byte_values() {
    // Value 64: [0x40, 0x01] → 0 + (1<<6) = 64
    assert_eq!(c().parse_bytes(&[0x40, 0x01]).unwrap(), Value::Int(64));
    // Value 127: [0x7F, 0x01] → 63 + (1<<6) = 127
    assert_eq!(c().parse_bytes(&[0x7F, 0x01]).unwrap(), Value::Int(127));
    // Value 8191: [0x7F, 0x7F] → 63 + (127<<6) = 63 + 8128 = 8191
    assert_eq!(c().parse_bytes(&[0x7F, 0x7F]).unwrap(), Value::Int(8191));
}

#[test]
fn parse_negative_values() {
    assert_eq!(c().parse_bytes(&[0x81]).unwrap(), Value::Int(-1));
    assert_eq!(c().parse_bytes(&[0xBF]).unwrap(), Value::Int(-63));
    // -64: sign(0x80) + continuation(0x40) + data=0 → [0xC0, 0x01]
    assert_eq!(c().parse_bytes(&[0xC0, 0x01]).unwrap(), Value::Int(-64));
}

#[test]
fn parse_max_value() {
    // 2^35 - 1 = 34359738367
    let data = [0x7F, 0xFF, 0xFF, 0xFF, 0xFF];
    assert_eq!(c().parse_bytes(&data).unwrap(), Value::Int(34_359_738_367));
}

#[test]
fn parse_empty_stream_errors() {
    assert!(c().parse_bytes(b"").is_err());
}

// =========================================================================
// Build tests
// =========================================================================

#[test]
fn build_zero() {
    assert_eq!(c().build_bytes(&Value::Int(0)).unwrap(), vec![0x00]);
}

#[test]
fn build_small_values() {
    for v in 0..=63i64 {
        assert_eq!(
            c().build_bytes(&Value::Int(v)).unwrap(),
            vec![v as u8],
            "failed for value {v}"
        );
    }
}

#[test]
fn build_negative_values() {
    assert_eq!(c().build_bytes(&Value::Int(-1)).unwrap(), vec![0x81]);
    assert_eq!(c().build_bytes(&Value::Int(-63)).unwrap(), vec![0xBF]);
}

// =========================================================================
// Roundtrip tests
// =========================================================================

#[test]
fn roundtrip_all_ranges() {
    let test_values: Vec<i64> = vec![
        0,
        1,
        2,
        10,
        50,
        63,
        64,
        65,
        127,
        128,
        8191,
        8192,
        65535,
        65536,
        1_000_000,
        100_000_000,
        1_000_000_000,
        10_000_000_000,
        34_359_738_367, // max positive
    ];

    for &v in &test_values {
        let built = c().build_bytes(&Value::Int(v)).unwrap();
        let parsed = c().parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Int(v), "roundtrip failed for value {v}");
    }

    let neg_values: Vec<i64> = vec![
        -1,
        -2,
        -10,
        -50,
        -63,
        -64,
        -65,
        -127,
        -128,
        -8191,
        -8192,
        -65535,
        -65536,
        -1_000_000,
        -100_000_000,
    ];

    for &v in &neg_values {
        let built = c().build_bytes(&Value::Int(v)).unwrap();
        let parsed = c().parse_bytes(&built).unwrap();
        assert_eq!(parsed, Value::Int(v), "roundtrip failed for value {v}");
    }
}

// =========================================================================
// Boundary / error tests
// =========================================================================

#[test]
fn build_overflow_errors() {
    // 2^35 exceeds maximum (2^35 - 1)
    assert!(c().build_bytes(&Value::Int(1i64 << 35)).is_err());
}

#[test]
fn sizeof_errors() {
    use construct::core::context::Context;
    assert!(c().sizeof(&Context::new()).is_err());
}
