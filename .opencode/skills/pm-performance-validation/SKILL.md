---
name: pm-performance-validation
description: Use when the PM receives or reviews any performance data — benchmark results, speedup ratios, profiling output, complexity measurements. The PM validates logical consistency of data against known constraints before dispatching optimization work.
---

# PM Performance Data Validation

## Core Principle

The PM's job in performance analysis is **not** to understand how costs are composed
(that's ARCH/DEV's job). The PM judges whether the data is **logically self-consistent**
— does it match what must be true given the technology, the problem structure, and
known constraints?

## How to Apply

When facing any performance data, ask: **Does anything I know to be true contradict
this data?**

Sources of logical constraints (non-exhaustive — any knowledge that creates a
constraint can serve):

- **Technology fundamentals**: What speedup range does this tech stack typically
  deliver? Do comparable projects achieve similar numbers? If Rust-core Python
  libraries typically hit 4-10x+, then 2x signals a structural problem.

- **Internal data relationships**: Are ratios between related operations as
  expected? Symmetric operations should have symmetric costs. Scenarios differing
  only in input size should show cost differences matching expected complexity.

- **Complexity consistency**: Does cost scale with input size as expected? O(n)
  expected but superlinear observed = design problem. Fixed cost should not
  depend on variable input parameters.

- **Cross-version progression**: A new version doing strictly more computation
  should have a higher performance ceiling (Rust advantage amplifies with more
  work). If performance drops, something structural was introduced.

- **Cross-system comparison**: How do numbers compare to reference implementations
  or industry benchmarks?

These are not a checklist. The key question is always: **what must be true, and
does the data satisfy it?**

## When Inconsistency Is Found

1. **Do NOT dispatch optimization tasks** — you don't know where the problem is
2. **Dispatch an investigation task** — ask for the root cause of the contradiction
3. **Evaluate the root cause** before deciding whether to optimize

## Historical Cases

| Case | Logical Constraint | Observed Data | Contradiction | Conclusion |
|------|-------------------|---------------|---------------|------------|
| Field count vs cost | Should be O(n) linear | Showed O(n²) | Complexity anomaly | Path design problem |
| Phase 1: 2x speedup | Rust >> Python, peers 10x+ | Only 2x | Conflicts with fundamentals | Architecture issue |
| Phase 2 vs Phase 1 | More work = higher ceiling | Phase 2 < Phase 1 | Progression violated | Structural overhead |
| parse vs build | Symmetric ops, symmetric cost | parse >> build | Symmetry broken | Extra work in parse |
| Gap vs field count | Fixed cost independent of fields | Same gap for 2 and 6 fields | Constant gap | Fixed cost bottleneck |

Each case used a different logical constraint. The next problem will have new
constraints. The method is: identify what must be true, then check the data.
