# Dependency check concurrency

[日本語](README.ja.md)

Measured on 2026-11-13. The per-manifest cap on concurrent dependency checks (version listing, judgment, and OSV queries), `MAX_VERSION_CHECK_CONCURRENCY`, was raised from 4 to 16. Version listing is still limited separately by the per-registry semaphores (8 for crates.io, 10 otherwise), so the cap mainly decides how many OSV queries for update candidates overlap. With a cap of 4, checking a 60-dependency manifest waited for 15 rounds of OSV requests.

## Conditions

- Windows x86_64, AMD Ryzen 7 5700X (8 cores, 16 logical processors), rustc 1.99.1 (`58cf8f9c3`), MSVC target, `cargo build --release --locked` (LTO, codegen-units=1, strip).
- "Before" is the maintenance working copy (`fb72ab3`); "after" changes only the cap to 16. Executable SHA-256: before `cb4b2f872c670959258a7955cee47d6792fd756e708a6caf34884e21f79c9327`, after `7eac16a0d336956a91d6593b9848779887bcfa5e815661d63ae7691caca61bdc`.
- `bash benchmarks/version-check-concurrency/measure.sh <name> <depup> 5` ran in the order before-a → after-a → after-b → before-b (10 runs per implementation and scenario), with `--dry-run`, the default age (one week), and OSV checks enabled.
- Scenarios: `sixty` is 60 crates.io dependencies (each update candidate is checked with OSV), `workspace` splits the same 60 into a four-member Cargo workspace of 15 each, `npm` is 30 npm packages, and `depup-own` is depup's own Cargo.toml (all latest, no OSV queries; a control).
- Results depend on the network and on the registries and OSV. No builds or tests ran during the measurement (only waiting for GitHub CI). On Windows depup reads the global configuration from the user profile rather than `HOME`, so both implementations ran with the same user configuration.

## Results

Times are medians of 10 runs in seconds (minimum–maximum in parentheses). Raw data: [before-a](before-a.csv), [after-a](after-a.csv), [after-b](after-b.csv), [before-b](before-b.csv); summary: [comparison.csv](comparison.csv).

| Scenario | Before | After | Reduction | Ranges |
|---|---:|---:|---:|---|
| sixty | 6.737 (6.230–11.562) | 2.575 (2.227–5.209) | 61.78% | do not overlap |
| workspace | 6.996 (6.619–7.832) | 3.143 (2.551–4.551) | 55.07% | do not overlap |
| npm | 5.719 (5.339–6.349) | 3.558 (3.283–7.034) | 37.78% | overlap (one after run took 7.034 s) |
| depup-own | 0.799 (0.694–3.680) | 0.960 (0.562–1.795) | -20.21% | overlap |

Because the `depup-own` control looked slower, it was re-measured with the same executables in two rounds of before → after → after → before, 20 runs per implementation ([control.csv](control.csv)). The medians were 0.734 s (0.660–1.153) before and 0.677 s (0.595–1.018) after, with overlapping ranges and no regression; the -20.21% above is attributed to the small sample.

During exploration, an uncommitted experimental build that read the cap from an environment variable compared 4, 8, 16, and 32 with two runs each. The `sixty` medians were about 6.4 s at 4, 3.4 s at 8, 2.2 s at 16, and 1.8 s at 32. The gain from 16 to 32 is small while concurrent OSV queries keep growing, so 16 was chosen. `workspace` has 15 dependencies per manifest, so 16 and 32 are the same setting, yet the two runs differed by about 2×, which shows how noisy this measurement is.

## Notes

- Concurrent version listing is unchanged (per-registry semaphores), and the crates.io adapter still keeps API use to one request per second. What grows is the number of concurrent OSV (api.osv.dev) queries, at most 16 per manifest; manifests are still processed one at a time.
- Output order still follows the input order of `futures::stream::buffered`, so the output does not change.
- Overall CLI time varies with the number of dependencies, update candidates, and network conditions. Small manifests and manifests without update candidates see no difference.
