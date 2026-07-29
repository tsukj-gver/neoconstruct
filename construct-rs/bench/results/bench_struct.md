# Bench Report

- **Timestamp**: 2026-07-30T00:47:03
- **Python**: 3.14.2
- **Platform**: Windows-11-10.0.26200-SP0
- **number**: 10000, iterations: 3

## Results

场景           impl  median ns    mean ± stdev           min        max        speedup   
---------------------------------------------------------------------------------------
B1-build     rs    195.5        196.5 ± 2.2            195.1      199.8      14.18x    
B1-build     py    2772.0       2772.8 ± 33.4          2733.6     2813.6               
B1-parse     rs    277.8        278.3 ± 1.6            277.0      280.7      10.42x    
B1-parse     py    2894.8       2896.4 ± 44.2          2850.3     2945.8               

## Summary

- speedup median: 14.180218448675243
- speedup min: 10.421982932095638
- speedup max: 14.180218448675243
- failed gates: none