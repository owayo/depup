# JSON manifest update measurements

[日本語](README.ja.md)

Measured on 2026-10-29. The change streams direct JSON properties instead of collecting them in a temporary vector, and borrows decoded strings when no escape expansion is needed. Escaped strings still use serde_json's validation and decoding. The shared walker serves Node, Bun catalogs, and Composer without changing the public parser API or rewriting rules.

## Conditions

- Windows x86_64, AMD Ryzen 7 5700X (8 cores / 16 logical processors).
- rustc 1.99.1 (`58cf8f9c3`, LLVM 23.1.1), MSVC target. `Cargo.lock` and `mise.toml` match dependency commit `8b07b212e5e3b61d2af2b7af577ef897524e6674` in both builds. This commit is also the baseline implementation.
- `cargo build --release --locked --example manifest_update_bench`, through `mise exec --`; repository release settings: LTO, one codegen unit, stripped symbols.
- Command: `manifest_update_bench 1000 7`. Each process warms up with 100 updates per case. Two processes per implementation give 14 samples, each containing 1,000 updates. Process order: before-a, after-a, after-b, before-b. No builds or tests ran during measurement.
- Input generation is outside the timer. An update changes the last dependency's `^1.0.0` to `^2.3.4`; the same key in `overrides` remains unchanged. Each case checks the resulting JSON. Escaped-key cases also check the spelling and CRLF count.
- Allocation requests are counted for a separate warmed update. Timing disables the counters. Bytes include reallocations, rather than peak or resident memory. The same counting allocator is present in both binaries.
- Network, registry parsing, file writes, and whole-command latency are outside this benchmark.

## Results

Times are medians across 14 samples, in microseconds per update. Full min/max ranges and unrounded medians are in [comparison.csv](comparison.csv).

| Case | Before µs | After µs | Time reduction | Allocation requests | Requested bytes |
|---|---:|---:|---:|---:|---:|
| Node, 1 dependency | 2.952 | 2.542 | 13.90% | 36 → 21 | 2,236 → 1,650 |
| Node, 100 dependencies | 25.610 | 21.331 | 16.71% | 140 → 21 | 19,870 → 11,748 |
| Node, 1,000 dependencies | 228.706 | 194.051 | 15.15% | 1,043 → 21 | 169,078 → 103,548 |
| Node, 100 escaped keys, CRLF | 36.688 | 35.862 | 2.25% | 340 → 321 | 24,109 → 17,587 |
| Bun, 100 nested catalog dependencies | 41.078 | 37.590 | 8.49% | 149 → 24 | 21,844 → 13,419 |
| Composer, 1,000 dependencies | 120.884 | 94.538 | 21.79% | 1,026 → 13 | 131,166 → 66,992 |

Node's 1,000-dependency case requests 38.76% fewer bytes; Composer requests 48.93% fewer. The escaped-key timing ranges overlap, so its small median reduction is not evidence of a clear speedup. Its allocation reduction is deterministic. No measured case has a slower median or more allocation requests/bytes; this does not establish behavior on other machines or total CLI speed.

Raw samples: [before-a](before-a.csv), [after-a](after-a.csv), [after-b](after-b.csv), [before-b](before-b.csv). [preliminary-before.csv](preliminary-before.csv) contains the initial exploratory baseline and is excluded from the comparison. To reproduce a median, combine each implementation's a/b files by `case`, sort the 14 `ns_per_update` values, and average the middle two. Time reduction is `(1 - after / before) * 100`.

Measured executable SHA-256 values:

```text
before 901aee7390a6b5700010ac3e81d004b30e0f7fbef2fb547e55827f119e60c3a4
after  f322c96fd27618a58281cee09e43346335a9bd84f424b6cdce4125e751a76552
```

## Validation and maintenance decisions

- Both new boundary tests pass on the baseline and changed implementation. All 12 JSON helper tests pass after the change. They cover transformation order, untouched nested values, CRLF, escaping, surrogate pairs, and rejection of invalid JSON strings.
- `make ci` passed after the change: formatting, clippy with warnings denied for all targets, and 2,827 tests. One test is ignored by default; Unix-gated cases are unavailable in this Windows run.
- AST structure and calls were inspected across the codebase, including install transaction setup, OSV judgment, shared registry fetching, path validation, and checked file replacement. `astro-sight context`, `impact`, and `review --git` reported no unresolved callers, public API changes, or newly dead symbols.
- The temporary collection and string ownership in the JSON walker were chosen because the baseline showed over 1,000 allocations per large update. The longer install transaction and OSV functions coordinate recovery/security decisions; splitting them without a specific improvement would broaden this change's risk. Public `enforce_lock_age_rust` remains available even though there is no internal caller.
- No reproducible product bug was found. Open GitHub issues were empty at the initial collection and subsequent check. No issue or PR was needed for this maintenance change.
- The documentation-link script `/Users/owa/GitHub/mac-shell/shell/link_agent.md.sh` is absent on Windows, and no equivalent was found in the local Windows scripts. It was not run. `CLAUDE.md` is a regular file in this checkout; it was preserved rather than replaced or edited.
