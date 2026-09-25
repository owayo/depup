# Development

Notes for building and maintaining depup. The setup and the standard `make` targets are in the [Development](../README.md#development) section of the README.

## Checks and CI

The CI quality job runs `make setup` and `make ci` on Linux and macOS, so running `make ci` locally performs the same checks: formatting, clippy, and the full test suite. On Windows, the CI build job runs `cargo test --locked` directly. The build job also builds the five release targets, because a dry run of the release workflow does not build anything.

To use the tools on your `PATH` instead of mise, pass `SYSTEM_TOOLS=1`; their versions may then differ from CI.

## Test Targets

Besides `make test`, two targets run a single test suite:

| Command | Description |
|---|---|
| `make test-e2e` | Run E2E tests only |
| `make test-integration` | Run integration tests only |

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
