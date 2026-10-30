# Cargo manifest update measurements

[日本語](README.ja.md)

Measured on 2026-10-31. Cargo registry updates previously compiled six package-specific regular expressions on every call, including multiline-table updates that did not need those patterns. Git tag updates compiled two more patterns per call. The change matches the package name literally and applies shared `LazyLock<Regex>` patterns to the remaining syntax. It also shares version parsing/formatting across the three declaration forms, borrows section names, reserves the output buffer, and avoids inspecting unrelated declarations for source options.

The public parser API, version formatting, source exclusions, dependency/patch section boundaries, quotes, comments, and line endings remain unchanged.

## Conditions

- Windows x86_64, AMD Ryzen 7 5700X, 8 cores / 16 logical processors.
- rustc 1.99.1 (`58cf8f9c3`, LLVM 23.1.1), MSVC target. Before and after use the same dependencies and toolchain from baseline `5d0572d4e312797001783ce681473cc7d37f4542`. Git blob IDs: `Cargo.lock` = `85cb3733a24ca111851f87a1c371e42045e28263`, `mise.toml` = `fec5b6ecfdd3bbc60c81871e64024351a9cc11a3`.
- Build both with `mise exec -- cargo build --release --locked --example manifest_update_bench`. Release settings are LTO, one codegen unit, and stripped symbols. The before build uses the baseline library with the same expanded benchmark as the after build; benchmark blob = `f99abe2c292b3fb36f4442e322649267675f708c`.
- Run `manifest_update_bench 1000 7 cargo`. Each process warms each case with 100 updates, then takes 7 samples of 1,000 updates. Two processes per implementation give 14 samples. Order: before-a, after-a, after-b, before-b. No builds or tests ran during measurement; CPU affinity was unrestricted for these samples.
- Inputs contain 1, 100, or 1,000 dependencies. Each operation changes only the last dependency, from `^1.0.0` to `^2.3.4`, or from tag `v1.0.0` to `v2.3.4`. Input generation and validation are outside timing. Every case checks byte-for-byte output, including an untouched same-named metadata entry, single quotes, CRLF, and no final newline; both input and expected output are parsed as TOML.
- Allocation requests and requested bytes are counted for one separate warmed update, with counting disabled during timing. Bytes include reallocations and do not represent peak or resident memory.
- Saved CSV files use LF line endings; normalizing the Windows output did not change any CSV field.
- This measures warmed, in-memory rewriting. The first lazy initialization, network requests, registry parsing, disk writes, and total CLI latency are outside the measurement. In particular, the one-dependency result does not establish the speedup of a cold, one-dependency CLI invocation.

## Results

Times are medians of 14 samples, in microseconds per update. Unrounded medians and min/max ranges are in [comparison.csv](comparison.csv).

| Case | Before µs | After µs | Time reduction | Allocation requests | Requested bytes |
|---|---:|---:|---:|---:|---:|
| Simple, 1 dependency | 407.239 | 1.643 | 99.60% | 3,535 → 19 | 3,045,601 → 651 |
| Simple, 100 dependencies | 451.931 | 6.964 | 98.46% | 4,232 → 19 | 3,421,431 → 2,928 |
| Simple, 1,000 dependencies | 952.455 | 55.839 | 94.14% | 10,536 → 19 | 3,784,311 → 23,628 |
| Inline table, 100 dependencies | 506.400 | 8.388 | 98.34% | 4,372 → 20 | 3,504,945 → 6,509 |
| Dotted key, 100 dependencies | 488.484 | 8.261 | 98.31% | 4,386 → 21 | 3,509,414 → 3,883 |
| Multiline table, 100 dependencies | 396.404 | 17.045 | 95.70% | 3,443 → 20 | 3,367,673 → 7,369 |
| Git tag inline table, 100 dependencies | 183.617 | 5.118 | 97.21% | 1,716 → 7 | 1,196,337 → 9,221 |
| Git tag dotted key, 100 dependencies | 192.753 | 7.890 | 95.91% | 1,928 → 10 | 1,196,232 → 7,628 |
| Git tag multiline table, 100 dependencies | 157.098 | 12.160 | 92.26% | 1,429 → 7 | 1,124,474 → 9,987 |

All Cargo cases have disjoint before/after timing ranges and fewer allocation requests and bytes. The large simple case requests 99.38% fewer bytes. Its remaining two scans grow linearly with the input size; the formerly linear allocation count is now constant in these simple-declaration cases.

Raw samples: [before-a](before-a.csv), [after-a](after-a.csv), [after-b](after-b.csv), [before-b](before-b.csv). To reproduce a median, combine the a/b files by `case`, sort the 14 `ns_per_update` values, and average the middle two. Time reduction is `(1 - after / before) * 100`.

Measured executable SHA-256:

```text
before bdf6d890f5d512b138a945cdaecfd0af8957fd250ad66e5a9d430e24912b00df
after  89c78194ed62e8a06875d24891fa286ffbaef1f08e0ee4f5ddd65f3e9b0dd94e
```

## Controls and exploratory measurements

The unchanged JSON cases use the same measurement helper. Their unrestricted 14-sample comparison is in [json-comparison.csv](json-comparison.csv), from `json-{before,after}-{a,b}.csv`. Allocation counts and requested bytes are identical. Timing medians range from a 3.37% reduction to a 6.21% increase; the escaped-key case's ranges do not overlap, so this run alone cannot establish the absence of a timing regression.

A follow-up pins the measurement driver and its child processes to logical CPU 2 (Windows affinity mask `4`), leaving the machine-wide configuration unchanged. It uses the same binaries, 100 warmup updates, 5,000 updates per sample, and two processes per implementation: 14 samples, again before-a → after-a → after-b → before-b, without concurrent builds or tests. Raw files are `json-affinity-{before,after}-{a,b}.csv`; medians and ranges are in [json-affinity-comparison.csv](json-affinity-comparison.csv).

| JSON control | Before µs | After µs | Time reduction |
|---|---:|---:|---:|
| Node, 1 dependency | 2.532 | 2.546 | -0.54% |
| Node, 100 dependencies | 21.310 | 21.779 | -2.20% |
| Node, 1,000 dependencies | 193.816 | 197.349 | -1.82% |
| Node, 100 escaped keys | 34.949 | 35.354 | -1.16% |
| Bun, 100 dependencies | 37.513 | 37.876 | -0.97% |
| Composer, 1,000 dependencies | 94.647 | 90.262 | 4.63% |

Node's 100- and 1,000-dependency timing ranges remain disjoint; the other Node/Bun ranges overlap. All JSON allocation metrics remain identical. The cause of these small timing shifts is not established. The change is retained for the 92–99.6% Cargo time reductions and the allocation reductions, with this measured JSON tradeoff recorded; it does not claim zero regression across every parser or total CLI speed.

The `exploratory/` directory is excluded from the final comparison. Its a/b files and comparison describe the first version-only change: the large case still made 3,016 allocations because unrelated lines were checked for source options. `preliminary-source-filter.csv` shows 19 allocations after adding the literal-name gate. Git tag timings in the first experiment were 4–10% slower despite unchanged source, and still compiled two regexes per call; applying the same measured optimization removed that cost. `preliminary-before.csv` is the initial 100-iteration baseline, and its JSON files contain one initial process per implementation.

## Validation and maintenance decisions

- The two added boundary tests pass before and after the change. All 86 Cargo parser tests pass, covering package-name boundaries, source exclusions, multiple declarations, target/workspace/patch scopes, quotes, CRLF, and missing packages. The new cases check full output with irregular spacing, shared name prefixes, comments, and an unterminated final line.
- Final local `make ci`, through Git Bash, passes formatting, clippy with warnings denied for all targets, and 2,850 tests. `make release` also succeeds. One test is ignored by default; Unix-gated tests are not available in this local Windows run.
- The project-wide AST inventory and selected calls were inspected, including OSV judgment, shared registry locking, install transaction setup, path containment, Git command arguments/timeouts/redaction, and checked file replacement. `astro-sight context`, `impact`, and `review --git` show no unresolved callers, public API changes, or newly dead symbols. Historical co-change suggestions do not require changes to the other language parsers or usage documentation.
- Sharing Cargo's repeated formatting paths provides a concrete reduction in duplicated behavior: `update_version` complexity falls from 17 to 11, and `update_git_tag` from 10 to 8. Larger install/OSV lifecycle functions retain their recovery and security boundaries. The internally unused public `enforce_lock_age_rust` is preserved. A batch rewrite API would affect all manifest parsers and writer guards; the residual scan time does not justify that wider change in this maintenance pass.
- Synchronization started on a clean `main` at the baseline and required no updates or stash. `mise exec -- depup --install --include-pinned` reported no updates for 22 library dependencies and the Rust toolchain; `Cargo.toml`, `Cargo.lock`, and `mise.toml` are unchanged. Initial and implementation-stage GitHub queries returned no open issues, and no new reproducible product bug was found.
- README and development documentation are updated in both languages, with the benchmark command and Git Bash/PowerShell check instructions. Two inspected file-replacement comments are translated to Japanese without code changes.
- `/Users/owa/GitHub/mac-shell/shell/link_agent.md.sh` is absent on Windows and no equivalent was found in the local Windows script filenames, so it was not run. `CLAUDE.md` is recorded by Git as a symlink but checked out as a regular file; it was preserved without editing or replacement.
