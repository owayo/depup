# Ecosystems and Monorepos

This page is a reference for two questions: which directories depup processes in a monorepo, and which declarations it reads and rewrites in each ecosystem.

## Monorepo Support

To process additional directories, list them in a [`.depup` file](configuration.md#depup-configuration-file). depup detects the following workspaces and project layouts on its own.

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

```text
# Error example (version mismatch)
Found version mismatched Tauri packages:
  tauri (v2.10.1) : @tauri-apps/api (v2.9.1)

# depup automatically synchronizes versions
@tauri-apps/api: 2.9.1 → 2.10.0
tauri: 2.9.0 → 2.10.1
```

Both packages are automatically adjusted to the same major.minor version (e.g., 2.10.x).

## Ecosystem Details

This chapter is a reference. Look up your ecosystem when you need to know exactly which declarations depup reads and how it rewrites them; the shared rules are in [Version Specifiers and Rewriting](usage.md#version-specifiers-and-rewriting).

For `package.json` (including Bun catalogs) and `composer.json`, JSON-escaped section and package names are recognized when updating. Only string values directly inside a dependency section are rewritten; nested objects and arrays are left untouched. The original key spelling, whitespace, and line endings are preserved.

### Node.js

In `package.json`, depup updates `dependencies`, `devDependencies`, `peerDependencies`, and `optionalDependencies`; `overrides` and other sections are left untouched.

depup accepts the node-semver-compatible legacy tilde spelling `~>1.2.3` and preserves `~>` when updating.

For npm partial comparators, `=1.2` and `=1` follow node-semver's partial-version rules instead of being treated as pinned exact versions: in node-semver, `=1.2` means any 1.2.x (`>=1.2.0 <1.3.0`). depup keeps the `=` operator and updates only the visible segment shape (`=1.2` → `=2.3`, `=1` → `=2`).

For npm comparator sets, depup supports bare partial lower bounds such as the `1.2` in `1.2 <2.0.0` and preserves the partial shape when updating the lower side (`1.2 <2.0.0` → `1.9 <2.0.0`).

node-semver's `HYPHENRANGE` accepts `XRANGEPLAIN` on both sides, so x-range endpoints such as `1.x - 2.x` are valid and updatable. Endpoints that depup rejects elsewhere (a digit after a wildcard like `1.x.3`, or a fully floating `*`) stay rejected.

For npm semver tokens, depup validates prerelease and build metadata identifiers before parsing them as updatable constraints. Identifiers with underscores (`1.2.3-rc_1`), empty identifier segments (`1.2.3-alpha..1`), and numeric prerelease identifiers with leading zeroes (`1.2.3-01`) are not processed, rather than being normalized into malformed `package.json` constraints.

Build metadata is stripped before the leading-zero prerelease check. SemVer allows build identifiers to contain hyphens and leading zeroes, so versions such as `1.0.0+2024-01` and `1.2.3+00` remain valid and updatable — only the prerelease segment is validated.

For Bun workspaces, depup parses and updates root `package.json` catalog definitions in both top-level `catalog` / `catalogs` and `workspaces.catalog` / `workspaces.catalogs`. Workspace package references such as `"react": "catalog:"` and `"jest": "catalog:testing"` are kept as catalog references; depup updates the shared catalog entry instead. pnpm catalogs defined in `pnpm-workspace.yaml` are not yet parsed as manifests, so `package.json` `catalog:` references backed by pnpm catalogs are not processed.

### Python

Beyond PEP 621, PEP 735, and Poetry, depup also reads uv's legacy `[tool.uv] dev-dependencies` and PDM's `[tool.pdm.dev-dependencies]`. Dependencies routed through `[tool.uv.sources]` to a non-PyPI source (`workspace = true`, `git`, `path`, `url`, or an `index` other than `pypi`) are not processed, so a workspace member or custom index is never overwritten with a same-named PyPI package. Poetry multiline dependency tables (`[tool.poetry.dependencies.<name>]`, `[tool.poetry.group.<g>.dependencies.<name>]`) and TOML quoted keys (`"zope.interface"`, `"ruamel.yaml"`) are parsed and updated too (names containing a dot must be quoted in TOML).

In `[project]`, `[tool.rye]`, and `[tool.uv]` sections, depup rewrites only the `dependencies` / `dev-dependencies` arrays; metadata strings such as `name`, `description`, and `keywords` are never modified even if they look like PEP 508 specifiers. Python PEP 508 version lists may include a trailing comma, such as `>=3.5,<4,`; depup parses and preserves that comma when updating the lower bound. Poetry dependencies with a non-`pypi` `source` are not processed, including PEP 621 dependencies enriched by `tool.poetry.dependencies`, because depup only queries PyPI. Poetry's multiple-constraints array form (`foo = [{version = "<=1.9", python = ">=3.6,<3.8"}, {version = "^2.0", python = ">=3.8"}]`) is not processed either, because depup cannot safely rewrite an individual array element without per-element `requires_python` resolution.

A `pyproject.toml` that configures a non-PyPI default index — a Poetry `priority = "primary"` / `"default"` source, a uv `[[tool.uv.index]] default = true` or `[tool.uv] index-url`, or a PDM source overriding `pypi` — none of its dependencies are processed, and depup prints a warning once. depup only queries PyPI, so updating those dependencies would replace private packages with same-named public ones.

Python compatible release clauses follow PEP 440: `~=1.2` (= `>=1.2,<2.0`) and `~=1.2.3` (= `>=1.2.3,<1.3.0`) are treated as ranges with an upper bound, so updates stay within the compatible range (`~=1.2.3` stays in the 1.2 series, `~=1.2` stays in the 1.x series) and preserve the original segment count (`~=1.2` → `~=1.9`; writing `~=1.9.0` would narrow the upper bound to `<1.10.0`). The invalid single-segment form `~=1` is not processed.

PEP 440 prefix matching is accepted only for release-segment `==` / `!=` specifiers such as `==1.2.*` and `!=1.2.*`. Invalid prefix forms such as `>=1.0.*`, `~=1.0.*`, `==1.0a1.*`, `==1.0.post1.*`, and `==1.0+local.*` are rejected at parse time and not processed. Arbitrary equality (`===1.0.*`) remains an exact pinned specifier, not prefix matching.

In Poetry's `[tool.poetry.dependencies]`, a bare version string without an operator (`requests = "2.28.0"`) is an exact pin — Poetry's official "Exact requirements", equivalent to `==2.28.0`. depup parses it as an exact/pinned dependency for both the simple form and inline tables (`{ version = "1.26.0", optional = true }`), so it is handled just like an explicit `==2.28.0` (skipped as `pinned`, updatable with `--include-pinned`) and is rewritten without adding an operator (`4.2.1` → `5.0.0`). This bare-exact interpretation applies only in the Poetry context; pip / PEP 508 requirements still require an operator, so a bare version there is not accepted.

PEP 440 local versions (the `+` label) are handled as Python-specific version semantics rather than semver build metadata. Exact and exclusion specifiers keep their local labels when parsed but are never rewritten (`torch==2.1.0+cu121`, `!=1.0+local1`); when a newer version exists, they are skipped with `parse error: constraint cannot be updated safely`. A local label names a separate build made from the same public version (`cu121` targets CUDA 12.1), so advancing only the public version would point at a build that does not exist. Python candidate ordering treats local versions as newer than the same public version (`1.0+local > 1.0`) and compares local segments per PEP 440 (`1.0+1 > 1.0+abc`, `1.0+abc.2 > 1.0+abc.1`). Ordered and compatible specifiers with local labels, which PEP 440 does not permit (`>=1.0+local`, `~=1.0+local`, `>=1.0+local,<2.0`), are not processed.

PEP 440 prerelease versions are detected and removed from the candidates by default even when written without a separator (e.g., `2.0.0rc1`, `1.0rc1`, `1.0.0a1`), so a stable dependency is never accidentally bumped to a release candidate. Post-releases (`1.0.post1`) compare as newer than the corresponding release, and epochs (`1!2.3`) take precedence in comparison. A post-release attached to a prerelease (`1.0a1.post1`) is also recognized as newer than the underlying prerelease (`1.0a1 < 1.0a1.post1 < 1.0`), so users tracking an alpha can still pick up its post-release fixes.

### Rust

In `Cargo.toml`, dependency updates are limited to dependency tables such as `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, `[workspace.dependencies]`, and target-specific dependency tables; metadata tables are left untouched.

Cargo renamed dependencies such as `alias = { package = "actual-crate", version = "1" }` are fetched by the real package name and written back through the manifest key. `--only` and `--exclude` accept either name.

Path dependencies (`{ path = "../common" }`) are not processed, even when they also declare a `version` for publishing, because they resolve to the local crate. Dependencies that point at a registry other than crates.io — a non-`crates-io` `registry = "..."` or any `registry-index = "..."` — are not processed either, because depup only queries crates.io.

Git dependencies are checked with `git ls-remote` instead of a registry:

| Reference | Behavior |
|-----------|----------|
| `tag` | Updated to the newest stable semver tag. If that tag exceeds the `--max-change` cap, the dependency is skipped with `max-change=<LEVEL>`; older tags within the cap are not considered |
| `branch`, or none (default branch) | Reported as updated when the remote head differs from the commit recorded in `Cargo.lock`, or when none is recorded. `Cargo.toml` stays as written; the new commit is picked up only when `--install` runs `cargo update`, and without `--install` nothing is written |
| `rev` | Always skipped as `pinned`, even with `--include-pinned` |

Tag updates are limited to the same dependency tables as version updates and to `[patch.<registry>]` / `[patch.<registry>.<package>]`, and they preserve single or double quotes in both inline and multiline tables. Git dependencies are not subject to the age filter or the OSV check, and a failed `git ls-remote` skips the dependency without changing the exit code.

Cargo comparison ranges may contain more than two comma-separated requirements, for example `>=1.0, <2.0, >=1.0.100`. Mixed multi-requirement constraints that combine caret/tilde/wildcard with comparators, such as `^1.2.2, <1.5`, are validated with `semver::VersionReq` and detected as ranges. Mixed constraints without an upper bound, such as `>=1.2.3, ^1.3`, cannot be rewritten safely and are skipped.

### Go

`go.mod` has no range syntax such as `^` or `~` and only holds exact versions, so depup does not read an exact Go version as an intentional pin. Go dependencies without a `// pinned` comment are updated regardless of `--include-pinned`. To keep one, add `// pinned` to the end of its line; it is then skipped as `pinned` unless `--include-pinned` is given. The word is recognized anywhere in the comment, so `// indirect; pinned` works too.

Versions listed in Go `exclude` directives are removed from the candidates; the directives themselves are not rewritten. Versions retracted by the upstream module's latest `go.mod` are also removed, including closed retract ranges. For modules without tagged versions, depup falls back to the Go Proxy `@latest` endpoint; an omitted `.info` `Time` uses the Unix epoch so an unknown release date is not permanently filtered by `--age`.

Go versions tagged `+incompatible` are handled with the same rule the `go` command applies: once a `+incompatible` version is reached in semver order, it and everything above it are removed from the candidates if the preceding compatible version has a real `go.mod` (not one synthesized by the Go proxy). Without this, a module like `github.com/libp2p/go-libp2p` would be "updated" from `v0.49.0` to a 2018-era `v6.0.23+incompatible`, and because that version still builds, the mistake would go unnoticed.

Modules replaced by a `replace` directive are not processed, because updating the `require` alone would stop the `replace` from matching and silently drop the local patch. A `replace` without a version covers every `require` of that module, and a versioned `replace` covers only the `require` with the same version.

For `go.mod`, depup treats block endings with trailing comments such as `) // direct deps` as normal block endings when parsing and updating `require`, `replace`, and `exclude` blocks.

Quoted `go.mod` module paths and versions, such as `require "golang.org/x/text" "v0.14.0"`, are parsed and updated while preserving the quotes.

`go.mod` updates preserve the original LF or CRLF line endings in both single-line and block `require` declarations.

### Ruby

Gemfile declarations can use either the common Ruby DSL form (`gem "rack", "~> 3.0"`) or parenthesized method-call form (`gem("rack", "~> 3.0")`). Both forms are parsed and updated while preserving the original call style. When the same gem is declared in multiple places (for example both at the top level and inside a `group :test` block), depup refuses the ambiguous write.

Gemfile compound constraints such as `gem "pg", ">= 0.18", "< 2.0"` are parsed and updated. depup advances only the inclusive lower bound and writes it back across the original arguments, preserving their count, order, quote style, spacing, parenthesized call form, and trailing conditional modifiers. The comparison baseline is the inclusive lower bound regardless of the order the constraints are written in, so `gem "pg", "< 2.0", ">= 0.18"` is compared against `0.18`. If the rewritten constraint cannot be split back into the original number of arguments (for example when one argument itself contains a comma), depup reports an error instead of applying an unsafe edit. Exclusion constraints such as `gem "rack", "!= 2.2.4"` are skipped, because replacing part of them can change their meaning.

The following Gemfile declarations are not processed:

- Gems that point to a non-registry source without a version (`git:`, `github:`, `bitbucket:`, `gist:`, `path:`, `source:`), because they are not RubyGems registry dependencies. The hash-rocket spelling (`:git => '...'`) is recognized as well.
- Gems declared inside `git ... do` / `github ... do` / `path ... do` / `source ... do` blocks, for the same reason.
- Declarations whose arguments continue on the next line (`gem "devise",`), because that line alone cannot determine the version.

If a gem with `git:` or a similar option explicitly includes a version, depup treats it as Bundler's gemspec constraint and parses and updates it while preserving the source option. Gems inside ordinary blocks such as `platforms` and `install_if` are processed like any other gem. Inline `group:` / `groups:` options are used to classify development dependencies.

Custom Bundler git source shorthands registered with `git_source(:name) { ... }` (for example `gem 'rails', stash: 'forks/rails'`) follow the same rules as the built-in `git:` / `github:` shorthands: a declaration without a version is a non-registry dependency and is not processed.

### PHP

In `composer.json`, depup updates `require` and `require-dev`; sections such as `replace`, `provide`, and `conflict` are left untouched. Composer accepts explicit equality (`=1.2.3`, `==1.2.3`), and depup preserves the operator when updating. Constraints using the `<>` exclusion spelling (`<>1.2.3`) are parsed but skipped rather than rewritten ([Constraints Left Unchanged](usage.md#constraints-left-unchanged)).

Composer platform packages such as `php`, `hhvm`, `ext-*`, `lib-*`, and Composer API packages are not processed. Inline aliases such as `1.0.0 as 1.1.0` are not processed either, because overwriting them with the registry's latest version would break the alias declaration.

Composer/Packagist accepts 1-4 segment numeric versions per `composer/semver`'s `VersionParser`, so depup parses and updates four-segment versions like `1.2.3.4`, `^1.0.0.0`, `~3.4.5.6`, and `1.0.0.*`, while forms with five or more segments are invalid and not processed.

Composer modifiers may omit the separator or use `.` / `_` (`composer/semver` allows `[._-]?`), so depup treats `5.0.0alpha3`, `1.0.0.RC1`, and `1.0.0_beta1` as prereleases and `2.2.1p1` / `2.2.1pl1` / `2.2.1patch1` as patch aliases that sort above the base version. Both forms occur on Packagist today (`nikic/php-parser` publishes `5.0.0beta1`; `laminas/laminas-diactoros` ships security patches as `2.2.1p2`).

Composer rejects the `~>` operator (`Invalid operator "~>"`), so for PHP a `~>` constraint is not processed, rather than being rewritten into a constraint Composer cannot read. Node accepts `~>` because node-semver defines it as valid.

### Java (Gradle)

Gradle rich version declarations using `strictly`, `require`, `prefer`, and `reject` are parsed in dependency blocks such as `implementation("org.slf4j:slf4j-api") { version { ... } }`. String notation shorthand supports exact, dynamic-prefix, and range constraints, including `group:name:1.2.3!!`, `group:name:5.3.+!!`, `group:name:[1.7, 1.8[!!`, and a strict range with a preferred version such as `group:name:[1.7, 1.8[!!1.7.25`. When `strictly` or `require` declares a range and `prefer` declares the selected version, depup keeps the range as the upper-bound constraint and updates the `prefer` value. Versions listed with `reject` are removed from the candidates, including dynamic rejects such as `2.+` and ranges such as `[1.5,1.9)`.

Gradle declaration wrappers are supported: `platform(...)`, `enforcedPlatform(...)`, and `testFixtures(...)`. BOM declarations such as `implementation platform('com.google.cloud:libraries-bom:26.1.0')` and `testImplementation(platform("org.junit:junit-bom:5.10.0"))` are parsed and updated, and the surrounding configuration name is still used for the dev/production classification.

Gradle variables declared with `ext.<name> = '...'` / `project.ext.<name> = "..."` are resolved alongside `ext { ... }` blocks, and qualified references such as `${Versions.retrofit}` resolve by their final segment. A short name defined more than once with different values is not resolved, so dependencies that reference it are not processed; this avoids picking up the wrong object's value.

Gradle version catalogs under `gradle/*.versions.toml` are detected as Java manifests. depup parses `[libraries]` entries written as `alias = "group:name:version"`, `module = "group:name"`, `group` / `name` / `version`, and `version.ref`; referenced `[versions]` entries are updated in place. Rich version tables with `strictly`, `require`, `prefer`, `reject`, and `rejectAll` follow the same candidate rules as Gradle build files. `[plugins]` entries are not processed because Gradle plugin IDs are not Maven Central coordinates.

For Gradle string notation, depup preserves classifier and extension suffixes such as `:resources@zip` or `@aar`, and ignores declarations that appear only in `//` line comments or `/* ... */` block comments. Gradle version catalog updates preserve the original TOML string or table shape where the version is declared.

Dependencies whose version is a `-SNAPSHOT` / `.SNAPSHOT` version (`1.2.3-SNAPSHOT`, `1.2.3-SNAPSHOT!!`, `[1.2.3-SNAPSHOT]`) are not processed. A snapshot is a moving reference that resolves to the newest timestamped build on every resolution, so rewriting it to a fixed release would silently change what the build uses. Stable qualifiers such as `.Final`, `.RELEASE`, `-jre`, and `-SP1` are still updated.

Gradle coordinates declared inside `resolutionStrategy { force ... }`, `constraints { }`, and `dependencySubstitution { }` are not processed. They restate a version that is declared elsewhere, and treating them as separate declarations would make the coordinate ambiguous and block the update.

JVM milestone releases are treated as prereleases and are removed from the candidates while the current version is stable: `4.0.0-M1`, the legacy Spring Boot dot form `2.0.0.M1`, and the spelled-out `-milestone1`. Without this, a project on `assertj-core 3.24.2` would be bumped to `4.0.0-M1`, `junit-bom 5.10.0` to `5.13.0-M3`, and `spring-core 5.3.23` to `7.0.0-M6`. Detection is limited to tokens where `m` is immediately followed by digits, so stable JVM qualifiers (`.Final`, `-jre`, `-android`, `.RELEASE`, `.GA`, `-SP1`) and identifiers like `-macos1` are never misclassified. As with other prereleases, a project already on a milestone keeps milestone candidates so it can advance to the next one.

### Swift

Only dependencies on GitHub URLs with a version requirement are processed. Non-GitHub URLs and `branch:` / `revision:` requirements are not processed, and neither are Swift Package Registry `id:` dependencies (`.package(id: "scope.name", ...)`), because the registry API adapter is not implemented yet (support is planned).

- URLs: HTTPS, scp-style SSH (`git@github.com:owner/repo.git`), standard SSH (`ssh://git@github.com/owner/repo.git`), and GitHub's SSH over port 443 (`ssh://git@ssh.github.com:443/owner/repo.git`) are accepted.
- Tags: both `v1.2.3` and `V1.2.3` are recognized. `Package.swift` version requirement strings are validated as strict SemVer (`X.Y.Z`, no leading zeroes).
- Because SwiftPM follows Semantic Versioning 2.0.0, depup parses and updates versions with prerelease identifiers (`1.0.0-beta.1`), build metadata (`1.0.0+build.123`), or both (`1.0.0-rc.1+sha.abc`).
- `.package(...)` declarations with trailing arguments such as `traits: [...]` (SPM 6.1) or `moduleAliases: [...]` are parsed; only the version requirement is replaced, and the other arguments are preserved.
- Dependencies that appear inside `//` line comments or `/* ... */` block comments are ignored.

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

When `[settings] minimum_release_age` is **explicitly set**, depup treats it as a [project policy](usage.md#resolution-priority), just like pnpm's and Bun's `minimumReleaseAge`, so it takes precedence over the CLI `--age`. Only a value written in a config file counts; mise's built-in 24-hour default is ignored. The syntax is shown under [Supported `minimumReleaseAge` Sources](usage.md#supported-minimumreleaseage-sources).

> **Caution**: mise's `m` means **minutes** (humantime convention), unlike depup's `--age 1m`, which means one month. `minimum_release_age = "1m"` is read as one minute, and because a project policy overrides the CLI `--age`, it effectively disables the age filter unless pnpm or Bun sets a larger value. Write `"1M"` or `"30d"` for one month.

`mise ls-remote` applies mise's own `minimum_release_age` by default and hides newer releases, so depup passes `--minimum-release-age 0` to fetch everything and keeps age evaluation in one place. `minimum_release_age_excludes` is not interpreted by depup; if it is set, depup prints a warning.
