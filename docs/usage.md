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

A minimum release age declared in the project (pnpm's or Bun's `minimumReleaseAge`, or mise's `minimum_release_age`) is treated as the **project policy** and takes precedence over the CLI `--age` and the global configuration file. The age for each manifest is resolved in this order (highest first):

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

If more than one of pnpm, Bun, and mise sets a value, depup uses the stricter (larger) one. Within pnpm, only the first value found in each directory is used; a pnpm lockfile is not required.

Each manifest inherits explicit settings from its directory up to the run root and uses the strictest inherited value. Settings in sibling projects do not affect its candidate selection. When started inside a Cargo, pnpm, or uv workspace, depup also reads the workspace root's policy. Tauri synchronization is confined to each app and respects the frontend and Rust scopes separately.

For Python and mise, depup also preserves stricter native settings: uv's `exclude-newer` in `uv.toml`, `[tool.uv]`, the user configuration, or `UV_EXCLUDE_NEWER`, including `UV_CONFIG_FILE` and system settings; and mise's global, local, and per-tool `minimum_release_age` or `MISE_MINIMUM_RELEASE_AGE`, including `MISE_CONFIG_FILE` and active `MISE_ENV` overlays. Native settings inherited above the run root are preserved too. These values can tighten the resolved depup policy even with `--no-age`. Per-tool mise ages and stricter uv package/index cutoffs are combined into the strictest age for that scope. Such native constraints disable publisher exemptions in that scope. Dates, timestamps, single-unit durations, and fixed ISO 8601 durations are supported; a native setting that cannot be read or interpreted stops that manifest's update instead of being replaced with a weaker value.

The cutoff is fixed when the run starts. Candidate selection, Tauri synchronization, and the Cargo audit use that same instant. uv and mise receive absolute cutoffs during install; pnpm receives minutes rounded up so seconds are never discarded.

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
- Node installs run at the shared pnpm workspace root or the manifest directory. Other languages use the target directory (PATH) without [`.depup`](configuration.md#depup-configuration-file), or the deepest listed directory containing the updated manifest when `.depup` is present.
- Installs run one at a time, in directory path order, and each language runs at most once per directory. The package manager's output is captured instead of streamed; its stderr is printed only when the install fails.

If an install fails, depup still runs the remaining installs, prints the failed command with the package manager's stderr, and exits with code 1 at the end (`Error: Some package manager installs failed`). A package manager that is not installed counts as a failure. Manifests that were already rewritten are not rolled back, and the Rust and Node lockfile age audits still run even when another install fails. An unchanged `--install` run also audits existing locks; it does not rerun package managers when there are no manifest updates.

### Commands per Package Manager

The package manager is detected from files in the directory where the install runs. Node members of a detected pnpm workspace share its root install and lock audit; other package managers do not search parent directories. Within each language, the first match wins:

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

The age filter decides which versions depup writes into manifests. Whether it also reaches transitive dependencies during `--install` depends on the package manager. An install that resolves multiple manifests uses the strictest policy of those manifests, including members with no updates ([Resolution Priority](#resolution-priority)):

| Package manager | What depup passes | Transitive dependencies |
|-----------------|-------------------|-------------------------|
| pnpm | Both `npm_config_minimum_release_age=<minutes>` and `pnpm_config_minimum_release_age=<minutes>` | Native filtering on supported pnpm versions, plus a post-install lock audit. The two prefixes cover pnpm 10 and pnpm 11 or later |
| uv | `--exclude-newer <timestamp>` | Filtered when uv resolves them |
| Cargo | Nothing; depup audits `Cargo.lock` after `cargo update` | crates.io crates that violate the age filter are rolled back ([below](#auditing-cargolock-rust)) |
| mise | `--minimum-release-age <timestamp>` and `MISE_MINIMUM_RELEASE_AGE` | Fuzzy top-level versions are filtered when timestamps are available. Only the `npm:` and `pypi:` backends pass the cutoff to unpinned transitive dependencies; exact pins and locked top-level versions bypass native filtering |
| npm | Nothing | Resolved versions are verified after install for package-lock v2/v3 |
| Yarn, Bun | Nothing | Lock audit is currently unsupported; an active age policy produces exit 2 |
| pip, Poetry, Rye, Pipenv, Go, Bundler, Composer, Gradle, SwiftPM | Nothing | Not filtered; only direct dependencies follow the age filter |

With `--verbose`, depup prints a note naming the package managers used in the run for which the age filter covers direct dependencies only. With `--no-age` and no project or native policy, nothing age-related is passed and neither the Rust nor Node lock audit runs. Native package/index exemptions configured in uv still follow uv's own semantics.

#### Auditing Node Lockfiles

With an active age policy, `--install` checks the actual resolved versions of direct and transitive dependencies, including dev dependencies. It also checks unchanged locks on subsequent runs and after another install fails. If no lock exists and no install is needed, no lock audit runs.

- pnpm: reads the lock graph using `pnpm list --depth Infinity --lockfile-only --json`. Shared workspaces include all projects and the root, with the strictest member policy. Missing required dependencies or CLI warnings make the graph unverified.
- npm: reads the `packages` table of `npm-shrinkwrap.json` (preferred) or `package-lock.json`, format 2 or 3, and verifies required dependencies across packages and workspaces.
- Yarn, Bun, and npm format 1: currently reported as unsupported, with exit code 2 when age is active.

Only packages whose registry source can be confirmed as public npm are queried. For pnpm, generated graph URLs alone are not source evidence; the effective registry configuration is checked too. Aliases use the real package name. Local, workspace, and Git dependencies are outside this release-date audit. Private or unknown registry sources are reported as unverified without querying public npm for their names. Missing publication dates, read or lookup failures, changed lock contents during the audit, and the 180-second time limit also produce exit code 2. Publication dates are checked independently of the update-candidate filters, so deprecated or non-latest locked versions can be verified.

Node auditing does not rewrite the lockfile. Violations always show the actual locked version, publication date, and cutoff; unverified entries always show their reason. Native age filtering helps prevent new resolutions, but does not replace this final verification.

#### Auditing `Cargo.lock` (Rust)

For Rust, depup checks all crates.io versions in the final `Cargo.lock`, including unchanged direct and transitive dependencies. It first rolls back newly resolved violations where the original manifest constraints and the pre-install minimum permit it, then performs a separate read-only verification of every final locked version:

```text
⠙ Auditing hyper [██████████████████████▓░░░░░░░] 18/24 (6s)
  . — 1 crate(s) rolled back to satisfy --age:
    hyper 1.11.1 → 1.11.0
```

depup limits crates.io API requests to one per second and reuses metadata fetched during the run. Rollback and final verification each have a separate 180-second budget per `Cargo.lock`. Final verification reads immutable publication dates from the [official sparse index](https://doc.rust-lang.org/cargo/reference/registry-index.html), with at most eight concurrent requests; missing dates or young versions needing publisher verification use the rate-limited API. This metadata includes yanked locked versions without allowing them as update or rollback candidates. Unchecked entries are reported and produce exit code 2, including on a no-change retry. An unreadable existing lock, or a missing lock after an install was needed, cannot pass. A lockless project with no updates does not run an install or lock audit. An unreadable pre-install lock also disables automatic rollback because its minimum cannot be established safely. Crates from registries other than crates.io are not audited and keep their locked versions, because their release dates cannot be looked up on crates.io.

A crate is rolled back to the following version:

- For a direct dependency that depup updated in this run, the version shown in its update line is preferred. That version has already passed the age filter, the OSV check, and `--max-change`.
- For any other crate, the newest stable (non-prerelease) version from the same semver series that satisfies the age filter and is older than the locked one.

In both cases, a rollback never goes below an available pre-install version from the same semver series. Pre-install versions absent from usable crates.io metadata, including yanked versions and versions with unreadable release dates, are excluded from both this minimum and restoration. In that case, depup chooses an older compatible version that satisfies the age filter, or reports an unresolved violation if none exists.

If only versions below an available minimum satisfy the age filter, the crate goes back to that pre-install version, so depup undoes only the change the install made. When that pre-install version is itself newer than the age filter allows, it remains an unresolved violation (exit code 2) and is reported on a separate yellow line instead of as rolled back. An unchanged violation at this minimum is also reported; it is never exempted merely because it was already locked:

```text
  . — 1 crate(s) returned to the version locked before the install, which is also newer than --age:
    foo 1.50.1 → 1.50.0
```

Crates such as `cc` and `syn` are rolled back one at a time with `cargo update -p <name>@<locked version> --precise <older version>`. This fails for groups of crates that pin each other with `=`, such as `wasm-bindgen`, `js-sys`, `web-sys`, `wasm-bindgen-futures`, and `wasm-bindgen-test`. `--precise` keeps the crates that depend on the target at their locked versions, and their `=` requirements do not allow an older target. When two or more crates in the group have no dependent within it, every one-at-a-time rollback conflicts. Instead, depup rolls back the conflicting crates together:

1. It copies what Cargo needs to resolve the workspace into a temporary directory: the root and member `Cargo.toml` files, empty placeholders for their target source files, and `Cargo.lock`.
2. It adds temporary pins to the copy and runs `cargo update` for those crates at once. Crates that depend on each other are resolved as one group, largest group first, so one crate that cannot be rolled back does not hold up the others. Within a group, depup first pins every crate to its rollback version; if Cargo cannot satisfy that, it pins only the crates that nothing else in the group depends on, and then allows any version up to the rollback version.
3. It removes the pins, tidies the lock file, and replaces the project's `Cargo.lock` with it.
4. It runs `cargo update --workspace --locked` in the project to confirm that your `Cargo.toml` accepts the new lock file as is. If it does not, the previous `Cargo.lock` is restored.

Your `Cargo.toml` files are never modified. If `Cargo.toml` or `Cargo.lock` changes while this is in progress, depup keeps your change and gives up on these crates. If the previous `Cargo.lock` cannot be restored after a failed check, depup says so even without `--verbose` and tells you where it saved the previous content. Cargo runs in the project directory, so the project's `.cargo/config.toml` (for example, source replacement) and `rust-toolchain.toml` (or `rust-toolchain`) also apply when the copy is resolved.

Crates rolled back together are reported with the others:

```text
  . — 10 crate(s) rolled back to satisfy --age:
    js-sys 0.3.106 → 0.3.105
    wasm-bindgen 0.2.129 → 0.2.128
    ...
```

A crate that drops out of `Cargo.lock` during the joint resolution is listed as `tokio 1.53.1 → removed` and counted as rolled back. Any rollback, one at a time or joint, can also bring other new versions into `Cargo.lock`; depup audits those as well and rolls them back in turn, up to a fixed number of rounds.

A shared workspace lock uses the strictest member policy and only publisher exemptions common to every member. If a locked version is absent from cached registry metadata, depup refreshes that metadata once. Missing metadata, unreadable or invalid locks, failed rollbacks, and an unfinished audit cause exit code `2` (an install failure still takes priority as `1`). Restoring the pre-install version is still unresolved if it does not satisfy the age policy.

A rollback counts only when `Cargo.lock` actually changed; depup rereads the lock file after each `cargo update` instead of trusting its exit status, and builds the final report from the lock file as it stands when the audit ends. Rollbacks are always reported. Crates that could not be rolled back, crates whose release date is unavailable, and crates left unchecked when the time cap is reached always show names, requested rollback targets when attempted, and reasons, even without `--verbose`:

```text
  . — 2 crate(s) could not be rolled back to satisfy --age:
    foo (2.1.3; requested: 2.1.2): cargo update failed: <cargo error>
    bar (1.0.2): no older version satisfies --age
  . — 1 crate(s) could not be checked against --age:
    example (1.0.0): release date unavailable
  . — age audit stopped after 180s; 3 crate(s) left unchecked
```

With `--verbose`, depup prints `Enforcing --age on registry crates in Cargo.lock...` when the audit starts. Failure reasons are also shown in normal output:

```text
  . — 2 crate(s) could not be rolled back to satisfy --age:
    foo (2.1.3): Cargo.toml requires `^2.1.3`, which excludes every version that satisfies --age
    bar (1.0.2): cargo update failed: <error from cargo>
```

| Reason | Meaning |
|--------|---------|
| `Cargo.toml requires ...` | The version requirement excludes every version old enough to satisfy the age filter. |
| `no older version satisfies --age` | No version older than the locked one is old enough. |
| `cargo update failed: ...` | Cargo rejected the rollback; its error message follows. |
| `not attempted: ...` | depup did not try the rollback, because it reached its limit on rounds or its time budget. |
| `release date unavailable` | The release date could not be retrieved, so the crate could not be checked. |

An unresolved audit exits with code `2`.

After `--install`, depup also compares each Rust update it reported with the version that ended up in `Cargo.lock`. When they differ, it prints a yellow note, even without `--verbose` and even when the age filter is off:

```text
  ./Cargo.toml — 1 update(s) locked at a different version than shown:
    wasm-bindgen 0.2.127 → 0.2.128 (locked: 0.2.129)
```

This happens, for example, when `--max-change` or the OSV check made depup choose an older version, but `cargo update` locked a newer one that still satisfies the `^` requirement.

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
