# HTTP Compression Measurements

[日本語](README.ja.md)

Measured on 2026-11-06. depup did not enable reqwest's compression features, so every registry response arrived uncompressed. npm version documents (packuments) grow with the number of versions: `typescript` is 15.9 MB and `@types/node` is 11.2 MB. depup now enables reqwest's `gzip` feature, which sends `Accept-Encoding: gzip` and decompresses gzip responses automatically.

The update decisions did not change. For both inputs, the `--dry-run --no-osv --json --verbose` output before and after the change has the same updates, skips, reasons, and publication dates (to the second).

## Response Sizes

Transfer sizes reported by `curl` for the same URL without compression and with `--compressed` (2026-11-06):

| URL | Uncompressed | gzip | Ratio |
|---|---:|---:|---:|
| `registry.npmjs.org/typescript` | 15,894,727 B | 2,030,480 B | 13% |
| `registry.npmjs.org/@types%2Fnode` | 11,183,996 B | 1,425,600 B | 13% |
| `registry.npmjs.org/react` | 7,112,377 B | 1,386,623 B | 19% |
| `registry.npmjs.org/lodash` | 247,678 B | 47,684 B | 19% |
| `pypi.org/pypi/django/json` | 630,589 B | 140,167 B | 22% |
| `repo.packagist.org/p2/symfony/console.json` | 382,087 B | 48,170 B | 13% |
| `rubygems.org/api/v1/versions/rails.json` | 476,446 B | 58,817 B | 12% |
| `index.crates.io/se/rd/serde` | 164,874 B | 164,874 B | 100% |

npm and PyPI returned uncompressed responses for `Accept-Encoding: br` and `zstd`, so only gzip is enabled. index.crates.io does not serve compressed responses, so crates.io fetches are unchanged.

## Conditions

- Windows 11 Home x86_64, AMD Ryzen 7 5700X (8 cores, 16 logical processors), on the same home connection for both implementations.
- rustc 1.99.1 (`58cf8f9c3`, LLVM 23.1.1), MSVC target. Both implementations were built with `mise exec -- cargo build --release --locked` (LTO, codegen-units=1, strip).
- Before: the same source as commit `3ec2ffb1b65c004cbe11fd8e9c7e50ad2e43583f` (executable SHA-256 `40E91526DC2408FA0702ECC97819A236BD609D38C6D6DB324B2731DAA9249DCA`, 12,739,072 bytes). After: the working tree with reqwest's `gzip` feature and the new dependencies locked to versions that satisfy the age policy (`Cargo.lock` blob `77f270bceaf03ba69e57147f841cae705be113ae`, SHA-256 `99DBB5A43A8A456DC887826B8561EF7E95D825D306816DF6E8C8CD3FA21C7252`, 12,797,952 bytes).
- [measure.sh](measure.sh) ran under Git Bash 5.3.15 and recorded the real time reported by bash's `time`. The order was before-a → after-a → after-b → before-b, with five runs per input in each block, ten runs per implementation in total. Nothing was built or tested during the measurement.
- Both inputs run with `--dry-run --no-osv` to exclude the per-candidate OSV queries.
  - `npm`: [npm-packages.json](npm-packages.json) (30 widely used packages, each with an update available).
  - `pypi`: [pypi-packages.toml](pypi-packages.toml) (30 widely used packages, each with an update available).
- On Windows, depup reads the global configuration from the user profile, not from `HOME`. The configuration during the measurement was the generated default (`age = "1w"`, `osv = true`).
- The times include process startup, TLS connection setup, registry responses, and JSON parsing. The faster the connection, the smaller the benefit of the reduced transfer size. Read the numbers as a before/after comparison measured alternately in the same period.

## Results

Times are the median of ten runs per implementation, in seconds. Minimums and maximums are in [comparison.csv](comparison.csv). The reduction is `(1 - after / before) * 100`.

| Input | Before (s) | After (s) | Reduction |
|---|---:|---:|---:|
| npm (30 packages) | 4.870 (4.501–6.274) | 2.728 (2.541–5.346) | 43.97% |
| pypi (30 packages) | 2.058 (1.710–5.131) | 1.083 (0.744–6.721) | 47.36% |

For both inputs, nine of the ten after runs were faster than the fastest before run. The remaining after run (npm 5.346 s, pypi 6.721 s) and the 5.131 s before run of pypi are far from the other runs; their cause was not isolated. All 40 runs exited with code 0. The executable grew by 58,880 bytes (0.46%).

Raw data: [before-a](before-a.csv), [after-a](after-a.csv), [after-b](after-b.csv), [before-b](before-b.csv).

## Not Covered

- Memory use was not measured. The decompressed responses are the same size, so JSON parsing should use the same memory, but this was not verified.
- For Packagist and RubyGems, only response sizes were checked; the whole CLI run was not measured.
