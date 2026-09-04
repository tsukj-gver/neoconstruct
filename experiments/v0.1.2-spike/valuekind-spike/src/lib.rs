//! v0.1.2-1 spike：ValueKind 字段值语义框架的核心机制 A/B 微基准。
//!
//! 目的：证明「ValueKind 编译期分类 + resolve 有效值 + ctx 回写有效值」
//! 相对现状「FieldMode 三分支 + 节点内 None 特判 + ctx 回写原始值」
//! 在热路径 build 上无性能回退（预测：差异 < 噪声；Instance 路径增量
//! = 一次 is_none 指针比较；值提供型路径反而少一次「ctx 写 None 再读」浪费）。
//!
//! 负载字段集（6 字段，模拟 B1-B4 场景）：
//!   x0: RW  int     （Instance）
//!   x1: RW  int     （Instance）
//!   x2: RW  bytes2  （Instance）
//!   c : RW  Const   （ValueKind::Const —— B1 场景：实例化隐式 default=None）
//!   d : RW  Default （ValueKind::Default）
//!   r : RO  Rebuild （ValueKind::Derived —— RO，不从实例取值）
//!
//! 两版循环的节点级代码完全相同（公平对比），差异仅在字段循环形态：
//!   v0（现状形态）：match field.mode { Rw => { getattr; ctx 写原始值; node.build }
//!                                        Ro => { compute; ctx 写; node.build } }
//!                  节点内 None 特判（ConstNode/DefaultNode 自己处理 None）。
//!   v1（框架形态）：match field.kind { Instance => getattr + None 检查
//!                                   Const(c) => getattr + None→c 补值
//!                                   Default(d) => getattr + None→d
//!                                   Derived => 忽略 attr，从 ctx 槽位求值 }
//!                  resolve 出**有效值**统一写 ctx（修复 B1e：None 不入 ctx），
//!                  节点收到的恒为有效值（节点 None 分支保留但不再触发）。
//!
//! §0 合规：一次 FFI 入口（pyfunction）；Rust 内全为 C API 进程内调用；
//! 无中间表示（PyObject 直达）；无 trait 抽象（封闭 enum + match 静态分派）。

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyString};

// ---------------------------------------------------------------------------
// 节点级构建逻辑（v0/v1 完全一致，保证 A/B 只测循环形态差异）
// ---------------------------------------------------------------------------

/// 模拟 FormatFieldNode.build：u8 写入。
#[inline]
fn node_build_u8(value: &Bound<'_, PyAny>, out: &mut Vec<u8>) -> PyResult<()> {
    out.push(value.extract()?);
    Ok(())
}

/// 模拟 BytesNode.build：bytes 写入。
#[inline]
fn node_build_bytes2(value: &Bound<'_, PyAny>, out: &mut Vec<u8>) -> PyResult<()> {
    out.extend_from_slice(&value.extract::<Vec<u8>>()?);
    Ok(())
}

/// 模拟 ConstNode.build：None → const；非 None → 相等用之，不等报错。
/// （v0 中该分支会被触发；v1 中 resolve 已补值，None 分支不触发但保留。）
#[inline]
fn node_build_const_u8(value: &Bound<'_, PyAny>, const_v: u8, out: &mut Vec<u8>) -> PyResult<()> {
    if value.is_none() {
        out.push(const_v);
        return Ok(());
    }
    let v: u8 = value.extract()?;
    if v == const_v {
        out.push(const_v);
        Ok(())
    } else {
        Err(pyo3::exceptions::PyValueError::new_err("const mismatch"))
    }
}

/// 模拟 DefaultNode.build：None → 默认值；非 None → 用显式值。
#[inline]
fn node_build_default_u8(value: &Bound<'_, PyAny>, default_v: u8, out: &mut Vec<u8>) -> PyResult<()> {
    if value.is_none() {
        out.push(default_v);
        Ok(())
    } else {
        node_build_u8(value, out)
    }
}

// ---------------------------------------------------------------------------
// ctx 模拟：PyDict（fields）+ Vec 槽位（expr_values_buf）
// ---------------------------------------------------------------------------

struct CtxSlots {
    dict: Py<PyDict>,
    slots: Vec<Option<Py<PyAny>>>,
}

impl CtxSlots {
    fn new(py: Python<'_>, n: usize) -> Self {
        Self {
            dict: PyDict::new_bound(py).unbind(),
            slots: (0..n).map(|_| None).collect(),
        }
    }

    /// 模拟 ctx.set_field_at：dict set_item + 槽位写入。
    #[inline]
    fn set_field_at(
        &mut self,
        py: Python<'_>,
        idx: usize,
        key: &Bound<'_, PyString>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.dict.bind(py).set_item(key, value)?;
        self.slots[idx] = Some(value.clone().unbind());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 字段表
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Rw,
    Ro,
}

/// 值语义分类（框架形态，v0.1.2-1 设计核心）。
enum ValueKind {
    /// 值必须来自实例；None → BuildValueMissing（对齐 construct KeyError）。
    Instance,
    /// 常量：None → 补 const；ctx 写有效值。
    Const(u8),
    /// 默认值：None → 用默认；ctx 写有效值。
    Default(u8),
    /// 派生（Rebuild/Computed）：忽略实例值，从 ctx 槽位求值（x0*2）。
    Derived,
}

struct Field {
    name: &'static str,
    mode: Mode,
    kind: ValueKind,
}

const N_FIELDS: usize = 6;

fn field_table() -> [Field; N_FIELDS] {
    [
        Field { name: "x0", mode: Mode::Rw, kind: ValueKind::Instance },
        Field { name: "x1", mode: Mode::Rw, kind: ValueKind::Instance },
        Field { name: "x2", mode: Mode::Rw, kind: ValueKind::Instance },
        Field { name: "c", mode: Mode::Rw, kind: ValueKind::Const(0x05) },
        Field { name: "d", mode: Mode::Rw, kind: ValueKind::Default(7) },
        Field { name: "r", mode: Mode::Ro, kind: ValueKind::Derived },
    ]
}

fn interned_names(py: Python<'_>) -> [Py<PyString>; N_FIELDS] {
    let t = field_table();
    [
        PyString::new_bound(py, t[0].name).unbind(),
        PyString::new_bound(py, t[1].name).unbind(),
        PyString::new_bound(py, t[2].name).unbind(),
        PyString::new_bound(py, t[3].name).unbind(),
        PyString::new_bound(py, t[4].name).unbind(),
        PyString::new_bound(py, t[5].name).unbind(),
    ]
}

// ---------------------------------------------------------------------------
// v0：现状形态
// ---------------------------------------------------------------------------

#[pyfunction]
fn build_v0(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Py<PyBytes>> {
    let fields = field_table();
    let names = interned_names(py);
    let mut ctx = CtxSlots::new(py, N_FIELDS);
    let mut out: Vec<u8> = Vec::with_capacity(8);

    for (idx, f) in fields.iter().enumerate() {
        let name = names[idx].bind(py);
        match f.mode {
            Mode::Rw => {
                let value = obj.getattr(name)?;
                // 现状缺陷形态：ctx 写原始 getattr 值（None 会流入表达式 → B1e）
                ctx.set_field_at(py, idx, name, &value)?;
                match idx {
                    0 | 1 => node_build_u8(&value, &mut out)?,
                    2 => node_build_bytes2(&value, &mut out)?,
                    3 => node_build_const_u8(&value, 0x05, &mut out)?,
                    4 => node_build_default_u8(&value, 7, &mut out)?,
                    _ => unreachable!(),
                }
            }
            Mode::Ro => {
                // 现状 RO：compute_ro_value（节点类型分派散落在 mod.rs —— B1f 缺 Const 分支的形态）
                let v0 = ctx.slots[0]
                    .as_ref()
                    .map(|p| p.bind(py).extract::<u8>().unwrap_or(0))
                    .unwrap_or(0);
                let derived: u8 = v0.wrapping_mul(2);
                let value = derived.into_py(py);
                let value_b = value.bind(py);
                ctx.set_field_at(py, idx, name, value_b)?;
                node_build_u8(value_b, &mut out)?;
            }
        }
    }
    Ok(PyBytes::new_bound(py, &out).unbind())
}

// ---------------------------------------------------------------------------
// v1：框架形态（就地替换式 resolve：零 owned 中转，设计规定的实现形态）
// ---------------------------------------------------------------------------

/// 编译期常量对象缓存（模拟 ValueKind::Const(Py<PyAny>) 持有 Python 常量对象，
/// 类定义时创建一次；build 时仅在 None 补值分支做一次 Bound clone）。
fn const_ref(py: Python<'_>, idx: usize) -> &'static Py<PyAny> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<[Py<PyAny>; 2]> = OnceLock::new();
    let arr: &'static [Py<PyAny>; 2] =
        CACHE.get_or_init(|| [5u8.into_py(py), 7u8.into_py(py)]);
    &arr[idx]
}

#[pyfunction]
fn build_v1(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Py<PyBytes>> {
    let fields = field_table();
    let names = interned_names(py);
        let mut ctx = CtxSlots::new(py, N_FIELDS);
    let mut out: Vec<u8> = Vec::with_capacity(8);

    for (idx, f) in fields.iter().enumerate() {
        let name = names[idx].bind(py);
        // resolve（就地替换式）：value 恒为 Bound，无 owned Py<PyAny> 中转。
        // 差异 vs v0 = 每字段一次 kind match（与 mode match 同形态）+ Instance
        // 路径一次 is_none 指针比较；值提供型路径的 None 补值仅一次 Bound clone。
        let value: Bound<'_, PyAny> = match &f.kind {
            ValueKind::Instance => {
                let v = obj.getattr(name)?;
                if v.is_none() {
                    return Err(pyo3::exceptions::PyValueError::new_err(format!(
                        "field '{}' requires a value for build",
                        f.name
                    )));
                }
                v
            }
            ValueKind::Const(_) => {
                let v = obj.getattr(name)?;
                if v.is_none() {
                    const_ref(py, 0).bind(py).clone()
                } else {
                    v
                }
            }
            ValueKind::Default(_) => {
                let v = obj.getattr(name)?;
                if v.is_none() {
                    const_ref(py, 1).bind(py).clone()
                } else {
                    v
                }
            }
            ValueKind::Derived => {
                let v0 = ctx.slots[0]
                    .as_ref()
                    .map(|p| p.bind(py).extract::<u8>().unwrap_or(0))
                    .unwrap_or(0);
                v0.wrapping_mul(2).into_py(py).into_bound(py)
            }
        };
        // 框架核心修复：ctx 写「有效值」（B1e 的机制解）
        ctx.set_field_at(py, idx, name, &value)?;
        match idx {
            0 | 1 => node_build_u8(&value, &mut out)?,
            2 => node_build_bytes2(&value, &mut out)?,
            3 => node_build_const_u8(&value, 0x05, &mut out)?,
            4 => node_build_default_u8(&value, 7, &mut out)?,
            5 => node_build_u8(&value, &mut out)?,
            _ => unreachable!(),
        }
    }
    Ok(PyBytes::new_bound(py, &out).unbind())
}

// ---------------------------------------------------------------------------
// 机制验证：v1 ctx 写有效值（B1e 修复证明）+ v0 对照（缺陷形态证明）
// ---------------------------------------------------------------------------

#[pyfunction]
fn probe_ctx_v0(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Py<PyDict>> {
    // 运行 v0 循环并返回 ctx dict：'c' 槽位 = None（缺陷形态）
    run_with_ctx(py, obj, false)
}

#[pyfunction]
fn probe_ctx_v1(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Py<PyDict>> {
    // 运行 v1 循环并返回 ctx dict：'c' 槽位 = 5（有效值，B1e 修复）
    run_with_ctx(py, obj, true)
}

fn run_with_ctx(py: Python<'_>, obj: &Bound<'_, PyAny>, v1: bool) -> PyResult<Py<PyDict>> {
    let fields = field_table();
    let names = interned_names(py);
    let mut ctx = CtxSlots::new(py, N_FIELDS);
    for (idx, f) in fields.iter().enumerate() {
        let name = names[idx].bind(py);
        if v1 {
            let effective: Py<PyAny> = match &f.kind {
                ValueKind::Instance => obj.getattr(name)?.into_any().unbind(),
                ValueKind::Const(c) => {
                    let v = obj.getattr(name)?;
                    if v.is_none() { (*c).into_py(py) } else { v.into_any().unbind() }
                }
                ValueKind::Default(d) => {
                    let v = obj.getattr(name)?;
                    if v.is_none() { (*d).into_py(py) } else { v.into_any().unbind() }
                }
                ValueKind::Derived => {
                    let v0 = ctx.slots[0]
                        .as_ref()
                        .map(|p| p.bind(py).extract::<u8>().unwrap_or(0))
                        .unwrap_or(0);
                    v0.wrapping_mul(2).into_py(py)
                }
            };
            ctx.set_field_at(py, idx, name, effective.bind(py))?;
        } else {
            match f.mode {
                Mode::Rw => {
                    let value = obj.getattr(name)?;
                    ctx.set_field_at(py, idx, name, &value)?;
                }
                Mode::Ro => {
                    let v0 = ctx.slots[0]
                        .as_ref()
                        .map(|p| p.bind(py).extract::<u8>().unwrap_or(0))
                        .unwrap_or(0);
                    let value = v0.wrapping_mul(2).into_py(py);
                    ctx.set_field_at(py, idx, name, value.bind(py))?;
                }
            }
        }
    }
    Ok(ctx.dict)
}

#[pymodule]
fn valuekind_spike(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(build_v0, m)?)?;
    m.add_function(wrap_pyfunction!(build_v1, m)?)?;
    m.add_function(wrap_pyfunction!(probe_ctx_v0, m)?)?;
    m.add_function(wrap_pyfunction!(probe_ctx_v1, m)?)?;
    Ok(())
}
