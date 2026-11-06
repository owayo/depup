# crates.io Fetch Measurements

[日本語](README.ja.md)

Measured on 2026-11-06. depup used to fetch crates.io version lists only from the API (`https://crates.io/api/v1/crates/{name}`) and paced those requests to one per second for the whole run. Waiting about one second per dependency made Rust projects take time roughly proportional to their dependency count. depup now reads the sparse index first (`https://index.crates.io/`, the same source Cargo uses) and fetches up to four crates of a manifest concurrently. It still uses the API when the index cannot settle the version list and when publisher-based age exemptions (`[age_exempt]`) are configured.

The update decisions did not change. For both inputs, the `--dry-run --json --verbose` output before and after the change has the same updates, skips, reasons, and publication dates (to the second).

## Conditions

- Windows 11 Home x86_64, AMD Ryzen 7 5700X (8 cores, 16 logical processors), on the same home connection for both implementations.
- rustc 1.99.1 (`58cf8f9c3`, LLVM 23.1.1), MSVC target. Both implementations were built with `mise exec -- cargo build --release --locked` (LTO, codegen-units=1, strip) from the same dependencies and toolchain (`Cargo.lock` blob `6c313e5350215b41abd3b788a59493a3f0cf7d05`).
- Before: commit `3ee5b038c131cad0ee14b5ded4794f154b84833e` (executable SHA-256 `674518C439522CED50304C2539B4DBB6A7BD5080B13EBD8A6D71BB7874297EE7`). After: the same commit plus the sparse index change in the working tree (SHA-256 `40E91526DC2408FA0702ECC97819A236BD609D38C6D6DB324B2731DAA9249DCA`).
- [measure.sh](measure.sh) ran under Git Bash 5.3.15 and recorded the real time reported by bash's `time`. The order was before-a → after-a → after-b → before-b, with three runs per input in each block, six runs per implementation in total. Nothing was built or tested during the measurement.
- Inputs:
  - `depup-own`: depup's own `Cargo.toml` (22 direct dependencies, all up to date), run with `--dry-run` and default settings (age 1w, OSV enabled). Up-to-date dependencies do not query OSV.
  - `sixty`: [sixty-crates.Cargo.toml](sixty-crates.Cargo.toml) (60 widely used crates, each with an update available), run with `--dry-run --no-osv` to exclude the per-candidate OSV queries.
- On Windows, depup reads the global configuration from the user profile, not from `HOME`. The configuration during the measurement was the generated default (`age = "1w"`, `osv = true`).
- The times include process startup, TLS connection setup, and registry and CDN responses. They vary with the time of day and the connection, so read them as a before/after comparison measured alternately in the same period.

## Results

Times are the median of six runs per implementation, in seconds. Minimums and maximums are in [comparison.csv](comparison.csv). The reduction is `(1 - after / before) * 100`.

| Input | Before (s) | After (s) | Reduction |
|---|---:|---:|---:|
| depup-own (22 direct dependencies) | 23.203 (22.377–23.692) | 0.702 (0.595–0.867) | 96.97% |
| sixty (60 dependencies, `--no-osv`) | 62.620 (61.676–65.053) | 1.422 (0.906–3.336) | 97.73% |

For both inputs, the before and after distributions do not overlap. Before the change, each dependency cost about one second, and waiting between API requests was almost the whole run time. After the change, the index fetches run concurrently and the dependency count matters much less. The slowest after run of sixty (3.336 s) is longer than the other five (0.906–1.938 s); its cause was not isolated. All 24 runs exited with code 0.

Raw data: [before-a](before-a.csv), [after-a](after-a.csv), [after-b](after-b.csv), [before-b](before-b.csv).

## Not Covered

- Runs with publisher-based age exemptions (`[age_exempt]`) still use the API and are unchanged.
- The post-install age audit of Cargo.lock already used the sparse index with up to eight concurrent requests. This measurement stops at the `--dry-run` check and does not include an install or the audit.
- index.crates.io does not serve compressed (for example, gzip) responses, so HTTP compression does not change the transfer size of this path.
