<p align="center">
  <img src="docs/images/app.png" width="128" alt="depup">
</p>

<h1 align="center">depup</h1>

<p align="center">
  Multi-language dependency updater CLI for Node.js, Python, Rust, Go, Ruby, PHP, Java, Swift and mise, with a release age filter and OSV vulnerability checks
</p>

<!-- standard:badges:start -->
<h3 align="center">Supported Platforms</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
</p>

<p align="center">
  <a href="https://github.com/owayo/depup/actions/workflows/ci.yml"><img src="https://github.com/owayo/depup/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/depup/releases/latest"><img src="https://img.shields.io/github/v/release/owayo/depup" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/owayo/depup" alt="License"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>
<!-- standard:badges:end -->

---

depup finds the manifests in a project (`package.json`, `pyproject.toml`, `Cargo.toml`, `go.mod`, `Gemfile`, `composer.json`, Gradle build files, `Package.swift`, and mise config files), looks up newer versions in each registry, and rewrites the version specifiers in place, for every language in one run.

It is conservative by default: a new version becomes a candidate only after it has been public for a week, versions with known vulnerabilities on OSV.dev are avoided, pinned versions stay as they are, and ranges keep their operators and upper bounds.

Add `--install` to let each project's package manager refresh its lock file after the update.

## Features

- **Multi-Language Support**: Node.js, Python, Rust, Go, Ruby, PHP, Java, and Swift dependencies, plus mise tool versions in `mise.toml` / `.tool-versions`, through the same workflow
- **Manifest Updates**: Directly updates version specifications in manifest files (`package.json`, `Cargo.toml`, and so on)
- **Smart Version Handling**: Preserves version range formats (`^`, `~`, `>=`) while keeping upper bounds intact
- **Pinned Version Detection**: Skips intentionally pinned versions by default
- **Age Filter**: Only updates to versions that have been public for at least N days or weeks (1 week by default)
- **Project Age Policies**: Applies the minimum release age set in pnpm, Bun, or mise settings automatically
- **Vulnerability Check**: Looks up the chosen version on OSV.dev and avoids versions with known vulnerabilities (enabled by default)
- **Package Manager Install**: `--install` runs each project's package manager after the update and passes the age filter on to pnpm, uv, and mise
- **Bun Catalogs**: Updates Bun `catalog` / `catalogs` definitions in `package.json`
- **Monorepo Support**: `.depup`, Cargo/pnpm/Go workspaces, Gradle multi-project builds, nested package installs, and Tauri projects
- **Multiple Output Formats**: Colored text with the release date of each new version, JSON, and diff

### Supported Languages

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-339933?logo=nodedotjs&amp;logoColor=white" alt="Node.js">
  <img src="https://img.shields.io/badge/Python-3776AB?logo=python&amp;logoColor=white" alt="Python">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&amp;logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/Go-00ADD8?logo=go&amp;logoColor=white" alt="Go">
  <img src="https://img.shields.io/badge/Ruby-CC342D?logo=ruby&amp;logoColor=white" alt="Ruby">
  <img src="https://img.shields.io/badge/PHP-777BB4?logo=php&amp;logoColor=white" alt="PHP">
  <img src="https://img.shields.io/badge/Java-ED8B00?logo=openjdk&amp;logoColor=white" alt="Java">
  <img src="https://img.shields.io/badge/Swift-F05138?logo=swift&amp;logoColor=white" alt="Swift">
  <img src="docs/images/badge-mise.svg" alt="mise">
</p>

| Language | Manifest | Registry | Lock Files |
|----------|----------|----------|------------|
| <img src="https://img.shields.io/badge/-339933?logo=nodedotjs&logoColor=white" height="16"> Node.js | package.json (including Bun catalogs) | npm | package-lock.json, pnpm-lock.yaml, yarn.lock, bun.lock, bun.lockb |
| <img src="https://img.shields.io/badge/-3776AB?logo=python&logoColor=white" height="16"> Python | pyproject.toml | PyPI | uv.lock, requirements.lock, poetry.lock |
| <img src="https://img.shields.io/badge/-000000?logo=rust&logoColor=white" height="16"> Rust | Cargo.toml | crates.io | Cargo.lock |
| <img src="https://img.shields.io/badge/-00ADD8?logo=go&logoColor=white" height="16"> Go | go.mod (go.work members auto-detected) | Go Proxy | go.sum |
| <img src="https://img.shields.io/badge/-CC342D?logo=ruby&logoColor=white" height="16"> Ruby | Gemfile | RubyGems | Gemfile.lock |
| <img src="https://img.shields.io/badge/-777BB4?logo=php&logoColor=white" height="16"> PHP | composer.json | Packagist | composer.lock |
| <img src="https://img.shields.io/badge/-ED8B00?logo=openjdk&logoColor=white" height="16"> Java | build.gradle, build.gradle.kts, gradle/*.versions.toml (settings.gradle subprojects auto-detected) | Maven Central | gradle.lockfile |
| <img src="https://img.shields.io/badge/-F05138?logo=swift&logoColor=white" height="16"> Swift | Package.swift | GitHub Tags | Package.resolved |
| <img src="docs/images/badge-mise-icon.svg" height="16"> mise | mise.toml, .mise.toml, .config/mise/config.toml, .tool-versions, etc. | `mise ls-remote` | mise.lock |

## Requirements

- **mise**: At runtime, only needed to update mise tool versions (version lists come from `mise ls-remote`). Without it, depup ignores mise config files, warns once, and continues with the other languages
- **Package managers**: `--install` runs the package manager of each project (npm, pnpm, uv, Cargo, Bundler, Composer, Gradle, and so on), so it must be installed; a package manager that is not installed counts as a failed install
- **Network access**: Versions are looked up in the public registries, and the vulnerability check queries the OSV.dev API

## Installation

<!-- standard:install:start -->
### Homebrew (macOS/Linux)

```bash
brew install owayo/depup/depup
```

### winget (Windows)

```powershell
winget install owayo.depup
```

### Cargo

Requires Rust 1.98 or later.

```bash
cargo install --git https://github.com/owayo/depup --locked
```

### From GitHub Releases

Download the archive for your platform from [Releases](https://github.com/owayo/depup/releases/latest), extract it, and put `depup` on your `PATH`. Each release also includes `SHA256SUMS` for checking the downloads.

| Platform | Archive |
|---|---|
| Linux (x86_64) | `depup-x86_64-unknown-linux-gnu.tar.gz` |
| Linux (ARM64) | `depup-aarch64-unknown-linux-gnu.tar.gz` |
| macOS (Intel) | `depup-x86_64-apple-darwin.tar.gz` |
| macOS (Apple Silicon) | `depup-aarch64-apple-darwin.tar.gz` |
| Windows (x86_64) | `depup-x86_64-pc-windows-msvc.zip` |

On macOS, if you downloaded the archive with a browser, remove the quarantine attribute before running it: `xattr -d com.apple.quarantine depup`.

### From Source

Requires [mise](https://mise.jdx.dev/) (the Rust toolchain is pinned in `mise.toml`).

```bash
git clone https://github.com/owayo/depup.git
cd depup
make install
```

`make install` installs to `/usr/local/bin`. Set `INSTALL_PATH` to change it (for example `make install INSTALL_PATH="$HOME/.local/bin"`).
<!-- standard:install:end -->

After a winget install, open a new terminal so that the updated `PATH` takes effect. To remove a build installed from source, run `make uninstall` with the same `INSTALL_PATH`.

## Usage

```bash
depup [OPTIONS] [PATH]
```

`PATH` is the directory to process (defaults to the current directory). depup updates every supported language it finds there unless you limit it with a language flag such as `--node` or `--python`.

```bash
# Preview all updates (dry run)
depup -n

# Update Node.js dependencies only
depup --node

# Update only lodash and typescript
depup --only lodash --only typescript

# Exclude react from updates
depup --exclude react

# Only update to versions at least 2 weeks old (the default is 1 week)
depup --age 2w

# Update and show diff
depup --diff

# Update, then run the package manager install (npm install, etc.)
depup --node --install

# JSON output for CI/CD
depup --json
```

Output for a Python project and for a Tauri project:

<table>
  <tr>
    <td align="center">
      <strong>Python (pyproject.toml)</strong><br>
      <img src="docs/images/output_python.png" width="400" alt="depup Python output">
    </td>
    <td align="center">
      <strong>Tauri (package.json + Cargo.toml)</strong><br>
      <img src="docs/images/output_tauri.png" width="400" alt="depup Tauri output">
    </td>
  </tr>
</table>

### How depup Decides Updates

Unless a dependency is pinned, depup looks up its published versions, picks the newest one that passes the checks below, and rewrites the manifest in place. By default:

- An exact version such as `"1.2.3"` in `package.json` is treated as an intentional pin and is not updated unless you pass `--include-pinned`. Go modules and mise tools are exceptions ([Pinned Versions and `--include-pinned`](docs/usage.md#pinned-versions-and---include-pinned)).
- Only versions that have been public for at least one week are candidates ([Age Filter](docs/usage.md#age-filter)).
- Versions with known vulnerabilities are avoided ([Vulnerability Check](docs/usage.md#vulnerability-check-osvdev)).
- Prereleases are not proposed while the current version is stable ([Candidate Ordering and Prereleases](docs/usage.md#candidate-ordering-and-prereleases)).
- Ranges keep their shape and upper bound; only the lower bound moves ([Upper and Lower Bounds of Ranges](docs/usage.md#upper-and-lower-bounds-of-ranges)).
- Major bumps are allowed unless `--max-change` caps them ([Limiting Bumps](docs/usage.md#limiting-bumps---max-change)).

### More Documentation

- [Filtering Candidates](docs/usage.md#filtering-candidates): the age filter, project age policies, the OSV.dev check, and `--max-change`
- [Version Specifiers and Rewriting](docs/usage.md#version-specifiers-and-rewriting): which specifiers count as pinned, how ranges and formats are preserved, and which constraints are left unchanged
- [Running the Package Manager](docs/usage.md#running-the-package-manager---install): the command `--install` runs for each package manager, and how far the age filter reaches into transitive dependencies
- [When a Dependency Is Not Updated](docs/usage.md#when-a-dependency-is-not-updated): the skip reasons that `--verbose` lists
- [Command-Line Reference](docs/cli-reference.md): all options, the text / JSON / diff output, and the exit codes
- [Ecosystems and Monorepos](docs/ecosystems.md): workspaces and Tauri projects, and the exact declarations depup reads in each ecosystem

## Configuration

depup reads settings from three places. For each setting, a command-line flag overrides the global configuration file for that run. The release age is the exception: a minimum release age set in the project takes precedence over `--age` as well.

| Setting | Location | Purpose |
|---|---|---|
| Global defaults | `~/.config/depup/config.toml`, created on the first run | Default `age`, `osv`, and `max_change` for every project |
| Monorepo directories | `.depup` at the project root | Additional directories to process |
| Project age policy | `minimumReleaseAge` in the pnpm or Bun settings, `minimum_release_age` in the mise settings | Minimum release age of that project |

For example, to wait two weeks instead of one and to rule out major bumps in every project:

```toml
# ~/.config/depup/config.toml
age = "2w"
max_change = "minor"
```

All keys, the `.depup` format, and how invalid values are handled: [docs/configuration.md](docs/configuration.md)

## Development

<!-- standard:dev:start -->
Requires [mise](https://mise.jdx.dev/). Tool versions are pinned in `mise.toml`.

```bash
make setup   # Install the toolchain (mise) and dependencies
make ci      # Run the same checks as CI (no changes)
```

| Command | Description |
|---|---|
| `make setup` | Install the toolchain (mise) and dependencies |
| `make build` | Build a debug binary |
| `make release` | Build a release binary |
| `make run` | Run the debug binary (arguments via ARGS="...") |
| `make test` | Run the tests |
| `make lint` | Run clippy with warnings as errors |
| `make fmt` | Format the code (rewrites files) |
| `make fmt-check` | Check the formatting (no changes) |
| `make check` | Run fmt-check and lint (no changes) |
| `make ci` | Run the same checks as CI (no changes) |
| `make install` | Install the release binary to INSTALL_PATH (default /usr/local/bin) |
| `make uninstall` | Remove the binary from INSTALL_PATH |
| `make clean` | Remove build artifacts |

Run `make` to list every target. Releases are published from GitHub Actions (**Actions → Release → Run workflow**).
<!-- standard:dev:end -->

The additional test targets, what CI runs on each OS, and the release steps are described in [docs/development.md](docs/development.md).

## License

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->
