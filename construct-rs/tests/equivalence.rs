//! Phase 12.9 equivalence tests.
//!
//! These tests verify that the compiled execution tree (Phase 12)
//! produces **identical** results to the declaration-tree path for a
//! wide range of schemas. Each test builds a schema, compiles it via
//! [`SchemaCompiler`], then compares `parse_bytes` and `build_bytes`
//! outputs between the old path and the new compiled path.
//!
//! If any of these tests fail, it indicates a behavioral divergence
//! between the compiled and declaration-tree implementations.

use construct::combined::CombinedConstruct;
use construct::compiler::SchemaCompiler;
use construct::constructs::adapters::{ExprAdapter, SymmetricAdapter};
use construct::constructs::bytes::{Bytes, BytesExpr, GreedyBytes};
use construct::constructs::computed::{Computed, Default as DefaultC, Padded, Rebuild};
use construct::constructs::control_flow::{IfThenElse, Switch};
use construct::constructs::enum_::Enum;
use construct::constructs::format_field::{INT16UB, INT32UB, INT8SB, INT8UB};
use construct::constructs::meta::Pass;
use construct::constructs::repetition::{Array, ArrayExpr, GreedyRange};
use construct::constructs::sequence::Sequence;
use construct::constructs::stream_ops::{Bitwise, Pointer, Prefixed};
use construct::constructs::struct_::Struct;
use construct::constructs::varint::VarInt;
use construct::core::Construct;
use construct::core::Renamed;
use construct::expr::{this_, CombinedExpr};
use construct::value::Value;

use indexmap::IndexMap;

// ===========================================================================
// Helper: compile a schema and compare parse + build
// ===========================================================================

/// Compiles `cc` and asserts that both paths produce the same parse result.
fn assert_parse_eq(cc: &CombinedConstruct, data: &[u8]) {
    let c: &dyn Construct = cc;
    let old = c
        .parse_bytes(data)
        .unwrap_or_else(|e| panic!("old parse failed: {e:?}"));
    let compiled = SchemaCompiler::new()
        .compile(cc)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
    let new = compiled
        .parse_bytes(data)
        .unwrap_or_else(|e| panic!("new parse failed: {e:?}"));
    assert_eq!(old, new, "parse mismatch for data={data:?}");
}

/// Compiles `cc` and asserts that both paths produce the same build result.
fn assert_build_eq(cc: &CombinedConstruct, value: &Value) {
    let c: &dyn Construct = cc;
    let old = c
        .build_bytes(value)
        .unwrap_or_else(|e| panic!("old build failed: {e:?}"));
    let compiled = SchemaCompiler::new()
        .compile(cc)
        .unwrap_or_else(|e| panic!("compile failed: {e:?}"));
    let new = compiled
        .build_bytes(value)
        .unwrap_or_else(|e| panic!("new build failed: {e:?}"));
    assert_eq!(old, new, "build mismatch for value={value:?}");
}

/// Runs both parse and build equivalence on the same schema.
fn assert_both_eq(cc: &CombinedConstruct, data: &[u8], value: &Value) {
    assert_parse_eq(cc, data);
    assert_build_eq(cc, value);
}

// ===========================================================================
// 1. FormatField leaves
// ===========================================================================

#[test]
fn eq_format_field_byte() {
    let cc: CombinedConstruct = INT8UB.into();
    assert_both_eq(&cc, &[0x42], &Value::UInt(0x42));
}

#[test]
fn eq_format_field_int32ub() {
    let cc: CombinedConstruct = INT32UB.into();
    assert_both_eq(&cc, &[0x00, 0x00, 0x01, 0x00], &Value::UInt(256));
}

#[test]
fn eq_format_field_int8sb() {
    let cc: CombinedConstruct = INT8SB.into();
    assert_both_eq(&cc, &[0xFF], &Value::Int(-1));
}

// ===========================================================================
// 2. Byte sequences
// ===========================================================================

#[test]
fn eq_bytes_fixed() {
    let cc: CombinedConstruct = Bytes::new(4).into();
    assert_both_eq(
        &cc,
        &[0x01, 0x02, 0x03, 0x04],
        &Value::Bytes(vec![1, 2, 3, 4]),
    );
}

#[test]
fn eq_greedy_bytes() {
    let cc: CombinedConstruct = GreedyBytes::new().into();
    assert_parse_eq(&cc, &[0xAA, 0xBB, 0xCC]);
    // GreedyBytes build needs a Bytes value
    assert_build_eq(&cc, &Value::Bytes(vec![0xAA, 0xBB, 0xCC]));
}

#[test]
fn eq_varint() {
    let cc: CombinedConstruct = VarInt::new().into();
    assert_both_eq(&cc, &[0xAC, 0x02], &Value::UInt(300));
}

// ===========================================================================
// 3. Struct with this-expression dependencies
// ===========================================================================

#[test]
fn eq_struct_with_bytes_expr() {
    // Struct { len: u8, data: Bytes(this.len) }
    let schema = Struct::new().field("len", Box::new(INT8UB.into())).field(
        "data",
        Box::new(BytesExpr::new(Box::new(CombinedExpr::Path(this_().field("len")))).into()),
    );
    let cc: CombinedConstruct = schema.into();
    let data = &[0x03, b'A', b'B', b'C'];
    assert_parse_eq(&cc, data);
    // Build value must contain len + data
    let mut container = IndexMap::new();
    container.insert("len".to_string(), Value::UInt(3));
    container.insert("data".to_string(), Value::Bytes(b"ABC".to_vec()));
    assert_build_eq(&cc, &Value::Container(container));
}

#[test]
fn eq_struct_multi_field() {
    let schema = Struct::new()
        .field("a", Box::new(INT8UB.into()))
        .field("b", Box::new(INT16UB.into()))
        .field("c", Box::new(INT8UB.into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[0x01, 0x00, 0x02, 0x03];
    assert_parse_eq(&cc, data);
    let mut container = IndexMap::new();
    container.insert("a".to_string(), Value::UInt(1));
    container.insert("b".to_string(), Value::UInt(2));
    container.insert("c".to_string(), Value::UInt(3));
    assert_build_eq(&cc, &Value::Container(container));
}

// ===========================================================================
// 4. Sequence (anonymous + named)
// ===========================================================================

#[test]
fn eq_sequence_anonymous() {
    let schema = Sequence::new()
        .push(Box::new(INT8UB.into()))
        .push(Box::new(INT8UB.into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[0x0A, 0x0B];
    assert_parse_eq(&cc, data);
    assert_build_eq(&cc, &Value::List(vec![Value::UInt(10), Value::UInt(11)]));
}

#[test]
fn eq_sequence_named() {
    let schema = Sequence::new().named("first", Box::new(INT8UB.into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[0x2A];
    assert_parse_eq(&cc, data);
}

// ===========================================================================
// 5. Array (fixed + expression length)
// ===========================================================================

#[test]
fn eq_array_fixed() {
    let schema = Array::new(3, Box::new(INT8UB.into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[10, 20, 30];
    assert_parse_eq(&cc, data);
    assert_build_eq(
        &cc,
        &Value::List(vec![Value::UInt(10), Value::UInt(20), Value::UInt(30)]),
    );
}

#[test]
fn eq_array_expr() {
    // Struct { count: u8, items: Array(this.count, u8) }
    let schema = Struct::new().field("count", Box::new(INT8UB.into())).field(
        "items",
        Box::new(
            ArrayExpr::new(
                Box::new(CombinedExpr::Path(this_().field("count"))),
                Box::new(INT8UB.into()),
            )
            .into(),
        ),
    );
    let cc: CombinedConstruct = schema.into();
    let data = &[0x02, 0x11, 0x22];
    assert_parse_eq(&cc, data);
    let mut container = IndexMap::new();
    container.insert("count".to_string(), Value::UInt(2));
    container.insert(
        "items".to_string(),
        Value::List(vec![Value::UInt(0x11), Value::UInt(0x22)]),
    );
    assert_build_eq(&cc, &Value::Container(container));
}

// ===========================================================================
// 6. GreedyRange
// ===========================================================================

#[test]
fn eq_greedy_range() {
    let schema = GreedyRange::new(Box::new(INT8UB.into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[1, 2, 3, 4];
    assert_parse_eq(&cc, data);
    assert_build_eq(
        &cc,
        &Value::List(vec![
            Value::UInt(1),
            Value::UInt(2),
            Value::UInt(3),
            Value::UInt(4),
        ]),
    );
}

// ===========================================================================
// 7. SymmetricAdapter
// ===========================================================================

#[test]
fn eq_symmetric_adapter() {
    let adapter = SymmetricAdapter::new(
        Box::new(INT8UB.into()),
        Box::new(|v, _ctx| {
            let n = v.to_u64()?;
            Ok(Value::UInt(n.wrapping_add(1)))
        }),
    );
    let cc: CombinedConstruct = adapter.into();
    let data = &[0x09];
    assert_parse_eq(&cc, data);
    assert_build_eq(&cc, &Value::UInt(10));
}

// ===========================================================================
// 8. Renamed
// ===========================================================================

#[test]
fn eq_renamed() {
    let renamed = Renamed::new(INT8UB, "my_byte");
    let cc: CombinedConstruct = renamed.into();
    assert_both_eq(&cc, &[0x55], &Value::UInt(0x55));
}

// ===========================================================================
// 9. Enum
// ===========================================================================

#[test]
fn eq_enum() {
    let mut mapping = IndexMap::new();
    mapping.insert("red".to_string(), 0);
    mapping.insert("green".to_string(), 1);
    mapping.insert("blue".to_string(), 2);
    let schema = Enum::new(Box::new(INT8UB.into()), mapping);
    let cc: CombinedConstruct = schema.into();
    // Parse: byte 1 → "green"
    assert_parse_eq(&cc, &[0x01]);
    // Build: "blue" → byte 2
    assert_build_eq(&cc, &Value::String("blue".to_string()));
}

// ===========================================================================
// 10. Switch
// ===========================================================================

#[test]
fn eq_switch() {
    // Struct { type: u8, value: Switch(this.type, [0→u8, 1→u16], default=Pass) }
    let inner_switch = Switch::new(
        Box::new(|ctx: &construct::core::context::Context| {
            Ok(ctx.get("type").cloned().unwrap_or(Value::None))
        }),
        vec![
            (
                Value::UInt(0),
                Box::new(INT8UB.into()) as Box<CombinedConstruct>,
            ),
            (Value::UInt(1), Box::new(INT16UB.into())),
        ],
        Some(Box::new(Pass::new().into())),
    );
    let schema = Struct::new()
        .field("type", Box::new(INT8UB.into()))
        .field("value", Box::new(inner_switch.into()));
    let cc: CombinedConstruct = schema.into();

    // type=0 → value is u8
    let data0 = &[0x00, 0x42];
    assert_parse_eq(&cc, data0);

    // type=1 → value is u16
    let data1 = &[0x01, 0x00, 0x10];
    assert_parse_eq(&cc, data1);
}

// ===========================================================================
// 11. IfThenElse
// ===========================================================================

#[test]
fn eq_if_then_else() {
    // Struct { flag: u8, value: If(flag==1, u16, u8) }
    let ite = IfThenElse::new(
        Box::new(|ctx: &construct::core::context::Context| {
            ctx.get("flag")
                .map(|v| v.as_bool().unwrap_or(false))
                .unwrap_or(false)
        }),
        Box::new(INT16UB.into()),
        Box::new(INT8UB.into()),
    );
    let schema = Struct::new()
        .field("flag", Box::new(INT8UB.into()))
        .field("value", Box::new(ite.into()));
    let cc: CombinedConstruct = schema.into();

    // flag=1 → then-branch (u16)
    let data_then = &[0x01, 0x00, 0x05];
    assert_parse_eq(&cc, data_then);

    // flag=0 → else-branch (u8)
    let data_else = &[0x00, 0x07];
    assert_parse_eq(&cc, data_else);
}

// ===========================================================================
// 12. Pointer
// ===========================================================================

#[test]
fn eq_pointer() {
    // Struct { a: u8, ptr: Pointer(0, u8), b: u8 }
    let schema = Struct::new()
        .field("a", Box::new(INT8UB.into()))
        .field(
            "ptr",
            Box::new(Pointer::new(0, Box::new(INT8UB.into())).into()),
        )
        .field("b", Box::new(INT8UB.into()));
    let cc: CombinedConstruct = schema.into();
    // Byte 0 is read by both "a" and "ptr"; byte 1 by "b"
    let data = &[0xAA, 0xBB];
    assert_parse_eq(&cc, data);
}

// ===========================================================================
// 13. Prefixed
// ===========================================================================

#[test]
fn eq_prefixed() {
    // Prefixed(Int8ub, Bytes) — length byte then that many bytes
    let schema = Prefixed::new(Box::new(INT8UB.into()), Box::new(GreedyBytes::new().into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[0x03, 0x01, 0x02, 0x03];
    assert_parse_eq(&cc, data);
}

// ===========================================================================
// 14. Bitwise
// ===========================================================================

#[test]
fn eq_bitwise_bytes() {
    // Bitwise(Bytes(4)) — reads 4 bits from 1 byte (the rest is padding)
    let schema = Bitwise::new(Box::new(Bytes::new(4).into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[0b10101010];
    assert_parse_eq(&cc, data);
}

// ===========================================================================
// 15. Computed
// ===========================================================================

#[test]
fn eq_computed() {
    // Struct { x: u8, double: Computed(this.x * 2) }
    let schema = Struct::new().field("x", Box::new(INT8UB.into())).field(
        "double",
        Box::new(
            Computed::new(Box::new(|ctx: &construct::core::context::Context| {
                let x = ctx.get("x").and_then(|v| v.to_u64().ok()).unwrap_or(0);
                Ok(Value::UInt(x * 2))
            }))
            .into(),
        ),
    );
    let cc: CombinedConstruct = schema.into();
    let data = &[0x05];
    assert_parse_eq(&cc, data);
}

// ===========================================================================
// 16. Rebuild
// ===========================================================================

#[test]
fn eq_rebuild() {
    // Struct { a: u8, b: Rebuild(u8, this.a) }
    // On parse: b is parsed from stream; on build: b is computed from a
    let schema = Struct::new().field("a", Box::new(INT8UB.into())).field(
        "b",
        Box::new(
            Rebuild::new(
                Box::new(INT8UB.into()),
                Box::new(|ctx: &construct::core::context::Context| {
                    Ok(ctx.get("a").cloned().unwrap_or(Value::UInt(0)))
                }),
            )
            .into(),
        ),
    );
    let cc: CombinedConstruct = schema.into();
    let data = &[0x07, 0x09];
    assert_parse_eq(&cc, data);

    // Build: b is computed from a
    let mut container = IndexMap::new();
    container.insert("a".to_string(), Value::UInt(7));
    // b should be set to a's value (7) during build
    assert_build_eq(&cc, &Value::Container(container));
}

// ===========================================================================
// 17. Default
// ===========================================================================

#[test]
fn eq_default() {
    let schema = DefaultC::new(Box::new(INT8UB.into()), Value::UInt(42));
    let cc: CombinedConstruct = schema.into();
    assert_both_eq(&cc, &[0x10], &Value::UInt(0x10));
}

// ===========================================================================
// 18. Padded
// ===========================================================================

#[test]
fn eq_padded() {
    // Padded(4, u8) — u8 then 3 padding bytes
    let schema = Padded::new_default(4, Box::new(INT8UB.into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[0x41, 0x00, 0x00, 0x00];
    assert_parse_eq(&cc, data);
    assert_build_eq(&cc, &Value::UInt(0x41));
}

// ===========================================================================
// 19. Nested composite: Struct containing Array containing Struct
// ===========================================================================

#[test]
fn eq_nested_struct_array_struct() {
    let inner = Struct::new().field("x", Box::new(INT8UB.into()));
    let arr = Array::new(2, Box::new(inner.into()));
    let schema = Struct::new()
        .field("count", Box::new(INT8UB.into()))
        .field("items", Box::new(arr.into()));
    let cc: CombinedConstruct = schema.into();

    let data = &[0x02, 0x0A, 0x0B];
    assert_parse_eq(&cc, data);

    let mut inner0 = IndexMap::new();
    inner0.insert("x".to_string(), Value::UInt(0x0A));
    let mut inner1 = IndexMap::new();
    inner1.insert("x".to_string(), Value::UInt(0x0B));
    let mut container = IndexMap::new();
    container.insert("count".to_string(), Value::UInt(2));
    container.insert(
        "items".to_string(),
        Value::List(vec![Value::Container(inner0), Value::Container(inner1)]),
    );
    assert_build_eq(&cc, &Value::Container(container));
}

// ===========================================================================
// 20. ExprAdapter wrapping
// ===========================================================================

#[test]
fn eq_expr_adapter() {
    let adapter = ExprAdapter::new(
        Box::new(INT8UB.into()),
        Box::new(|v, _ctx| {
            let n = v.to_u64()?;
            Ok(Value::UInt(n + 100))
        }),
        Box::new(|v, _ctx| {
            let n = v.to_u64()?;
            Ok(Value::UInt(n.saturating_sub(100)))
        }),
    );
    let cc: CombinedConstruct = adapter.into();
    let data = &[0x05];
    assert_parse_eq(&cc, data);
    assert_build_eq(&cc, &Value::UInt(105));
}

// ===========================================================================
// 21. Pass (meta construct)
// ===========================================================================

#[test]
fn eq_pass() {
    let cc: CombinedConstruct = Pass::new().into();
    assert_both_eq(&cc, &[], &Value::None);
}

// ===========================================================================
// 22. Struct with Pass field (flagbuildnone)
// ===========================================================================

#[test]
fn eq_struct_with_pass_field() {
    let schema = Struct::new()
        .field("a", Box::new(INT8UB.into()))
        .field("meta", Box::new(Pass::new().into()));
    let cc: CombinedConstruct = schema.into();
    let data = &[0x33];
    assert_parse_eq(&cc, data);
    let mut container = IndexMap::new();
    container.insert("a".to_string(), Value::UInt(0x33));
    container.insert("meta".to_string(), Value::None);
    assert_build_eq(&cc, &Value::Container(container));
}
