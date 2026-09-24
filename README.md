<p align="center">
  <img src="docs/images/app.png" width="128" alt="depup">
</p>

<h1 align="center">depup</h1>

<p align="center">
  Multi-language dependency updater CLI tool
</p>

<h3 align="center">Supported Platforms</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
  <br>
  <a href="https://github.com/owayo/depup/actions/workflows/release.yml"><img src="https://github.com/owayo/depup/actions/workflows/release.yml/badge.svg?branch=main" alt="Release"></a>
  <a href="https://github.com/owayo/depup/actions/workflows/ci.yml"><img src="https://github.com/owayo/depup/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/depup/releases"><img src="https://img.shields.io/github/v/release/owayo/depup" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

<h3 align="center">Supported Languages</h3>

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

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>

---

### Output Examples

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

## Features

- **Multi-Language Support**: Node.js, Python, Rust, Go, Ruby, PHP, Java, Swift
- **mise Support**: Updates tool versions in `mise.toml` / `.tool-versions` through the same workflow
- **Manifest Updates**: Directly updates version specifications in manifest files (`package.json`, `Cargo.toml`, and so on)
- **Smart Version Handling**: Preserves version range formats (`^`, `~`, `>=`) while keeping upper bounds intact
- **Pinned Version Detection**: Skips intentionally pinned versions by default
- **Age Filter**: Only updates to versions that have been public for at least N days or weeks (1 week by default)
- **Vulnerability Check**: Looks up the chosen version on OSV.dev and avoids versions with known vulnerabilities (enabled by default)
- **Project Age Policies**: Applies the minimum release age set in pnpm, Bun, or mise settings automatically
- **Bun Catalogs**: Updates Bun `catalog` / `catalogs` definitions in `package.json`
- **Monorepo Support**: `.depup`, Cargo/pnpm/Go workspaces, Gradle multi-project builds, nested package installs, and Tauri projects
- **Release Date Display**: Shows when each new version was released
- **Multiple Output Formats**: Text (colored), JSON, diff

## Supported Languages

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

- **OS**: macOS, Linux, Windows
- **Rust**: 1.85+ (for building from source)
- **mise**: only needed to update mise tool versions (version lists come from `mise ls-remote`). Without it, depup ignores mise config files, warns once, and continues with the other languages

## Installation

### Homebrew (macOS/Linux)

```bash
brew install owayo/depup/depup
```

### winget (Windows)

```powershell
winget install owayo.depup
```

### From Source

```bash
git clone https://github.com/owayo/depup.git
cd depup
cargo install --path .
```

### From GitHub Releases

Download the latest binary from [Releases](https://github.com/owayo/depup/releases).

#### macOS (Apple Silicon)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-aarch64-apple-darwin.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### macOS (Intel)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-x86_64-apple-darwin.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### Linux (x86_64)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### Linux (ARM64)

```bash
curl -L https://github.com/owayo/depup/releases/latest/download/depup-aarch64-unknown-linux-gnu.tar.gz | tar xz
sudo mv depup /usr/local/bin/
```

#### Windows

Download `depup-x86_64-pc-windows-msvc.zip` from [Releases](https://github.com/owayo/depup/releases), extract, and add to PATH.

> `winget install owayo.depup` does this for you (it registers `depup` on PATH), so the manual download is only needed if you do not use winget. After a winget install, open a new terminal so the updated PATH takes effect.

## Quickstart

```bash
# Update all dependencies (dry run)
depup -n

# Update Node.js dependencies only
depup --node

# Only update to versions at least 2 weeks old (the default is 1 week)
depup --age 2w

# Update and show diff
depup --diff
```

## Usage

### Basic Syntax

```bash
depup [OPTIONS] [PATH]
```

`PATH` is the directory to process (defaults to the current directory). With `--cd`, depup changes to that directory first and resolves `PATH` from there.

### Options

| Option | Short | Description |
|--------|-------|-------------|
| `--cd <DIR>` | `-C` | Change to directory before running |
| `--dry-run` | `-n` | Show what would be updated without making changes |
| `--verbose` | | Enable verbose output |
| `--quiet` | `-q` | Minimal output |
| `--node` | | Update only Node.js dependencies |
| `--python` | | Update only Python dependencies |
| `--rust` | | Update only Rust dependencies |
| `--go` | | Update only Go dependencies |
| `--ruby` | | Update only Ruby dependencies |
| `--php` | | Update only PHP dependencies |
| `--java` | | Update only Java dependencies |
| `--swift` | | Update only Swift dependencies |
| `--mise` | | Update only mise tool versions (mise.toml / .tool-versions) |
| `--exclude <PKG>` | | Exclude specific packages (repeatable) |
| `--only <PKG>` | | Update only specific packages (repeatable) |
| `--include-pinned` | | Include pinned versions in update |
| `--age <DURATION>` | | Minimum release age (e.g., 2w, 10d, 1m). Overrides global config |
| `--no-age` | | Disable age filter for this run (overrides global config and default, but not a project `minimumReleaseAge`) |
| `--osv` | | Check candidates against the OSV.dev vulnerability database and avoid versions with known vulnerabilities (enabled by default) |
| `--no-osv` | | Disable OSV vulnerability check for this run (overrides global config and default) |
| `--max-change <LEVEL>` | | Limit allowed bumps: `patch` (patch only), `minor` (patch + minor), `major` (default — all) |
| `--json` | | Output results in JSON format |
| `--diff` | | Show changes in diff format |
| `--install` | | Run package manager install after update |
| `--version` | `-V` | Show version |
| `--help` | `-h` | Show help |

When `--only` is present, it takes precedence over `--exclude`. This lets an explicit allow-list entry remain updatable even if the same package also appears in a broader exclude list.

### Examples

```bash
# Preview all updates
depup -n

# Update only lodash and typescript
depup --only lodash --only typescript

# Exclude react from updates
depup --exclude react

# --only takes precedence if the same package is also excluded
depup --only lodash --exclude lodash

# Only update to versions at least 2 weeks old
depup --age 2w

# Update Python and Rust only
depup --python --rust

# Update Java (Gradle) dependencies
depup --java

# Update Swift (Package.swift) dependencies
depup --swift

# Update mise tool versions (mise.toml / .tool-versions)
depup --mise

# JSON output for CI/CD
depup --json

# Update, then run the package manager install (npm install, etc.)
depup --node --install

# Run in a different directory
depup --cd ./projects/myapp -n
```

## How depup Decides Updates

Unless a dependency is pinned, depup looks up its published versions, picks the newest one that passes the checks below, and rewrites the manifest in place. By default:

- An exact version such as `"1.2.3"` in `package.json` is treated as an intentional pin and is not updated unless you pass `--include-pinned`. Go modules and mise tools are exceptions ([Pinned Versions and `--include-pinned`](#pinned-versions-and---include-pinned)).
- Only versions that have been public for at least one week are candidates ([Age Filter](#age-filter)).
- Versions with known vulnerabilities are avoided ([Vulnerability Check](#vulnerability-check-osvdev)).
- Prereleases are not proposed while the current version is stable ([Candidate Ordering and Prereleases](#candidate-ordering-and-prereleases)).
- Ranges keep their shape and upper bound; only the lower bound moves ([Upper and Lower Bounds of Ranges](#upper-and-lower-bounds-of-ranges)).
- Major bumps are allowed unless `--max-change` caps them ([Limiting Bumps](#limiting-bumps---max-change)).

This README uses two terms for a dependency that depup leaves unchanged:

- **Skipped**: depup recognizes the dependency but does not update it. Skipped dependencies are counted in the output, and `--verbose` lists each one with its reason, such as `pinned` or `latest` ([When a Dependency Is Not Updated](#when-a-dependency-is-not-updated)).
- **Not processed**: depup does not take the declaration as an updatable dependency, so it does not appear in the output at all. This happens when the declaration points to a non-registry source (such as a Cargo `path` dependency), names a platform package (such as `php` in Composer), or writes its version in a floating or unsupported form (such as `"*"` or `latest`).

Filters such as the age filter and the vulnerability check remove candidate versions, not the dependency itself. depup updates the dependency to the newest remaining candidate and reports it as skipped only when no candidate newer than the current version remains.

## Filtering Candidates

Three configurable filters decide which published versions can be chosen: the age filter and the vulnerability check (both on by default) and `--max-change` (off unless you set it). You can set each one per run with a flag, or as a default in the [Global Configuration File](#global-configuration-file). Prereleases and versions above a range's upper bound are also removed from the candidates ([Version Specifiers and Rewriting](#version-specifiers-and-rewriting)).

### Age Filter

The age filter only considers versions that were released at least a given time ago, so a new release is not adopted as soon as it is published. **A one-week (`1w`) age filter applies by default** unless you override it:

```bash
# Default — implicit --age 1w
depup

# Only update to versions at least 2 weeks old
depup --age 2w

# Only update to versions at least 10 days old
depup --age 10d

# Only update to versions at least 1 month old
depup --age 1m

# Disable the age filter for this run
depup --no-age
```

The age filter applies to the versions depup writes into manifests; for transitive dependencies during `--install`, see [Transitive Dependencies and the Age Filter](#transitive-dependencies-and-the-age-filter). The GitHub Tags API does not return per-tag release timestamps, so the filter has no practical effect on Swift packages: every tag passes regardless of the cutoff. Cargo git dependencies are not subject to the age filter either.

#### Resolution Priority

A minimum release age declared in the project (pnpm's or Bun's `minimumReleaseAge`, or mise's `minimum_release_age`) is treated as the **project policy** and takes precedence over any CLI or config value. The age that applies to a run is resolved in this order (highest first):

1. **Project policy** from pnpm, Bun, or mise settings (see the sources below)
2. CLI `--age <DURATION>` or `--no-age` (the two cannot be combined; `--no-age` only takes effect when no project policy is set)
3. `age` in `~/.config/depup/config.toml` (see [Global Configuration File](#global-configuration-file))
4. Built-in default `1w`

When a project policy overrides the CLI value, depup prints a yellow warning so the active source is visible:

```
⚠ --age ignored: project's minimumReleaseAge (14 days from pnpm-workspace.yaml) takes precedence
```

`--age` and `--no-age` cannot override a project policy. To use a different age, change or remove the setting in the project file.

#### Supported `minimumReleaseAge` Sources

**pnpm** (checked in this order; the first value found wins):
- `.npmrc` (`minimum-release-age=10d`)
- `pnpm-workspace.yaml` (`minimumReleaseAge: 14400` in minutes)
- `package.json` (`pnpm.settings.minimumReleaseAge`)

**Bun** (`bunfig.toml`):

```toml
[install]
minimumReleaseAge = 259200  # seconds (e.g. 3 days)
```

**mise** (`[settings]` in `mise.toml` or another mise config file; note that `m` means minutes, see [mise and the Age Filter](#mise-and-the-age-filter)):

```toml
[settings]
minimum_release_age = "7d"  # s / m (minutes) / h / d / w / M / y
```

If more than one of pnpm, Bun, and mise sets a value, depup uses the **stricter** (larger) one. Within pnpm, only the first value found is used.

### Vulnerability Check (OSV.dev)

depup looks up the version it is about to adopt in the public [OSV.dev](https://osv.dev/) database. If that version has a known vulnerability, depup removes it from the candidates and checks the next newest one. **This check is enabled by default** — no flag required. Combined with the age filter, depup picks the newest version that is both mature and free of known vulnerabilities:

```bash
# OSV check runs by default
depup

# Same as above (explicit opt-in, overrides `osv = false` in global config)
depup --osv

# Disable OSV check for this run (overrides global config and default)
depup --no-osv
```

- The OSV.dev API is public and **does not require any authentication token**.
- Swift packages are not checked: OSV identifies Swift packages by their full repository URL, while depup identifies them by GitHub `owner/repo`, so the lookups would not match.
- mise tools are not checked — each backend has its own version scheme and namespace, so they cannot be mapped onto a single OSV ecosystem. Cargo git dependencies are not checked either.
- A failed OSV lookup does not block the update: the version is adopted without a vulnerability check, so it does not get the `✓ OSV` mark. The failure is listed in the `Errors:` section (the `errors` array in JSON) and does not change the exit code.

**Priority order (highest first):**
1. CLI `--osv` or `--no-osv` (the two cannot be combined)
2. `osv` in `~/.config/depup/config.toml`
3. Built-in default (`true`: the check runs)

To turn the check off for every run, set `osv = false` in the [Global Configuration File](#global-configuration-file).

#### Fallback Example

When the version depup is about to adopt has a known vulnerability, depup removes it from the candidates and checks the next newest one, until it finds a safe version or no newer candidate is left. Updates that pass the OSV check are marked with `✓ OSV`:

```
$ depup --install --include-pinned
  ⚠ OSV: dompurify 3.4.8 vulnerable (GHSA-vxr8-fq34-vvx9)
./package.json (Node.js) — 9 updates, 41 skips
  @mui/icons-material   9.0.1 → 9.1.0 [minor] (2026/06/08 08:30) ✓ OSV
  @mui/material         9.0.1 → 9.1.0 [minor] (2026/06/08 08:29) ✓ OSV
  @tanstack/react-query 5.100.14 → 5.101.0 [minor] (2026/06/02 19:24) ✓ OSV
  next                  16.2.6 → 16.2.9 [patch] (2026/06/09 23:02) ✓ OSV
  openai                6.39.1 → 6.42.0 [minor] (2026/06/03 22:39) ✓ OSV
  react                 19.2.6 → 19.2.7 [patch] (2026/06/01 18:00) ✓ OSV
  react-dom             19.2.6 → 19.2.7 [patch] (2026/06/01 18:01) ✓ OSV
  @types/node           25.9.1 → 25.9.2 [patch] (2026/06/05 22:33) ✓ OSV 🔧
  @types/react          19.2.15 → 19.2.17 [patch] (2026/06/05 20:10) ✓ OSV 🔧

Errors:
  ✗ OSV check for dompurify: 3.4.8 vulnerable, falling back (GHSA-vxr8-fq34-vvx9)

Summary:
  9 package(s) updated (4 minor, 5 patch)
  41 package(s) skipped
```

In this run, no other dompurify candidate qualified after 3.4.8 was removed, so dompurify was not updated and is one of the 41 skips. When the fallback does find a safe version, the update line is followed by `↳ OSV skipped: <version> (<advisory>)`.

The `falling back` entry is informational: depup avoided a vulnerable version as designed, so it does not affect the exit code. The `⚠ OSV:` line above the report is progress output on stderr: it appears only when stderr is a terminal, except with `--quiet`, where it is always printed. In the default text output, the same information also appears in the `Errors:` section (`errors` in JSON), so it is not lost in CI.

### Limiting Bumps (`--max-change`)

Use `--max-change <LEVEL>` to limit how large a version bump depup may make:

```bash
# Only allow patch bumps (1.0.0 → 1.0.5 is allowed, 1.0.0 → 1.1.0 is not)
depup --max-change patch

# Allow patch and minor bumps (1.0.0 → 1.5.3 is allowed, 1.0.0 → 2.0.0 is not)
depup --max-change minor

# Default — allow all bumps including major
depup --max-change major
```

When every newer candidate exceeds the cap, the dependency is skipped with reason `max-change=<LEVEL>`; if some newer candidates are within the cap, depup updates to the newest of them. Cargo git dependencies that track a tag work differently: only the newest tag is considered, and if it exceeds the cap, the dependency is skipped with `max-change=<LEVEL>`.

**Priority order (highest first):**
1. CLI `--max-change <LEVEL>`
2. `max_change` in `~/.config/depup/config.toml`
3. Built-in default (no cap)

### Global Configuration File

On its first run, depup creates `~/.config/depup/config.toml`, which lists the default settings with explanatory comments; it never overwrites an existing file. Edit it to change the defaults for every project. A command-line flag still wins for a single run (see the priority lists above). The generated file looks like this:

```toml
# depup global configuration
# https://github.com/owayo/depup
#
# This file is auto-generated on first run.
# Edit values below to override depup's built-in defaults.

# Default age filter applied to every depup run.
# Accepts the same format as --age: Nd (days), Nw (weeks), Nm (months).
# Override per-run with --age <DURATION> or disable with --no-age.
age = "1w"

# Check candidate versions against the OSV.dev vulnerability database
# and skip versions with known vulnerabilities (enabled by default).
# Requires network access; on API errors depup keeps the original candidate.
# Override per-run with --osv / --no-osv.
osv = true

# Limit the maximum allowed version change.
# Accepts: "patch" (allow only patch bumps), "minor" (allow patch + minor),
# or "major" (default — all bumps allowed).
# Override per-run with --max-change <LEVEL>.
# max_change = "minor"
```

A key that is missing from the file falls back to the built-in default (`age = "1w"`, `osv = true`, no `max_change` limit). If the file cannot be created or parsed, depup prints a warning and uses the built-in defaults. If a single value is invalid (such as `age = "abc"`), depup prints a warning and uses the built-in default for that setting only.

## Version Specifiers and Rewriting

This chapter covers the rules shared by every ecosystem: which specifiers count as pinned, how ranges are advanced, which formats are preserved, which constraints are left alone, how candidates are ordered, and how manifests are written. Rules that apply to a single ecosystem are collected in [Ecosystem Details](#ecosystem-details).

### Pinned Versions and `--include-pinned`

Pinned versions are treated as intentional and skipped as `pinned` by default:

| Language | Example | Updated by default |
|----------|---------|--------------------|
| Node.js | `"1.2.3"` | ❌ |
| Node.js | `"^1.2.3"`, `"~1.2.3"`, `"=1.2"` | ✅ |
| Python | `"==1.2.3"`, Poetry `"1.2.3"` | ❌ |
| Python | `">=1.2.3"`, `"^1.2.3"` | ✅ |
| Rust | `"=1.2.3"` | ❌ |
| Rust | `"1.2.3"`, `"^1.2.3"` | ✅ |
| Go | `v1.2.3` with a `// pinned` comment | ❌ |
| Go | `v1.2.3` | ✅ |
| Ruby | `'1.2.3'`, `'= 1.2.3'` | ❌ |
| Ruby | `'~> 1.2.3'`, `'>= 1.2.3'` | ✅ |
| PHP | `"1.2.3"` | ❌ |
| PHP | `"^1.2.3"`, `"~1.2.3"` | ✅ |
| Java | Fixed version in Gradle (`'g:a:1.2.3'`) | ❌ |
| Java | Strict version in Gradle (`1.2.3!!`) | ❌ |
| Java | Maven hard requirement (`[1.0]`) | ❌ |
| Java | Dynamic or range version in Gradle (`5.3.+`, `[1.7, 1.8[!!`) | ✅ |
| Swift | `exact: "1.2.3"` | ❌ |
| Swift | `from: "1.2.3"`, `.upToNextMinor` | ✅ |
| mise | `node = "26.7.0"` | ✅ |

Use `--include-pinned` to update pinned versions. Without it, depup does not look up pinned versions at all, so the output does not show whether a newer version exists.

> **Caution**: Go and mise are exceptions. depup updates their exact versions even without `--include-pinned`. To keep a Go dependency as it is, add a `// pinned` comment to its line ([Go](#go)). To keep a mise tool as it is, pass `--exclude <tool>`; the name is matched in every language, so `--exclude node` also excludes an npm package named `node`.

### Upper and Lower Bounds of Ranges

depup respects upper-bound range constraints (both exclusive and inclusive):

```
">=3.5.0,<4.0.0"   → ">=3.9.1,<4.0.0"
">=1.0,<=2.0"      → ">=2.0,<=2.0"
"4.0.0..<5.0.0"    → "4.99.0..<5.0.0"
"4.0.0...4.9.9"    → "4.9.9...4.9.9"
"1.2.0 - 2.0.0"    → "1.9.3 - 2.0.0" (npm hyphen)
"1.0 - 2.0"        → "2.0.9 - 2.0" (npm/Composer partial upper expands to `<2.1`)
"[1.0,2.0)"        → "[1.9.3,2.0)" (Maven-style)
"[1.0,2.0]"        → "[2.0,2.0]" (Maven-style)
"[1.0,2.0.Final)"  → "[1.9.3,2.0.Final)" (Maven qualifier)
"[1.0,2.0-beta1-SNAPSHOT)" → "[1.9.3,2.0-beta1-SNAPSHOT)" (multi-part Maven qualifier)
"[1.0,2.0["        → "[1.9.3,2.0[" (Maven alt upper bracket)
"<4.0.0"           → skipped (upper-bound only)
">1.0.0"           → skipped (exclusive lower bound)
"]1.0,2.0["        → skipped (exclusive Maven lower bound)
```

When a dependency has a range with an upper bound (e.g., `>=3.5.0,<4.0.0`, `>=1.0,<=2.0`, `4.0.0...4.9.9`), depup will:
- **Not propose** versions that exceed the upper bound
- **Accept** versions equal to an inclusive upper bound (`<=`, `...`)
- **Preserve** the original constraint shape in the manifest file
- **Update only the lower-bound side** to the newest compatible version within the range

For npm/Composer hyphen ranges, a partial right-hand side like `1.0 - 2.0` is interpreted as a wildcard-expanded exclusive upper bound, so `2.0.x` versions remain candidates while `2.1.0` and later do not.

### Preserving the Original Format

depup preserves the original version range format. For specifiers whose width depends on how many segments are written, such as tilde (`~`), the segment count is kept too. The examples in the last group are exact versions, so they change only with `--include-pinned`:

```
# Operators and tilde width (npm / Cargo / Composer / RubyGems)
"^1.2.3" → "^2.0.0"  (caret preserved)
"~1.2.3" → "~1.3.0"  (tilde preserved)
"~1.2"   → "~1.9"    (segment count preserved — adding a segment would narrow the range)
"~1"     → "~2"      (single-segment tilde keeps its major-level width)
"~1.2 <2.0.0" → "~1.9 <2.0.0" (tilde inside a comparator set keeps its segment count too)
"~1, <5.0" → "~4, <5.0" (Cargo multi-requirement, tilde width preserved)
"~> 7.0" → "~> 8.1"  (RubyGems pessimistic operator, segment count preserved)
">=1.0.0" → ">=2.0.0" (range preserved)

# Python (pyproject.toml)
"requests (>=2.28,<3); python_version < '3.12'" → "requests (>=2.31,<3); python_version < '3.12'" (PEP 508 parentheses and marker preserved)
"coverage [toml] >=7,<8" → "coverage [toml] >=7.6,<8" (PEP 508 extras spacing preserved)
"'paramiko>=3.5.0,<4.0.0,'" → "'paramiko>=3.9.1,<4.0.0,'" (PEP 508 trailing comma preserved)
"'paramiko>=3.5.0,<4.0.0'" → "'paramiko>=3.9.1,<4.0.0'" (TOML literal string quote preserved)

# Wildcards, x-ranges, and partial versions
"1.x" → "2.x" (wildcard shape preserved)
"1.2.x - 2.3.x" → "1.9.x - 2.3.x" (npm hyphen range with x-range endpoints)
"1.x.x" → "2.x.x" (all wildcard positions preserved)
"1.2.*" → "1.3.*" (wildcard shape preserved)
"v1.*" → "v2.*" (leading `v` preserved)
"V1.*" → "V2.*" (Composer uppercase `V` preserved)
"^1.x" → "^2.x" (npm caret + x-range, operator preserved)
"~1.2.x" → "~2.3.x" (npm tilde + x-range, operator preserved)
"=1.x" → "=2.x" (npm equality + x-range, operator preserved)
"=1.2" → "=2.3" (npm partial comparator, operator preserved)

# Gradle / Maven
"5.3.+" → "5.4.+" (Gradle prefix preserved)
"5.3.+!!" → "6.1.+!!" (Gradle strict dynamic prefix preserved)
"[1.7, 1.8[!!" → "[1.7.36, 1.8[!!" (Gradle strict range without a preferred version)
prefer("1.7.25") → prefer("1.7.36") (Gradle rich version inside a strict range)
"org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.25" → "org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.36" (Gradle strict range shorthand with prefer)

# Gradle / Maven exact versions (updated only with --include-pinned)
"1.2.3!!" → "2.0.0!!" (Gradle strict preserved)
"[1.0]" → "[2.0]" (Maven hard requirement preserved)
"[1.2.3.Final]" → "[1.3.0]" (Maven hard requirement with qualifier)
group = "com.google.guava", name = "guava", version = "32.1.2-jre" → version = "33.4.0-jre" (Gradle Kotlin map notation)
junit = "junit:junit:4.13.2" → "junit:junit:4.13.3" (Gradle version catalog library)
guava = "32.1.2-jre" → "33.4.0-jre" (Gradle version catalog version reference)
"group:name:1.0.0:classifier@zip" → "group:name:1.1.0:classifier@zip" (Gradle classifier/extension preserved)
```

### Constraints Left Unchanged

Constraints that cannot be rewritten safely are skipped instead of being rewritten partially. If a newer version exists, the reason is `parse error: constraint cannot be updated safely`; otherwise the dependency is reported as `latest`. Examples:

- OR constraints in npm/Composer (`^1 || ^2`), including Composer's backward-compatible single-pipe spelling (`^1 | ^2`)
- Exclusion constraints containing `!=` (`!=1.2.3`, `>=1.0, !=1.5.0, <2.0`), or Composer's `<>` spelling of not-equal (`>=1.0 <>1.5.0 <2.0`, `>=1.0,<>1.5.0,<2.0`)
- Upper-bound-only constraints (`<4.0.0`, `<=2.0`)
- Strict lower bounds (`>1.0.0`)
- Maven-style ranges without a lower bound (`(,2.0]`) or with an exclusive lower bound (`]1.0,2.0[`)

npm has no `!=` comparator, so an npm constraint containing one is not processed at all.

Floating selectors such as `"*"`, npm dist-tags like `"latest"`, and Gradle dynamic selectors (`"latest.release"`, `"latest.integration"`, `"latest.milestone"`, and any user-defined `latest.<status>`) are not processed, so they are never turned into exact versions. Multi-segment fully-floating wildcards without a numeric anchor (Composer's `*.*`, `v*`, `V*`, `x.x`) and empty Maven ranges (`[,]`, `(,)`) are not processed either, to prevent phantom updates or "always outdated" misjudgments. Wildcard tokens (`x`/`X`/`*`) followed by numeric segments (`1.x.3`, `^x.0.0`) are invalid x-ranges in node-semver / semver and would produce malformed output, so they are not processed.

### Candidate Ordering and Prereleases

Version candidates are ordered with ecosystem-specific rules. Node.js, Rust, Go, and Swift use SemVer (including numeric prereleases such as `1.0.0-1`) and ignore build metadata when comparing precedence, so `1.1.3` and `1.1.3+spec-1.1.0` do not trigger a metadata-only update. Python uses PEP 440 normalization; Ruby follows RubyGems segment ordering and treats alphabetic or hyphenated versions as prereleases; Composer patch aliases (`-p1`, `-pl1`, `-patch1`) sort after the corresponding release; and Java uses Gradle's documented version ordering. Numeric components are compared without a fixed integer-size limit.

Prereleases (alpha, beta, rc, canary, dev, and similar) are removed from the candidates while the current version is stable. If the current version is already a prerelease, prerelease candidates are kept so it can move on to the next prerelease or to the final release. There is no option to offer prereleases to a stable dependency. Versions whose suffix marks them as deprecated are treated the same way, so `serde_yaml 0.9.33` is not moved to `0.9.34-deprecated`.

### Writing Rules

depup only rewrites the dependency declarations it parsed. Other sections of a manifest are left untouched even when they contain package names and versions — for example `overrides` in `package.json`, `replace` / `provide` / `conflict` in `composer.json`, and metadata tables in `Cargo.toml` or `pyproject.toml` ([Ecosystem Details](#ecosystem-details) lists the exact sections). When a value is rewritten, the surrounding syntax is kept; in TOML manifests, both basic strings (`"..."`) and literal strings (`'...'`) keep their quote style.

If the same dependency key is declared more than once in a manifest, or several dependencies share one Gradle version variable or version-catalog `version.ref`, depup cannot tell which declaration to change. It refuses the write and reports an error (exit code 2) rather than silently changing a declaration that should stay as it is, such as a pinned one.

## Running the Package Manager (`--install`)

With `--install`, depup runs each project's package manager after writing the manifests, so lock files and installed packages follow the new versions.

- An install runs only for manifests that received at least one update, and never with `--dry-run`.
- Without [`.depup`](#depup-configuration-file), every install runs in the target directory (the `PATH` argument, or the current directory), even when the updated manifest belongs to a workspace member. With `.depup`, each install runs in the deepest listed directory that contains the updated manifest, so nested apps install in their own directories.
- Installs run one at a time, in directory path order, and each language runs at most once per directory. The package manager's output is captured instead of streamed; its stderr is printed only when the install fails.

If an install fails, depup still runs the remaining installs, prints the failed command with the package manager's stderr, and exits with code 1 at the end (`Error: Some package manager installs failed`). A package manager that is not installed counts as a failure. Manifests that were already rewritten are not rolled back, and the Rust age audit ([below](#transitive-dependencies-and-the-age-filter)) does not run for any project.

### Commands per Package Manager

The package manager is detected from files in the directory where the install runs; parent directories are not searched. Within each language, the first match wins:

| Language | Detected by | Command |
|----------|-------------|---------|
| Node.js | `pnpm-lock.yaml` | `pnpm install` |
| | `yarn.lock` | `yarn install` |
| | `bun.lock` / `bun.lockb` | `bun install` |
| | `package-lock.json`, or `package.json` alone | `npm install` |
| Python | `uv.lock` | `uv sync` |
| | `poetry.lock` | `poetry install` |
| | `requirements.lock` / `requirements-dev.lock` | `rye sync` |
| | `Pipfile.lock` | `pipenv install` |
| | `pyproject.toml` / `requirements.txt` | `pip install -e .` |
| Rust | `Cargo.toml`, or `src-tauri/Cargo.toml` in a Tauri project (`cargo update` then runs in `src-tauri/`) | `cargo update` |
| Go | `go.mod` | `go mod download` |
| Ruby | `Gemfile` | `bundle install` |
| PHP | `composer.json` | `composer update` |
| Java | `gradlew` | `./gradlew dependencies` |
| | `build.gradle` / `build.gradle.kts` | `gradle dependencies` |
| Swift | `Package.swift` | `swift package resolve` |
| mise | any of the [mise config files](#files) | `mise install` |

If none of the listed files is present, no install runs for that language, and nothing is printed. When the age filter is active, pnpm, uv, and mise also receive the age setting ([below](#transitive-dependencies-and-the-age-filter)), and `uv sync` always runs with `UV_MALWARE_CHECK=1` ([uv Malware Check](#uv-malware-check-preview)).

When `--install` processes a PHP project, depup runs `composer update` rather than `composer install`. `composer install` reuses the existing lock file and cannot reflect constraints that depup has just changed in `composer.json`; `composer update` resolves those constraints and refreshes `composer.lock`.

### Transitive Dependencies and the Age Filter

The age filter decides which versions depup writes into manifests. Whether it also reaches transitive dependencies during `--install` depends on the package manager. The age passed here is the same value that was resolved for the update ([Resolution Priority](#resolution-priority)):

| Package manager | What depup passes | Transitive dependencies |
|-----------------|-------------------|-------------------------|
| pnpm | `npm_config_minimum_release_age=<minutes>` (environment variable) | Filtered by pnpm v10.16 or later; older versions ignore the variable |
| uv | `--exclude-newer <timestamp>` | Filtered when uv resolves them |
| Cargo | Nothing; depup audits `Cargo.lock` after `cargo update` | Violations are rolled back (see below) |
| mise | `MISE_MINIMUM_RELEASE_AGE=<seconds>s` (environment variable) | mise tools have no transitive dependencies; `mise install` applies the age when it resolves a partial version such as `node = "26"` |
| npm, Yarn, Bun, pip, Poetry, Rye, Pipenv, Go, Bundler, Composer, Gradle, SwiftPM | Nothing | Not filtered; only direct dependencies follow the age filter |

With `--verbose`, depup prints a note naming the package managers for which the age filter covers direct dependencies only. With `--no-age` and no project policy, nothing age-related is passed and the Rust audit does not run.

For Rust, depup audits the crates whose locked version the install changed, and rolls back any that violate the age filter:

```
⠙ Auditing hyper [██████████████████████▓░░░░░░░] 18/24 (6s)
  . — 1 transitive dep(s) rolled back to satisfy --age:
    hyper 1.11.1 → 1.11.0
```

Only changed entries are audited because depup limits crates.io requests to one per second, following its crawler policy; auditing an entire lock file (often hundreds of crates) would stall the run for several minutes with nothing on screen. The audit is capped at 180 seconds per `Cargo.lock`; any crates left over are reported as unchecked. If `Cargo.lock` did not exist before the install, every entry counts as changed, so the cap is more likely to be reached. Rollbacks are always reported, crates that could not be rolled back are listed with `--verbose`, and the audit never changes the exit code.

### uv Malware Check (Preview)

When `--install` triggers `uv sync` for a Python project, depup always sets `UV_MALWARE_CHECK=1` in the environment. This enables [uv's preview malware check](https://astral.sh/blog/uv-audit) — a feature announced alongside `uv audit` but distinct from the `uv audit` command. On every sync operation (`uv add`, `uv sync`, etc.), uv cross-references the currently locked resolution against OSV's MAL advisories and terminates the sync before any malicious package is installed.

- Always on — no opt-in flag required.
- Older uv releases that predate the feature simply do not act on the variable, so enabling it unconditionally does not break existing builds.
- Astral marks the feature as preview, so its exact behavior may change.
- The check runs inside uv: when it finds malware, uv aborts the sync with an error, and depup reports a failed install and exits with code 1.

## Output and Exit Codes

### Progress Display

<p align="center">
  <img src="docs/images/scanning.png" alt="depup scanning">
</p>

### Text Output (Default)

- `🔧` indicates development dependencies (such as devDependencies)
- Release date shown in `(yyyy/mm/dd HH:MM)` format
- Change type: `[major]`, `[minor]`, `[patch]`
- `✓ OSV` marks versions that passed the vulnerability check. A `↳ OSV skipped:` line below an update lists the vulnerable versions that were removed from the candidates; the dependency itself was still updated
- Skipped dependencies are counted in each manifest heading (`— N updates, M skips`) and in the summary; a manifest with no updates also shows the count per reason
- With `--verbose`, every skipped dependency is listed under its reason, and the summary is broken down by reason and language
- Errors, including OSV notices, are listed in the `Errors:` section

### JSON Output

```bash
depup --json
```

```json
{
  "manifests": [
    {
      "path": "package.json",
      "language": "node",
      "updates": [
        {
          "type": "update",
          "dependency": {
            "name": "lodash",
            "version_spec": "^4.17.20"
          },
          "new_version": "4.17.21",
          "released_at": "2024-12-15T10:30:00Z"
        }
      ]
    }
  ]
}
```

### Diff Output

```bash
depup --diff
```

```diff
--- package.json
+++ package.json
@@ dependencies @@
-  "lodash": "^4.17.20"
+  "lodash": "^4.17.21"
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | No failures. This includes runs with no updates, dry runs, and runs whose only `Errors:` entries are OSV notices (fallbacks or failed lookups). |
| `1` | A package manager install failed, `--cd` could not change the directory, or depup could not run at all (for example, the HTTP client failed to initialize or the output could not be written). |
| `2` | Part of the run failed: a manifest could not be read, parsed, or written (including a refused ambiguous write), or a registry lookup failed. Invalid command-line arguments also exit with `2`. |

A code-2 failure does not stop the run. A failed lookup skips that dependency, and a manifest that cannot be read or parsed is left out; everything else is still written and installed. A failed write leaves that file unchanged, but its dependencies are still listed as updated and `--install` still runs for it. When both `1` and `2` apply, `1` wins. Three lookup problems leave the exit code unchanged: a failed `git ls-remote` for a Cargo git dependency, a registry that returns no usable versions (`fetch failed: no versions available`), and a missing `mise` command (mise config files are ignored with a warning). There is no dedicated code for "updates available"; use `--json` to inspect the result in CI.

Errors are listed in the `Errors:` section of the text output and in the `errors` array of the JSON output. With `--diff`, or with `--quiet` in text output, the list is not printed unless `--verbose` is also given (it then goes to stderr), so check the exit code to detect failures.

## When a Dependency Is Not Updated

By default, the text output only counts skipped dependencies. Run with `--verbose` to list each one under its reason:

| Reason | Meaning | See |
|--------|---------|-----|
| `latest` | No version newer than the current one passes the filters. A newer release may exist but be too recent, vulnerable, a prerelease, or above the range's upper bound. | [Filtering Candidates](#filtering-candidates), [Upper and Lower Bounds of Ranges](#upper-and-lower-bounds-of-ranges) |
| `pinned` | The version is pinned, so depup did not query the registry. | [Pinned Versions and `--include-pinned`](#pinned-versions-and---include-pinned) |
| `max-change=<LEVEL>` | Newer versions exist, but all of them exceed the `--max-change` cap. | [Limiting Bumps](#limiting-bumps---max-change) |
| `excluded` / `not in --only` | The package was excluded with `--exclude`, or `--only` lists other packages. | [Options](#options) |
| `no suitable version` | No published version passes the filters, not even the current one (for example, every release is newer than the age cutoff). | [Filtering Candidates](#filtering-candidates) |
| `parse error: ...` | depup read the constraint but cannot rewrite it safely (for example, `parse error: constraint cannot be updated safely`). Despite the label, the manifest itself parsed correctly, and the exit code is not affected. | [Constraints Left Unchanged](#constraints-left-unchanged) |
| `fetch failed: ...` | The version lookup failed, for example because of a registry error or a failed `git ls-remote`. Whether the exit code changes depends on the cause. | [Exit Codes](#exit-codes) |

These are the labels in the text output. In `--json` output with `--verbose`, the same reasons appear as `already_latest`, `pinned`, `change_level_limited: <LEVEL>`, `excluded`, `not_in_only_list`, `no_suitable_version`, `parse_error: ...`, and `fetch_failed: ...`.

If a dependency does not appear in the output at all, depup did not process the declaration; check in [Ecosystem Details](#ecosystem-details) that its file, section, and declaration form are supported. Manifests of languages left out by a language flag such as `--node` are not parsed (project age settings in pnpm, Bun, and mise files are still read). If the manifest was updated but the install failed, see [Running the Package Manager](#running-the-package-manager---install).

## Monorepo Support

### `.depup` Configuration File

For monorepo projects with multiple subdirectories, create a `.depup` file at the project root to list additional directories to process:

```
# .depup
gui       # Frontend app
api       # Backend API
shared    # Shared libraries
```

Run `depup` from the root directory to update dependencies across all listed directories at once. The root directory itself is always scanned in addition to the listed directories. Version lookups are cached, so shared packages are only fetched once.
When `--install` is used, depup runs each package manager in the deepest listed directory that contains the updated manifest, so nested apps install in their own directories instead of the repository root ([Running the Package Manager](#running-the-package-manager---install)).

The `.depup` format:

- `#` starts a comment (line or inline)
- Empty lines are ignored
- Paths are relative to the `.depup` file location
- If any entry is an absolute path, contains `..`, or is a symlink that resolves outside the directory containing `.depup`, depup prints a warning and ignores the whole `.depup` file. The run then behaves as if there were no `.depup`: the root and its automatically detected workspaces are still processed
- Non-existent directories are ignored with a warning

### pnpm Workspaces

depup detects `pnpm-workspace.yaml` and processes all workspace packages. Both block-style (`- 'packages/*'`) and flow-style (`packages: ['packages/*', 'apps/*']`) `packages` arrays are supported, including negation patterns (`!packages/legacy`).

### Cargo Workspaces

depup expands `[workspace] members` (including glob patterns like `crates/*`) and leaves out entries listed in `[workspace] exclude`.

### Go Workspaces

depup expands the `use` directives in `go.work` (both the single-line and `use ( ... )` block forms) and processes each member module's `go.mod`. Without this, a repository whose root has no `go.mod` would report "no updates" even though every member has outdated dependencies.

### Gradle Multi-Project Builds

depup expands `include ':app', ':core'` from `settings.gradle` / `settings.gradle.kts` (both the Groovy and Kotlin DSL forms) and processes each subproject's `build.gradle` / `build.gradle.kts`, plus `buildSrc/`. Most dependency declarations live in subprojects, so scanning only the root build file would miss them.

Paths expanded from `go.work` and `settings.gradle` are subject to the same containment checks as `.depup`: absolute paths, `..` traversal, and symlinks resolving outside the project are rejected.

### Tauri Projects

depup automatically detects `src-tauri/Cargo.toml` in Tauri projects.

#### Tauri Version Synchronization

Tauri projects require the npm `@tauri-apps/api` package and the Rust `tauri` crate to have matching major/minor versions. depup automatically synchronizes these versions to prevent build errors.

```
# Error example (version mismatch)
Found version mismatched Tauri packages:
  tauri (v2.10.1) : @tauri-apps/api (v2.9.1)

# depup automatically synchronizes versions
@tauri-apps/api: 2.9.1 → 2.10.0
tauri: 2.9.0 → 2.10.1
```

Both packages are automatically adjusted to the same major.minor version (e.g., 2.10.x).

## Ecosystem Details

This chapter is a reference. Look up your ecosystem when you need to know exactly which declarations depup reads and how it rewrites them; the shared rules are in [Version Specifiers and Rewriting](#version-specifiers-and-rewriting).

### Node.js

In `package.json`, depup updates `dependencies`, `devDependencies`, `peerDependencies`, and `optionalDependencies`; `overrides` and other sections are left untouched.

depup accepts the node-semver-compatible legacy tilde spelling `~>1.2.3` and preserves `~>` when updating.

For npm partial comparators, `=1.2` and `=1` follow node-semver's partial-version rules instead of being treated as pinned exact versions: in node-semver, `=1.2` means any 1.2.x (`>=1.2.0 <1.3.0`). depup keeps the `=` operator and updates only the visible segment shape (`=1.2` → `=2.3`, `=1` → `=2`).

For npm comparator sets, depup supports bare partial lower bounds such as `1.2 <2.0.0` and preserves the partial shape when updating the lower side.

node-semver's `HYPHENRANGE` accepts `XRANGEPLAIN` on both sides, so x-range endpoints such as `1.x - 2.x` are valid and updatable. Endpoints that depup rejects elsewhere (a digit after a wildcard like `1.x.3`, or a fully floating `*`) stay rejected.

For npm semver tokens, depup validates prerelease and build metadata identifiers before parsing them as updatable constraints. Identifiers with underscores (`1.2.3-rc_1`), empty identifier segments (`1.2.3-alpha..1`), and numeric prerelease identifiers with leading zeroes (`1.2.3-01`) are not processed, rather than being normalized into malformed `package.json` constraints.

Build metadata is stripped before the leading-zero prerelease check. SemVer allows build identifiers to contain hyphens and leading zeroes, so versions such as `1.0.0+2024-01` and `1.2.3+00` remain valid and updatable — only the prerelease segment is validated.

For Bun workspaces, depup parses and updates root `package.json` catalog definitions in both top-level `catalog` / `catalogs` and `workspaces.catalog` / `workspaces.catalogs`. Workspace package references such as `"react": "catalog:"` and `"jest": "catalog:testing"` are kept as catalog references; depup updates the shared catalog entry instead. pnpm catalogs defined in `pnpm-workspace.yaml` are not yet parsed as manifests, so `package.json` `catalog:` references backed by pnpm catalogs are not processed.

### Python

Beyond PEP 621, PEP 735, and Poetry, depup also reads uv's legacy `[tool.uv] dev-dependencies` and PDM's `[tool.pdm.dev-dependencies]`. Dependencies routed through `[tool.uv.sources]` to a non-PyPI source (`workspace = true`, `git`, `path`, `url`, or an `index` other than `pypi`) are not processed, so a workspace member or custom index is never overwritten with a same-named PyPI package. Poetry multiline dependency tables (`[tool.poetry.dependencies.<name>]`, `[tool.poetry.group.<g>.dependencies.<name>]`) and TOML quoted keys (`"zope.interface"`, `"ruamel.yaml"`) are parsed and updated too (names containing a dot must be quoted in TOML).

In `[project]`, `[tool.rye]`, and `[tool.uv]` sections, depup rewrites only the `dependencies` / `dev-dependencies` arrays; metadata strings such as `name`, `description`, and `keywords` are never modified even if they look like PEP 508 specifiers. Python PEP 508 version lists may include a trailing comma, such as `>=3.5,<4,`; depup parses and preserves that comma when updating the lower bound. Poetry dependencies with a non-`pypi` `source` are not processed, including PEP 621 dependencies enriched by `tool.poetry.dependencies`, because depup only queries PyPI. Poetry's multiple-constraints array form (`foo = [{version = "<=1.9", python = ">=3.6,<3.8"}, {version = "^2.0", python = ">=3.8"}]`) is not processed either, because depup cannot safely rewrite an individual array element without per-element `requires_python` resolution.

A `pyproject.toml` that configures a non-PyPI default index — a Poetry `priority = "primary"` / `"default"` source, a uv `[[tool.uv.index]] default = true` or `[tool.uv] index-url`, or a PDM source overriding `pypi` — none of its dependencies are processed, and depup prints a warning once. depup only queries PyPI, so updating those dependencies would replace private packages with same-named public ones.

Python compatible release clauses follow PEP 440: `~=1.2` (= `>=1.2,<2.0`) and `~=1.2.3` (= `>=1.2.3,<1.3.0`) are treated as ranges with an explicit upper bound, so updates stay within the compatible range (`~=1.2.3` stays in the 1.2 series, `~=1.2` stays in the 1.x series) and preserve the original segment count (`~=1.2` → `~=1.9`; writing `~=1.9.0` would narrow the upper bound to `<1.10.0`). The invalid single-segment form `~=1` is not processed.

PEP 440 prefix matching is accepted only for release-segment `==` / `!=` specifiers such as `==1.2.*` and `!=1.2.*`. Invalid prefix forms such as `>=1.0.*`, `~=1.0.*`, `==1.0a1.*`, `==1.0.post1.*`, and `==1.0+local.*` are rejected at parse time and not processed. Arbitrary equality (`===1.0.*`) remains an exact pinned specifier, not prefix matching.

In Poetry's `[tool.poetry.dependencies]`, a bare version string without an operator (`requests = "2.28.0"`) is an exact pin — Poetry's official "Exact requirements", equivalent to `==2.28.0`. depup parses it as an exact/pinned dependency for both the simple form and inline tables (`{ version = "1.26.0", optional = true }`), so it is handled just like an explicit `==2.28.0` (skipped as `pinned`, updatable with `--include-pinned`) and is rewritten without adding an operator (`4.2.1` → `5.0.0`). This bare-exact interpretation applies only in the Poetry context; pip / PEP 508 requirements still require an operator, so a bare version there is not accepted.

PEP 440 local versions (the `+` label) are handled as Python-specific version semantics rather than semver build metadata. Exact and exclusion specifiers keep their local labels when parsed but are never rewritten (`torch==2.1.0+cu121`, `!=1.0+local1`); when a newer version exists, they are skipped with `parse error: constraint cannot be updated safely`. A local label names a separate build made from the same public version (`cu121` targets CUDA 12.1), so advancing only the public version would point at a build that does not exist. Python candidate ordering treats local versions as newer than the same public version (`1.0+local > 1.0`) and compares local segments per PEP 440 (`1.0+1 > 1.0+abc`, `1.0+abc.2 > 1.0+abc.1`). Ordered and compatible specifiers with local labels, which PEP 440 does not permit (`>=1.0+local`, `~=1.0+local`, `>=1.0+local,<2.0`), are not processed.

PEP 440 prerelease versions are detected and removed from the candidates by default even when written without a separator (e.g., `2.0.0rc1`, `1.0rc1`, `1.0.0a1`), so a stable dependency is never accidentally bumped to a release candidate. Post-releases (`1.0.post1`) compare as newer than the corresponding release, and epochs (`1!2.3`) take precedence in comparison. A post-release attached to a prerelease (`1.0a1.post1`) is also recognized as newer than the underlying prerelease (`1.0a1 < 1.0a1.post1 < 1.0`), so users tracking an alpha can still pick up its post-release fixes.

### Rust

In `Cargo.toml`, dependency updates are limited to dependency tables such as `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, `[workspace.dependencies]`, and target-specific dependency tables; metadata tables are left untouched.

Cargo renamed dependencies such as `alias = { package = "actual-crate", version = "1" }` are fetched by the real package name and written back through the manifest key. `--only` and `--exclude` accept either name.

Path dependencies (`{ path = "../common" }`) are not processed, even when they also declare a `version` for publishing, because they resolve to the local crate. Dependencies that point at a registry other than crates.io — a non-`crates-io` `registry = "..."` or any `registry-index = "..."` — are not processed either, because depup only queries crates.io.

Git dependencies are checked with `git ls-remote` instead of a registry. A `tag` is updated to the newest stable semver tag; if that tag exceeds the `--max-change` cap, the dependency is skipped with `max-change=<LEVEL>` (older tags within the cap are not considered). A `branch` (or the default branch) is reported as updated when the remote head differs from the commit recorded in `Cargo.lock`, or when none is recorded. `Cargo.toml` stays as written, and the new commit is picked up only when `--install` runs `cargo update`; without `--install`, nothing is written. A `rev` is always skipped as `pinned`, even with `--include-pinned`. Tag updates are limited to the same dependency tables as version updates (in both inline and multiline form), preserve single or double quotes, and also support `[patch.<registry>]` / `[patch.<registry>.<package>]`. Git dependencies are not subject to the age filter or the OSV check, and a failed `git ls-remote` skips the dependency without changing the exit code.

Cargo comparison ranges may contain more than two comma-separated requirements, for example `>=1.0, <2.0, >=1.0.100`. Mixed multi-requirement constraints that combine caret/tilde/wildcard with comparators, such as `^1.2.2, <1.5`, are validated with `semver::VersionReq` and detected as ranges. Mixed constraints without an upper bound, such as `>=1.2.3, ^1.3`, cannot be rewritten safely and are skipped.

### Go

`go.mod` has no range syntax such as `^` or `~` and only holds exact versions, so depup does not read an exact Go version as an intentional pin. Go dependencies without a `// pinned` comment are updated regardless of `--include-pinned`. To keep one, add `// pinned` to the end of its line; it is then skipped as `pinned` unless `--include-pinned` is given. The word is recognized anywhere in the comment, so `// indirect; pinned` works too.

Versions listed in Go `exclude` directives are removed from the candidates; the directives themselves are not rewritten. Versions retracted by the upstream module's latest `go.mod` are also removed, including closed retract ranges. For modules without tagged versions, depup falls back to the Go Proxy `@latest` endpoint; an omitted `.info` `Time` uses the Unix epoch so an unknown release date is not permanently filtered by `--age`.

Go versions tagged `+incompatible` are handled with the same rule the `go` command applies: once a `+incompatible` version is reached in semver order, it and everything above it are removed from the candidates if the preceding compatible version has a real `go.mod` (not one synthesized by the Go proxy). Without this, a module like `github.com/libp2p/go-libp2p` would be "updated" from `v0.49.0` to a 2018-era `v6.0.23+incompatible`, and because that version still builds, the mistake would go unnoticed.

Modules targeted by a `replace` directive are not processed. A `replace` without a version covers every `require` of that module, and a versioned `replace` covers only the `require` with the same version. Updating the `require` alone would stop the `replace` from matching and silently drop the local patch.

For `go.mod`, depup treats block endings with trailing comments such as `) // direct deps` as normal block endings when parsing and updating `require`, `replace`, and `exclude` blocks.
Quoted `go.mod` module paths and versions, such as `require "golang.org/x/text" "v0.14.0"`, are parsed and updated while preserving the quotes.
`go.mod` updates preserve the original LF or CRLF line endings in both single-line and block `require` declarations.

### Ruby

Gemfile declarations can use either the common Ruby DSL form (`gem "rack", "~> 3.0"`) or parenthesized method-call form (`gem("rack", "~> 3.0")`). Both forms are parsed and updated while preserving the original call style. When the same gem is declared in multiple places (for example both at the top level and inside a `group :test` block), depup refuses the ambiguous write.

Gemfile compound constraints such as `gem "pg", ">= 0.18", "< 2.0"` are parsed and updated. depup advances only the inclusive lower bound and writes it back across the original arguments, preserving their count, order, quote style, spacing, parenthesized call form, and trailing conditional modifiers. The comparison baseline is the inclusive lower bound regardless of the order the constraints are written in, so `gem "pg", "< 2.0", ">= 0.18"` is compared against `0.18`. If the rewritten constraint cannot be split back into the original number of arguments (for example when one argument itself contains a comma), depup reports an error instead of applying an unsafe edit. Exclusion constraints such as `gem "rack", "!= 2.2.4"` are skipped, because replacing part of them can change their meaning.

Gemfile entries that point to non-registry sources without a version (`git:`, `github:`, `bitbucket:`, `gist:`, `path:`, `source:`) are not processed, rather than being converted into RubyGems registry constraints. If such an entry explicitly includes a version, depup treats it as Bundler's gemspec constraint and can parse and update it while preserving the source option. Both Ruby option spellings are recognized — `git: '...'` and the hash-rocket form `:git => '...'`. Gems declared inside `git ... do` / `github ... do` / `path ... do` / `source ... do` blocks are not processed for the same reason, while ordinary blocks such as `platforms` and `install_if` are still processed. Declarations whose arguments continue on the next line (`gem "devise",`) are not processed, rather than being reported as versionless registry gems, because that line alone cannot determine the version. Inline `group:` / `groups:` options are used to classify development dependencies.

Custom Bundler git source shorthands registered with `git_source(:name) { ... }` (for example `gem 'rails', stash: 'forks/rails'`) follow the same rules as the built-in `git:` / `github:` shorthands: a declaration without a version is a non-registry dependency and is not processed.

### PHP

In `composer.json`, depup updates `require` and `require-dev`; sections such as `replace`, `provide`, and `conflict` are left untouched. Composer accepts explicit equality (`=1.2.3`, `==1.2.3`), and depup preserves the operator when updating. Constraints using the `<>` exclusion spelling (`<>1.2.3`) are parsed but skipped rather than rewritten.

Composer platform packages such as `php`, `hhvm`, `ext-*`, `lib-*`, and Composer API packages are not processed. Inline aliases such as `1.0.0 as 1.1.0` are not processed either, because overwriting them with the registry's latest version would break the alias declaration.

Composer/Packagist accepts 1-4 segment numeric versions per `composer/semver`'s `VersionParser`, so depup parses and updates four-segment versions like `1.2.3.4`, `^1.0.0.0`, `~3.4.5.6`, and `1.0.0.*`, while forms with five or more segments are invalid and not processed.

Composer modifiers may omit the separator or use `.` / `_` (`composer/semver` allows `[._-]?`), so depup treats `5.0.0alpha3`, `1.0.0.RC1`, and `1.0.0_beta1` as prereleases and `2.2.1p1` / `2.2.1pl1` / `2.2.1patch1` as patch aliases that sort **above** the base version. Both forms occur on Packagist today (`nikic/php-parser` publishes `5.0.0beta1`; `laminas/laminas-diactoros` ships security patches as `2.2.1p2`).

Composer rejects the `~>` operator (`Invalid operator "~>"`), so for PHP a `~>` constraint is not processed, rather than being rewritten into a constraint Composer cannot read. Node accepts `~>` because node-semver defines it as valid.

### Java (Gradle)

Gradle rich version declarations using `strictly`, `require`, `prefer`, and `reject` are parsed in dependency blocks such as `implementation("org.slf4j:slf4j-api") { version { ... } }`. String notation shorthand supports exact, dynamic-prefix, and range constraints, including `group:name:1.2.3!!`, `group:name:5.3.+!!`, `group:name:[1.7, 1.8[!!`, and a strict range with a preferred version such as `group:name:[1.7, 1.8[!!1.7.25`. When `strictly` or `require` declares a range and `prefer` declares the selected version, depup keeps the range as the upper-bound constraint and updates the `prefer` value. Versions listed with `reject` are removed from the candidates, including dynamic rejects such as `2.+` and ranges such as `[1.5,1.9)`.

Gradle declaration wrappers are supported: `platform(...)`, `enforcedPlatform(...)`, and `testFixtures(...)`. BOM declarations such as `implementation platform('com.google.cloud:libraries-bom:26.1.0')` and `testImplementation(platform("org.junit:junit-bom:5.10.0"))` are parsed and updated, and the surrounding configuration name is still used for the dev/production classification. Gradle variables declared with `ext.<name> = '...'` / `project.ext.<name> = "..."` are resolved alongside `ext { ... }` blocks, and qualified references such as `${Versions.retrofit}` resolve by their final segment. A short name defined more than once with different values is not resolved, so dependencies that reference it are not processed; this avoids picking up the wrong object's value.

Gradle version catalogs under `gradle/*.versions.toml` are detected as Java manifests. depup parses `[libraries]` entries written as `alias = "group:name:version"`, `module = "group:name"`, `group` / `name` / `version`, and `version.ref`; referenced `[versions]` entries are updated in place. Rich version tables with `strictly`, `require`, `prefer`, `reject`, and `rejectAll` follow the same candidate rules as Gradle build files. `[plugins]` entries are not processed because Gradle plugin IDs are not Maven Central coordinates.

For Gradle string notation, depup preserves classifier and extension suffixes such as `:resources@zip` or `@aar`, and ignores declarations that appear only in `//` line comments or `/* ... */` block comments. Gradle version catalog updates preserve the original TOML string or table shape where the version is declared.

Gradle `-SNAPSHOT` / `.SNAPSHOT` versions are not processed. A snapshot is a moving reference that resolves to the newest timestamped build on every resolution, so rewriting it to a fixed release would silently change what the build uses. Stable qualifiers such as `.Final`, `.RELEASE`, `-jre`, and `-SP1` are still updated.

Gradle coordinates declared inside `resolutionStrategy { force ... }`, `constraints { }`, and `dependencySubstitution { }` are not processed. They restate a version that is declared elsewhere, and treating them as separate declarations would make the coordinate ambiguous and block the update.

JVM milestone releases are treated as prereleases and are removed from the candidates of a stable dependency: `4.0.0-M1`, the legacy Spring Boot dot form `2.0.0.M1`, and the spelled-out `-milestone1`. Without this, a project on `assertj-core 3.24.2` would be bumped to `4.0.0-M1`, `junit-bom 5.10.0` to `5.13.0-M3`, and `spring-core 5.3.23` to `7.0.0-M6`. Detection is limited to tokens where `m` is immediately followed by digits, so stable JVM qualifiers (`.Final`, `-jre`, `-android`, `.RELEASE`, `.GA`, `-SP1`) and identifiers like `-macos1` are never misclassified. As with other prereleases, a project already on a milestone keeps milestone candidates so it can advance to the next one.

### Swift

For Swift GitHub dependencies, depup accepts HTTPS URLs, scp-style SSH URLs (`git@github.com:owner/repo.git`), standard SSH URLs (`ssh://git@github.com/owner/repo.git`), and GitHub's SSH-over-443 URLs (`ssh://git@ssh.github.com:443/owner/repo.git`). It recognizes both `v1.2.3` and `V1.2.3` tag prefixes from GitHub tags, while `Package.swift` version requirement strings are validated as strict SemVer (`X.Y.Z`, no leading zeroes). depup also ignores `Package.swift` dependencies that appear inside `//` line comments or `/* ... */` block comments.
Because SwiftPM follows Semantic Versioning 2.0.0, depup parses and updates dependencies that include prerelease identifiers (`1.0.0-beta.1`) and build metadata (`1.0.0+build.123`), including combined forms (`1.0.0-rc.1+sha.abc`).
depup also parses `.package(...)` declarations with trailing arguments such as `traits: [...]` (SPM 6.1) or `moduleAliases: [...]`, updating only the version requirement while preserving the extra arguments. Swift Package Registry `id:` dependencies (`.package(id: "scope.name", ...)`) are not processed yet; support is planned once a registry API adapter is implemented. Only GitHub URL dependencies with a version requirement are processed; non-GitHub URLs and `branch:` / `revision:` requirements are not.

### mise

Tool versions declared in [mise](https://mise.jdx.dev) config files go through the same workflow as every other language (detect → parse → judge → write). Version lists come from `mise ls-remote <tool> --json`, so every backend mise supports (core / aqua / ubi / asdf / `npm:` / `cargo:` / `go:` / `pipx:` …) works out of the box.

#### Files

`mise.toml`, `.mise.toml`, `mise/config.toml`, `.mise/config.toml`, `.config/mise.toml`, `.config/mise/config.toml`, `.tool-versions`

`mise.local.toml` (personal local override, usually gitignored) and `mise.<env>.toml` (per-environment overlay) are intentionally not read, so depup never rewrites a single environment behind your back.

#### Version Specifiers

| Form | Example | Handling |
|------|---------|----------|
| Exact | `node = "26.7.0"` | Updated to the latest version |
| Prefix | `node = "26"` / `"26.7"` | Segment count preserved (`26` → `27`, `26.7` → `26.8`) |
| Explicit selector | `go = "prefix:1.19"` | `prefix:` preserved (`prefix:1.24`) |
| Vendored | `java = "temurin-21.0.5"` | Stays within the same vendor (`temurin-21.0.9`) |
| Inline table | `python = { version = "3.13", virtualenv = ".venv" }` | Only `version` is rewritten; other options are preserved |
| Table form | `[tools.terraform]` + `version = "1.15.0"` | Same as above |
| Floating | `latest` / `lts` / `system` | Not processed — there is no fixed version to update |
| Non-version | `ref:master` / `path:./shfmt` / `sub-2:lts` | Not processed |
| Multiple versions | `python = ["3.12", "3.13"]` | Not processed — there is no single version to rewrite |

Most of the 3000+ entries `mise ls-remote java` returns carry a vendor prefix (`temurin-`, `graalvm-community-`, `zulu-`, …). depup only considers candidates with the same prefix as the current value, so a project on `temurin-21` is never rewritten to `zulu-27`.

Only the `[tools]` section is rewritten; identically named keys under `[settings]`, `[env]`, `[tasks]`, or `[alias]` are untouched. Quote style (`"` / `'`), trailing comments, CRLF line endings, and the column alignment of `.tool-versions` are all preserved.

#### mise and the Age Filter

When `[settings] minimum_release_age` is **explicitly set**, depup treats it as a [project policy](#resolution-priority), just like pnpm's and Bun's `minimumReleaseAge`, so it takes precedence over the CLI `--age`. Only a value written in a config file counts; mise's built-in 24-hour default is ignored. The syntax is shown under [Supported `minimumReleaseAge` Sources](#supported-minimumreleaseage-sources).

> **Caution**: mise's `m` means **minutes** (humantime convention), unlike depup's `--age 1m`, which means one month. `minimum_release_age = "1m"` is read as one minute, and because a project policy overrides the CLI `--age`, it effectively disables the age filter unless pnpm or Bun sets a larger value. Write `"1M"` or `"30d"` for one month.

`mise ls-remote` applies mise's own `minimum_release_age` by default and hides newer releases, so depup passes `--minimum-release-age 0` to fetch everything and keeps age evaluation in one place. `minimum_release_age_excludes` is not interpreted by depup; if it is set, depup prints a warning.

## Build

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Install locally
cargo install --path .
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

[MIT](LICENSE)
