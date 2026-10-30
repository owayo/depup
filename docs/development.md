# Development

Notes for building and maintaining depup. The setup and the standard `make` targets are in the [Development](../README.md#development) section of the README.

## Checks and CI

The CI quality job runs `make setup` and `make ci` on Linux and macOS, so running `make ci` locally performs the same checks: formatting, clippy, and the full test suite. On Windows, the CI build job runs `cargo test --locked` directly. The build job also builds the five release targets, because a dry run of the release workflow does not build anything.

To use the tools on your `PATH` instead of mise, pass `SYSTEM_TOOLS=1`; their versions may then differ from CI.

On Windows, run the `make` targets in Git Bash, since their recipes use a POSIX shell. From PowerShell, the equivalent checks with the pinned toolchain are:

```powershell
mise install
mise exec -- cargo fetch --locked
mise exec -- cargo fmt --all -- --check
mise exec -- cargo clippy --locked --all-targets -- -D warnings
mise exec -- cargo test --locked
```

## Test Targets

Besides `make test`, two targets run a single test suite:

| Command | Description |
|---|---|
| `make test-e2e` | Run E2E tests only |
| `make test-integration` | Run integration tests only |

## JSON Manifest Benchmark

Run the in-memory update benchmark with the pinned toolchain:

```bash
mise exec -- cargo run --release --locked --example manifest_update_bench -- 1000 7
```

The arguments are updates per sample and sample count. Each case warms up with 100 updates, then prints CSV with nanoseconds per update and allocation requests for one warmed update. Timing runs with allocation counting disabled; allocation bytes include reallocation requests and are not peak memory usage.

Cases cover 1, 100, and 1,000 Node dependencies, escaped keys with CRLF, nested Bun catalogs, and 1,000 Composer dependencies. Each update targets the last dependency. The benchmark checks the resulting JSON and preserves a same-named entry outside the dependency sections. It excludes network requests and disk writes, so its results measure manifest rewriting, rather than total CLI duration. Compare the same inputs, dependencies, toolchain, and release profile, without concurrent builds or tests. Recorded measurements are in [benchmarks/json-manifest](../benchmarks/json-manifest/README.md).

## Cargo Manifest Benchmark

Select the Cargo cases with the third argument:

```bash
mise exec -- cargo run --release --locked --example manifest_update_bench -- 1000 7 cargo
```

This uses the same warmup, timing, and allocation counters as the JSON cases. It covers simple versions with 1, 100, and 1,000 dependencies, and inline tables, dotted keys, and multiline tables with 100 dependencies. The three table forms also have git tag cases. Inputs use single quotes, CRLF, and a final line without a newline; each case checks the complete output and validates the TOML. Recorded measurements and scope limits are in [benchmarks/cargo-manifest](../benchmarks/cargo-manifest/README.md).

## Release

Releases are published from GitHub Actions: open **Actions > Release > Run workflow**.

Try it with **dry_run** checked first. A dry run only computes and prints the next version; it does not commit, tag, build, create a GitHub Release, update the Homebrew tap, or submit to winget.

Versions use the `yy.m.counter` format (for example `26.9.100`). The counter starts at 100 each month and goes up by one with each release in that month. The month is taken in Japan Standard Time.

A run without dry_run:

1. Bumps the version in `Cargo.toml` and syncs only depup's own entry in `Cargo.lock` (`cargo update --workspace`; dependencies are not re-resolved), then commits the change, tags it `v<version>`, and pushes both. If the tag already exists, the run stops here
2. Builds the five targets (Linux x86_64 / ARM64, macOS Apple Silicon / Intel, Windows x86_64) and creates the GitHub Release with the archives and `SHA256SUMS`
3. Updates the Homebrew tap ([owayo/homebrew-depup](https://github.com/owayo/homebrew-depup)) when the GitHub App for the tap is configured
4. Submits the new version to winget-pkgs when a winget token is configured and `owayo.depup` is already registered there; otherwise this step is skipped
5. Writes the result of each job to the summary of the run

If a job after the version bump fails, use **Re-run failed jobs** on the same run. **Re-run all jobs** starts again from the version bump and stops because the tag already exists.
