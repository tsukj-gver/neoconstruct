# Bench Report

- **Timestamp**: 2026-07-30T10:55:52
- **Python**: 3.14.2
- **Platform**: Windows-11-10.0.26200-SP0
- **number**: 5000, iterations: 5

## Results

场景           impl  median ns    mean ± stdev           min        max        speedup   
---------------------------------------------------------------------------------------
B1-build     rs    197.1        197.2 ± 0.5            196.5      197.7      13.36x    
B1-build     py    2633.4       2632.4 ± 65.3          2526.6     2697.5               
B1-parse     rs    275.9        276.1 ± 0.8            275.1      277.0      10.25x    
B1-parse     py    2827.8       2811.4 ± 90.1          2698.5     2913.7               
B4-build     rs    2330.3       2349.4 ± 40.9          2307.5     2395.7     16.38x    
B4-build     py    38159.2      38079.8 ± 253.4        37774.4    38417.8              
B4-parse     rs    3384.9       3426.3 ± 75.7          3367.2     3550.3     12.73x    
B4-parse     py    43098.1      43098.7 ± 119.6        42955.5    43277.5              

## Summary

- speedup median: 13.35947611807237
- speedup min: 10.250108770133604
- speedup max: 16.37538839172211
- failed gates: none