# Bench Report

- **Timestamp**: 2026-07-31T20:48:12
- **Python**: 3.14.2
- **Platform**: Windows-11-10.0.26200-SP0
- **number**: 5000, iterations: 10

## Results

场景           impl  median ns    mean ± stdev           min        max        speedup   
---------------------------------------------------------------------------------------
DF1-build    rs    207.2        225.2 ± 36.0           205.6      295.6      9.75x     
DF1-build    py    2020.5       1999.1 ± 82.8          1898.9     2102.5               
DF1-parse    rs    230.2        237.5 ± 24.7           227.5      307.5      9.08x     
DF1-parse    py    2090.8       2097.5 ± 86.8          1986.1     2206.8               
DF2-build    rs    259.0        271.6 ± 32.4           248.7      346.5      10.52x    
DF2-build    py    2725.3       2735.9 ± 105.1         2567.8     2877.5               
DF2-parse    rs    255.9        272.6 ± 36.5           253.5      356.1      10.34x    
DF2-parse    py    2645.4       2624.9 ± 70.2          2493.3     2691.6               
TM1-build    rs    165.0        167.0 ± 10.1           160.7      195.1      13.15x    
TM1-build    py    2169.7       2138.5 ± 96.1          1988.9     2284.5               
TM1-parse    rs    254.7        268.3 ± 45.7           249.0      398.2      9.23x     
TM1-parse    py    2350.0       2329.7 ± 67.7          2187.0     2402.8               

## Summary

- speedup median: 10.337344971013763
- speedup min: 9.084071907097782
- speedup max: 13.15220959415531
- failed gates: none