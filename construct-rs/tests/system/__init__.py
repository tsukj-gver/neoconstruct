"""Phase 9 系统测试：真实二进制协议端到端验证。

每个协议一个测试文件：
- test_modbus_rtu.py  — Modbus RTU（功能码 Switch + 寄存器数据 + CRC16 校验）
- test_can_frame.py   — CAN 2.0A/B（BitStruct ID + 标志位 + 条件分支）
- test_iec104.py      — IEC 60870-5-104（APCI 帧类型 Switch + 嵌套 + ASDU）
- test_ipv4.py        — IPv4 报文头（BitStruct 版本/IHL/标志 + Checksum）

每个文件包含：
1. 协议定义（@dataclass StructMixin 子类，仅用已实现的构造器）
2. parse 测试（构造真实/合理报文字节，parse 后验证字段）
3. build 测试（build 回字节，验证 round-trip 一致）
4. parity 测试（vs Python construct 2.10.70，子进程隔离）
5. 复杂场景（条件分支、bitfield ID、CRC/校验）

API gap（若发现）记录在过程记录 `plans/phase9-system-test/traces/9.1-DEV系统测试.md`。
"""
