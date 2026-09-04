# Bench Report

- **Timestamp**: 2026-09-04T20:51:15
- **Python**: 3.13.15
- **Platform**: Windows-11-10.0.26200-SP0
- **number**: 30000, iterations: 20

## Results

场景           impl  median ns    mean ± stdev           min        max        speedup   
---------------------------------------------------------------------------------------
B1-build     rs    607.1        617.9 ± 178.9          379.9      930.3      31.05x    
B1-build     py    18847.9      17325.5 ± 3856.4       8533.7     20669.3              
B1-parse     rs    677.7        790.6 ± 221.8          529.8      1138.8     26.82x    
B1-parse     py    18178.1      17433.6 ± 2720.7       10187.6    20136.4              

## Summary

- speedup median: 31.04535434747316
- speedup min: 26.821454038208074
- speedup max: 31.04535434747316
- failed gates: none