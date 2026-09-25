# Usage Guide

This guide explains how depup chooses the new version of each dependency and how it writes it back. [How depup Decides Updates](#how-depup-decides-updates) gives the overview; the later chapters are references to look up when a result is not what you expected. The options are listed in the [Command-Line Reference](cli-reference.md), and the rules specific to each ecosystem are in [Ecosystems and Monorepos](ecosystems.md).

## How depup Decides Updates

Unless a dependency is pinned, depup looks up its published versions, picks the newest one that passes the checks below, and rewrites the manifest in place. By default:

- An exact version such as `"1.2.3"` in `package.json` is treated as an intentional pin and is not updated unless you pass `--include-pinned`. Go modules and mise tools are exceptions ([Pinned Versions and `--include-pinned`](#pinned-versions-and---include-pinned)).
- Only versions that have been public for at least one week are candidates ([Age Filter](#age-filter)).
- Versions with known vulnerabilities are avoided ([Vulnerability Check](#vulnerability-check-osvdev)).
- Prereleases are not proposed while the current version is stable ([Candidate Ordering and Prereleases](#candidate-ordering-and-prereleases)).
- Ranges keep their shape and upper bound; only the lower bound moves ([Upper and Lower Bounds of Ranges](#upper-and-lower-bounds-of-ranges)).
- Major bumps are allowed unless `--max-change` caps them ([Limiting Bumps](#limiting-bumps---max-change)).

This guide uses two terms for a dependency that depup leaves unchanged:

- **Skipped**: depup recognizes the dependency but does not update it. Skipped dependencies are counted in the output, and `--verbose` lists each one with its reason, such as `pinned` or `latest` ([When a Dependency Is Not Updated](#when-a-dependency-is-not-updated)).
- **Not processed**: depup does not take the declaration as an updatable dependency, so it does not appear in the output at all. This happens when the declaration points to a non-registry source (such as a Cargo `path` dependency; Cargo git dependencies are the exception and are checked with `git ls-remote`), names a platform package (such as `php` in Composer), or writes its version in a floating or unsupported form (such as `"*"` or `latest`).

Filters such as the age filter and the vulnerability check remove candidate versions, not the dependency itself. depup updates the dependency to the newest remaining candidate and reports it as skipped only when no candidate newer than the current version remains.

## Filtering Candidates

Three configurable filters decide which published versions can be chosen: the age filter and the vulnerability check (both on by default) and `--max-change` (off unless you set it). You can set each one per run with a flag, or as a default in the [Global Configuration File](configuration.md#global-configuration-file). Prereleases and versions above a range's upper bound are also removed from the candidates ([Version Specifiers and Rewriting](#version-specifiers-and-rewriting)).

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

A minimum release age declared in the project (pnpm's or Bun's `minimumReleaseAge`, or mise's `minimum_release_age`) is treated as the **project policy** and takes precedence over the CLI `--age` and the global configuration file. The age that applies to a run is resolved in this order (highest first):

1. Project policy from pnpm, Bun, or mise settings (see below for the files read and how multiple values are combined)
2. CLI `--age <DURATION>` or `--no-age` (the two cannot be combined; `--no-age` only takes effect when no project policy is set)
3. `age` in `~/.config/depup/config.toml` (see [Global Configuration File](configuration.md#global-configuration-file))
4. Built-in default `1w`

When a project policy overrides the CLI value, depup prints a yellow warning so the active source is visible:

```text
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

**mise** (`[settings]` in `mise.toml` or another mise config file; note that `m` means minutes, see [mise and the Age Filter](ecosystems.md#mise-and-the-age-filter)):

```toml
[settings]
minimum_release_age = "7d"  # s / m (minutes) / h / d / w / M / y
```

If more than one of pnpm, Bun, and mise sets a value, depup uses the stricter (larger) one. Within pnpm, only the first value found is used.

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

- The OSV.dev API is public and does not require any authentication token.
- Swift packages are not checked: OSV identifies Swift packages by their full repository URL, while depup identifies them by GitHub `owner/repo`, so the lookups would not match.
- mise tools are not checked: each backend has its own version scheme and namespace, so they cannot be mapped onto a single OSV ecosystem.
- Cargo git dependencies are not checked either.
- A failed OSV lookup does not block the update: the version is adopted without a vulnerability check, so it does not get the `✓ OSV` mark. The failure is listed in the `Errors:` section (the `errors` array in JSON) and does not change the exit code.

**Priority order (highest first):**
1. CLI `--osv` or `--no-osv` (the two cannot be combined)
2. `osv` in `~/.config/depup/config.toml`
3. Built-in default (`true`: the check runs)

To turn the check off for every run, set `osv = false` in the [Global Configuration File](configuration.md#global-configuration-file).

#### Fallback Example

When the version depup is about to adopt has a known vulnerability, depup removes it from the candidates and checks the next newest one, until it finds a safe version or no newer candidate is left. Updates that pass the OSV check are marked with `✓ OSV`:

```text
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

## Version Specifiers and Rewriting

This chapter covers the rules shared by every ecosystem: which specifiers count as pinned, how ranges are advanced, which formats are preserved, which constraints are left alone, how candidates are ordered, and how manifests are written. Rules that apply to a single ecosystem are collected in [Ecosystem Details](ecosystems.md#ecosystem-details).

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

> **Caution**: Go and mise are exceptions. depup updates their exact versions even without `--include-pinned`. To keep a Go dependency as it is, add a `// pinned` comment to its line ([Go](ecosystems.md#go)). To keep a mise tool as it is, pass `--exclude <tool>`; the name is matched in every language, so `--exclude node` also excludes an npm package named `node`.

### Upper and Lower Bounds of Ranges

depup respects upper-bound range constraints (both exclusive and inclusive):

```text
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

```text
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

The following are not processed at all (rather than skipped), so they do not appear in the output:

- Floating selectors, which always point to the newest version: `"*"`, npm dist-tags like `"latest"`, and Gradle dynamic selectors (`"latest.release"`, `"latest.integration"`, `"latest.milestone"`, and any user-defined `latest.<status>`). Rewriting them would turn them into exact versions.
- Multi-segment fully-floating wildcards without a numeric anchor (Composer's `*.*`, `v*`, `V*`, `x.x`) and empty Maven ranges (`[,]`, `(,)`). Accepting them would cause phantom updates or "always outdated" misjudgments.
- Wildcard tokens (`x`/`X`/`*`) followed by numeric segments (`1.x.3`, `^x.0.0`). They are invalid x-ranges in node-semver / semver and would produce malformed output, so they are rejected at parse time.

### Candidate Ordering and Prereleases

Version candidates are compared with ecosystem-specific rules:

| Ecosystem | Ordering |
|-----------|----------|
| Node.js / Rust / Go / Swift | SemVer. A purely numeric suffix such as `1.0.0-1` also counts as a prerelease and sorts before `1.0.0`. Build metadata is ignored, so `1.1.3` and `1.1.3+spec-1.1.0` do not trigger a metadata-only update |
| Python | PEP 440 normalization and ordering |
| Ruby | RubyGems segment ordering; versions containing letters or hyphens are prereleases |
| PHP | composer/semver rules; patch aliases (`-p1`, `-pl1`, `-patch1`) sort after the corresponding release |
| Java / mise | Gradle's documented version ordering |

Numeric components are compared without a fixed integer-size limit, so very large numbers do not overflow.

Prereleases (alpha, beta, rc, canary, dev, and similar) are removed from the candidates while the current version is stable. If the current version is already a prerelease, prerelease candidates are kept so it can move on to the next prerelease or to the stable release. There is no option to offer prereleases to a stable dependency. Versions whose suffix marks them as deprecated are treated the same way, so `serde_yaml 0.9.33` is not moved to `0.9.34-deprecated`.

### Writing Rules

depup only rewrites the dependency declarations it parsed. Other sections of a manifest are left untouched even when they contain package names and versions — for example `overrides` in `package.json`, `replace` / `provide` / `conflict` in `composer.json`, and metadata tables in `Cargo.toml` or `pyproject.toml` ([Ecosystem Details](ecosystems.md#ecosystem-details) lists the exact sections). When a value is rewritten, the surrounding syntax is kept; in TOML manifests, both basic strings (`"..."`) and literal strings (`'...'`) keep their quote style.

If the same dependency key is declared more than once in a manifest, or several dependencies share one Gradle version variable or version-catalog `version.ref`, depup cannot tell which declaration to change. It refuses the write and reports an error (exit code 2) rather than silently changing a declaration that should stay as it is, such as a pinned one.

## Running the Package Manager (`--install`)

With `--install`, depup runs each project's package manager after writing the manifests, so lock files and installed packages follow the new versions.

- An install runs only for manifests that received at least one update, and never with `--dry-run`.
- Without [`.depup`](configuration.md#depup-configuration-file), every install runs in the target directory (the `PATH` argument, or the current directory), even when the updated manifest belongs to a workspace member. With `.depup`, each install runs in the deepest listed directory that contains the updated manifest, so nested apps install in their own directories.
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
| mise | any of the [mise config files](ecosystems.md#files) | `mise install` |

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

With `--verbose`, depup prints a note naming the package managers used in the run for which the age filter covers direct dependencies only. With `--no-age` and no project policy, nothing age-related is passed and the Rust audit does not run.

For Rust, depup checks the release dates of the crates whose version in `Cargo.lock` changed during the install, and rolls back any that violate the age filter to the newest version that satisfies it:

```text
⠙ Auditing hyper [██████████████████████▓░░░░░░░] 18/24 (6s)
  . — 1 transitive dep(s) rolled back to satisfy --age:
    hyper 1.11.1 → 1.11.0
```

Only changed entries are audited because depup limits crates.io requests to one per second, following its crawler policy; auditing an entire lock file (often hundreds of crates) would take several minutes on its own. The audit is capped at 180 seconds per `Cargo.lock`; any crates left over are reported as unchecked. If `Cargo.lock` did not exist before the install, every entry counts as changed, so the cap is more likely to be reached. Rollbacks are always reported, crates that could not be rolled back are listed with `--verbose`, and the audit never changes the exit code.

### uv Malware Check (Preview)

When `--install` triggers `uv sync` for a Python project, depup always sets `UV_MALWARE_CHECK=1` in the environment. This enables [uv's preview malware check](https://astral.sh/blog/uv-audit) — a feature announced alongside `uv audit` but distinct from the `uv audit` command. On every sync operation (`uv add`, `uv sync`, etc.), uv cross-references the currently locked resolution against OSV's MAL advisories and terminates the sync before any malicious package is installed.

- Always on — no opt-in flag required.
- Older uv releases that predate the feature simply do not act on the variable, so enabling it unconditionally does not break existing builds.
- Astral marks the feature as preview, so its exact behavior may change.
- The check runs inside uv: when it finds malware, uv aborts the sync with an error, and depup reports a failed install and exits with code 1.

## When a Dependency Is Not Updated

By default, the text output only counts skipped dependencies. Run with `--verbose` to list each one under its reason:

| Reason | Meaning | See |
|--------|---------|-----|
| `latest` | No version newer than the current one passes the filters. A newer release may exist but be too recent, vulnerable, a prerelease, or above the range's upper bound. | [Filtering Candidates](#filtering-candidates), [Upper and Lower Bounds of Ranges](#upper-and-lower-bounds-of-ranges) |
| `pinned` | The version is pinned, so depup did not query the registry. | [Pinned Versions and `--include-pinned`](#pinned-versions-and---include-pinned) |
| `max-change=<LEVEL>` | Newer versions exist, but all of them exceed the `--max-change` cap. | [Limiting Bumps](#limiting-bumps---max-change) |
| `excluded` / `not in --only` | The package was excluded with `--exclude`, or `--only` lists other packages. | [Options](cli-reference.md#options) |
| `no suitable version` | No published version passes the filters, not even the current one (for example, every release is newer than the age cutoff). | [Filtering Candidates](#filtering-candidates) |
| `parse error: ...` | depup read the constraint but cannot rewrite it safely (for example, `parse error: constraint cannot be updated safely`). Despite the label, the manifest itself parsed correctly, and the exit code is not affected. | [Constraints Left Unchanged](#constraints-left-unchanged) |
| `fetch failed: ...` | The version lookup failed, for example because of a registry error or a failed `git ls-remote`. Whether the exit code changes depends on the cause. | [Exit Codes](cli-reference.md#exit-codes) |

These are the labels in the text output. In `--json` output with `--verbose`, the same reasons appear as `already_latest`, `pinned`, `change_level_limited: <LEVEL>`, `excluded`, `not_in_only_list`, `no_suitable_version`, `parse_error: ...`, and `fetch_failed: ...`.

If a dependency does not appear in the output at all, depup did not process the declaration; check in [Ecosystem Details](ecosystems.md#ecosystem-details) that its file, section, and declaration form are supported. Manifests of languages left out by a language flag such as `--node` are not parsed (project age settings in pnpm, Bun, and mise files are still read). If the manifest was updated but the install failed, see [Running the Package Manager](#running-the-package-manager---install).
