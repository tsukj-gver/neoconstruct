// 组 3-6 的测量函数（通过 include! 合并到 main.rs）。

// ---------------------------------------------------------------------------
// 组 3：create_class (tp_new) 成本
// ---------------------------------------------------------------------------

fn bench_struct_overhead(py: Python<'_>) {
    print_header("组3: create_class (tp_new) 成本 — parse 独有");

    // 简单 object 子类（最小 tp_new 路径）
    let cls_object: Py<PyType> = py
        .eval_bound("type('M', (), {})", None, None)
        .unwrap()
        .extract()
        .unwrap();
    let s_tp_new_object: Sample = bench(|| {
        let inst = create_class(cls_object.bind(py)).unwrap();
        std::hint::black_box(inst);
    });
    print_row("tp_new(object 子类)", s_tp_new_object);

    // @dataclass 装饰类（StructMixin 风格）
    let code = concat!(
        "from dataclasses import dataclass\n",
        "@dataclass\n",
        "class D:\n",
        "    x: int = 0\n",
        "    y: int = 0\n",
    );
    let globals = pyo3::types::PyDict::new_bound(py);
    py.run_bound(code, Some(&globals), None).unwrap();
    let cls_dc: Py<PyType> = globals
        .get_item("D")
        .unwrap()
        .unwrap()
        .extract()
        .unwrap();
    let s_tp_new_dc: Sample = bench(|| {
        let inst = create_class(cls_dc.bind(py)).unwrap();
        std::hint::black_box(inst);
    });
    print_row("tp_new(@dataclass 类)", s_tp_new_dc);

    // -----------------------------------------------------------------
    // 组 4：PyBytes::new_bound 成本 — build 独有
    // -----------------------------------------------------------------

    println!();
    println!("=== 组4: PyBytes::new_bound 成本 — build 独有 ===");
    println!(
        "{:<40}  {:>10}  {:>10}",
        "operation", "median(ns)", "min(ns)"
    );
    println!("{}", "-".repeat(64));

    let s_bytes_1: Sample = bench(|| {
        let b = PyBytes::new_bound(py, &[0x42u8]);
        std::hint::black_box(b);
    });
    print_row("PyBytes::new_bound(1 byte)", s_bytes_1);

    let s_bytes_8: Sample = bench(|| {
        let b = PyBytes::new_bound(py, &[0u8; 8]);
        std::hint::black_box(b);
    });
    print_row("PyBytes::new_bound(8 bytes)", s_bytes_8);

    // PyBytes.as_bytes() — parse 入口（提取输入字节）
    let input_bytes: Py<PyBytes> = PyBytes::new_bound(py, &[0u8; 8]).into();
    let ib = input_bytes.bind(py);
    let s_as_bytes: Sample = bench(|| {
        let s: &[u8] = ib.as_bytes();
        std::hint::black_box(s);
    });
    print_row("PyBytes.as_bytes(8 bytes)", s_as_bytes);

    // BuildStream::with_capacity + into_bytes — build 端 stream 生命周期
    let s_stream_cycle: Sample = bench(|| {
        let mut stream = BuildStream::with_capacity(8);
        stream.write(&[0u8; 8]);
        let v = stream.into_bytes();
        std::hint::black_box(v);
    });
    print_row("BuildStream::with_capacity(8) + write + into_bytes", s_stream_cycle);
}

// ---------------------------------------------------------------------------
// 组 5：getattr("__dict__") + downcast + dict.set_item 成本
// ---------------------------------------------------------------------------

fn bench_pyybytes_getattr(py: Python<'_>) {
    print_header("组5: getattr / dict 操作成本");

    let cls: Py<PyType> = py
        .eval_bound("type('M', (), {})", None, None)
        .unwrap()
        .extract()
        .unwrap();
    let inst = create_class(cls.bind(py)).unwrap();
    let inst_b = inst.clone();

    // getattr("__dict__")（每 parse 一次）
    let dict_name = pyo3::types::PyString::new_bound(py, "__dict__");
    let s_getattr_dict: Sample = bench(|| {
        let d = inst_b.getattr(&dict_name).unwrap();
        std::hint::black_box(d);
    });
    print_row("getattr('__dict__')", s_getattr_dict);

    // getattr("__dict__") + downcast PyDict
    let s_getattr_dict_downcast: Sample = bench(|| {
        let d_binding = inst_b.getattr(&dict_name).unwrap();
        let d = d_binding.downcast::<PyDict>().unwrap();
        std::hint::black_box(d);
    });
    print_row("getattr('__dict__') + downcast::<PyDict>", s_getattr_dict_downcast);

    // dict.set_item（每字段 parse）
    let dict = {
        let d_binding = inst_b.getattr("__dict__").unwrap();
        d_binding.downcast::<PyDict>().unwrap().clone()
    };
    let key = pyo3::types::PyString::new_bound(py, "field_a");
    let value: Py<PyAny> = 42i64.into_py(py);
    let vb = value.bind(py);
    let s_dict_setitem: Sample = bench(|| {
        dict.set_item(&key, vb).unwrap();
        std::hint::black_box(());
    });
    print_row("dict.set_item(interned key, i64 value)", s_dict_setitem);

    // getattr(field_name)（每字段 build）— inst.field_a
    let cls_with_field: Py<PyType> = py
        .eval_bound("type('M', (), {'field_a': 42})", None, None)
        .unwrap()
        .extract()
        .unwrap();
    let inst_wf = create_class(cls_with_field.bind(py)).unwrap();
    // 注：tp_new 创建的实例不会自动有 field_a（那是类属性）。手工 setattr。
    inst_wf.setattr("field_a", 42i64).unwrap();
    let inst_wf_b = &inst_wf;
    let field_name = pyo3::types::PyString::new_bound(py, "field_a");
    let s_getattr_field: Sample = bench(|| {
        let v = inst_wf_b.getattr(&field_name).unwrap();
        std::hint::black_box(v);
    });
    print_row("getattr(interned field name)", s_getattr_field);

    // getattr + extract<i64>（完整 build 字段读取）
    let s_getattr_extract_i64: Sample = bench(|| {
        let v = inst_wf_b.getattr(&field_name).unwrap();
        let n: i64 = v.extract().unwrap();
        std::hint::black_box(n);
    });
    print_row("getattr + extract::<i64>", s_getattr_extract_i64);

    // getattr + extract<i128>（BitsInteger build 路径）
    let s_getattr_extract_i128: Sample = bench(|| {
        let v = inst_wf_b.getattr(&field_name).unwrap();
        let n: i128 = v.extract().unwrap();
        std::hint::black_box(n);
    });
    print_row("getattr + extract::<i128>", s_getattr_extract_i128);
}

// ---------------------------------------------------------------------------
// 组 6：BitsIntegerNode.parse vs build 对比（验证 i128 增量）
// ---------------------------------------------------------------------------

fn bench_bitsinteger_compare(py: Python<'_>) {
    print_header("组6: BitsIntegerNode 裸节点 parse vs build (length=8 unsigned)");

    let data = [0x42u8];
    let node_parse = BitsIntegerNode::new(8, false, false);
    let node_build = BitsIntegerNode::new(8, false, false);
    let mut ctx_p = Context::placeholder(py);
    let mut ctx_b = Context::placeholder(py);
    let mut path = Path::new();

    let s_parse: Sample = bench(|| {
        let mut stream = ParseStream::new(&data);
        let v = node_parse
            .parse(py, &mut stream, &mut ctx_p, &mut path)
            .unwrap();
        std::hint::black_box(v);
    });
    print_row("BitsInteger(8,unsigned).parse [裸]", s_parse);

    let value_obj: Py<PyAny> = 0x42i64.into_py(py);
    let vobj = value_obj.bind(py);
    let s_build: Sample = bench(|| {
        let mut stream = BuildStream::with_capacity(8);
        node_build
            .build(py, vobj, &mut stream, &mut ctx_b, &mut path)
            .unwrap();
        std::hint::black_box(stream.bit_pos());
    });
    print_row("BitsInteger(8,unsigned).build [裸,prealloc]", s_build);

    // FormatField 对照（Phase 1 等价字段）
    println!();
    println!("=== 组6b: FormatField(Int8ub) 裸节点 parse vs build 对照 ===");
    println!(
        "{:<40}  {:>10}  {:>10}",
        "operation", "median(ns)", "min(ns)"
    );
    println!("{}", "-".repeat(64));

    let ff_parse = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
    let ff_build = FormatFieldNode::new(PythonFormat::UnsignedInt8Big);
    let s_ff_parse: Sample = bench(|| {
        let mut stream = ParseStream::new(&data);
        let v = ff_parse
            .parse(py, &mut stream, &mut ctx_p, &mut path)
            .unwrap();
        std::hint::black_box(v);
    });
    print_row("FormatField(Int8ub).parse [裸]", s_ff_parse);

    let s_ff_build: Sample = bench(|| {
        let mut stream = BuildStream::with_capacity(8);
        ff_build
            .build(py, vobj, &mut stream, &mut ctx_b, &mut path)
            .unwrap();
        std::hint::black_box(stream.bit_pos());
    });
    print_row("FormatField(Int8ub).build [裸,prealloc]", s_ff_build);

    // -----------------------------------------------------------------
    // 组 7：StructNode 不同字段数下 parse vs build（隔离 tp_new 贡献）
    // -----------------------------------------------------------------

    println!();
    println!("=== 组7: StructNode parse vs build vs 字段数 (FormatField) ===");
    println!(
        "{:<36}  {:>10}  {:>10}  {:>9}",
        "operation", "median(ns)", "min(ns)", "P-B(ns)"
    );
    println!("{}", "-".repeat(70));

    bench_struct_n(py, 0);
    bench_struct_n(py, 1);
    bench_struct_n(py, 2);
    bench_struct_n(py, 3);
    bench_struct_n(py, 6);
    bench_struct_n(py, 10);

    // BitStruct（BitwiseNode + StructNode + BitsInteger 字段）
    println!();
    println!("=== 组7b: BitwiseNode + StructNode parse vs build (BitsInteger) ===");
    println!(
        "{:<36}  {:>10}  {:>10}  {:>9}",
        "operation", "median(ns)", "min(ns)", "P-B(ns)"
    );
    println!("{}", "-".repeat(70));

    bench_bitstruct_n(py, 1);
    bench_bitstruct_n(py, 2);
    bench_bitstruct_n(py, 3);
}

/// 构造一个含 N 个 Int8ub 字段的 StructNode 并测 parse/build。
fn bench_struct_n(py: Python<'_>, n: usize) {
    let cls: Py<PyType> = py
        .eval_bound("type('M', (), {})", None, None)
        .unwrap()
        .extract()
        .unwrap();
    let mut fields = Vec::with_capacity(n);
    for i in 0..n {
        let name = format!("f{}", i);
        fields.push(StructField {
            name: FieldName::new(py, name),
            node: Node::FormatField(FormatFieldNode::new(PythonFormat::UnsignedInt8Big)),
            mode: FieldMode::Rw,
        });
    }
    let node = StructNode::new(py, fields, cls, false, false);

    let data = vec![0x42u8; n.max(1)];
    let mut ctx_p = Context::placeholder(py);
    let mut ctx_b = Context::placeholder(py);
    let mut path = Path::new();

    let s_parse: Sample = bench(|| {
        let mut stream = ParseStream::new(&data);
        let v = node.parse(py, &mut stream, &mut ctx_p, &mut path).unwrap();
        std::hint::black_box(v);
    });

    // build: 构造一个含 N 个字段的对象
    let obj_code = format!(
        "type('O', (), {{{}}})()",
        (0..n)
            .map(|i| format!("'f{}': 42", i))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let obj = py.eval_bound(&obj_code, None, None).unwrap();

    let cap = n.max(1);
    let s_build: Sample = bench(|| {
        let mut stream = BuildStream::with_capacity(cap);
        node.build(py, &obj, &mut stream, &mut ctx_b, &mut path)
            .unwrap();
        std::hint::black_box(stream.bit_pos());
    });

    let diff = s_parse.median_ns - s_build.median_ns;
    println!(
        "{:<36}  {:>10.2}  {:>10.2}  {:>+9.2}",
        format!("Struct({} Int8ub)", n),
        s_parse.median_ns,
        s_parse.min_ns,
        diff
    );
    println!(
        "{:<36}  {:>10.2}  {:>10.2}",
        format!("        build",),
        s_build.median_ns,
        s_build.min_ns
    );
}

/// 构造一个含 N 个 BitsInteger 字段的 BitStruct 并测 parse/build。
fn bench_bitstruct_n(py: Python<'_>, n: usize) {
    let cls: Py<PyType> = py
        .eval_bound("type('M', (), {})", None, None)
        .unwrap()
        .extract()
        .unwrap();
    let mut fields = Vec::with_capacity(n);
    for i in 0..n {
        let name = format!("b{}", i);
        fields.push(StructField {
            name: FieldName::new(py, name),
            node: Node::BitsInteger(BitsIntegerNode::new(8, false, false)),
            mode: FieldMode::Rw,
        });
    }
    let inner = StructNode::new(py, fields, cls, false, false);
    let node = construct_rust::nodes::bitwise::BitwiseNode::new(Node::Struct(inner));

    let data = vec![0x42u8; n.max(1)];
    let mut ctx_p = Context::placeholder(py);
    let mut ctx_b = Context::placeholder(py);
    let mut path = Path::new();

    let s_parse: Sample = bench(|| {
        let mut stream = ParseStream::new(&data);
        let v = node.parse(py, &mut stream, &mut ctx_p, &mut path).unwrap();
        std::hint::black_box(v);
    });

    let obj_code = format!(
        "type('O', (), {{{}}})()",
        (0..n)
            .map(|i| format!("'b{}': 42", i))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let obj = py.eval_bound(&obj_code, None, None).unwrap();

    let cap = n.max(1);
    let s_build: Sample = bench(|| {
        let mut stream = BuildStream::with_capacity(cap);
        node.build(py, &obj, &mut stream, &mut ctx_b, &mut path)
            .unwrap();
        std::hint::black_box(stream.bit_pos());
    });

    let diff = s_parse.median_ns - s_build.median_ns;
    println!(
        "{:<36}  {:>10.2}  {:>10.2}  {:>+9.2}",
        format!("Bitwise(Struct({} BitsInteger8))", n),
        s_parse.median_ns,
        s_parse.min_ns,
        diff
    );
    println!(
        "{:<36}  {:>10.2}  {:>10.2}",
        format!("        build",),
        s_build.median_ns,
        s_build.min_ns
    );
}

// ---------------------------------------------------------------------------
// 组 8：分析摘要
// ---------------------------------------------------------------------------

fn bench_summary() {
    println!();
    println!("============================================================");
    println!("分析摘要（详见 docs/分析-phase3-parse-build不对称.md）");
    println!("============================================================");
    println!("模型：parse-build 差距 = StructNode 固有不对称 + 字段级差");
    println!();
    println!("验证项：");
    println!("1. 组1 i128::into_py 与 i64::into_py 成本差（应较小，pyo3 内部优化）");
    println!("2. 组2 extract<i128> 与 extract<i64> 成本差（应较小，读取不需 alloc）");
    println!("3. 组3 tp_new 成本（StructNode parse 独有，~50-80ns）");
    println!("4. 组4 PyBytes::new_bound 成本（build 独有，~30ns）");
    println!("5. 组5 getattr + dict.set_item（parse 独有，~20-30ns）");
    println!("6. 组6 BitsIntegerNode 裸节点的 parse/build 差（应 < 30ns）");
    println!("7. 组7 StructNode 字段数从 0 → 10 时 parse-build 差距变化");
    println!("   预期：0 字段差距 ≈ tp_new + getattr(__dict__) - PyBytes");
    println!("   每增加 1 个 FormatField 字段差距 += (PyLong 创建 - extract - getattr)");
    println!("8. 组7b BitStruct 同上，用 BitsInteger 字段（含 i128 路径）");
}
