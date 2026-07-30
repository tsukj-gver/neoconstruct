# Bench Report

- **Timestamp**: 2026-07-30T19:46:07
- **Python**: 3.14.2
- **Platform**: Windows-11-10.0.26200-SP0
- **number**: 5000, iterations: 10

## Results

场景           impl  median ns    mean ± stdev           min        max        speedup   
---------------------------------------------------------------------------------------
B5-build     rs    177.7        178.3 ± 2.3            175.8      184.4      12.02x    
B5-build     py    2137.2       2143.5 ± 37.8          2098.7     2209.9               
B5-parse     rs    243.3        244.9 ± 4.5            240.8      256.5      9.01x     
B5-parse     py    2191.0       2179.9 ± 34.2          2121.0     2233.2               

## Summary

- speedup median: 12.024756661732187
- speedup min: 9.00550788409481
- speedup max: 12.024756661732187
- failed gates: none