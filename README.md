<p align="center">
  <img src="docs/images/app.png" width="128" alt="depup">
</p>

<h1 align="center">depup</h1>

<p align="center">
  Multi-language dependency updater CLI tool
</p>

<p align="center">
  <a href="https://github.com/owayo/depup/actions/workflows/ci.yml"><img src="https://github.com/owayo/depup/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/depup/releases"><img src="https://img.shields.io/github/v/release/owayo/depup" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

<h3 align="center">Supported Languages</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Node.js-339933?logo=nodedotjs&logoColor=white" alt="Node.js">
  <img src="https://img.shields.io/badge/Python-3776AB?logo=python&logoColor=white" alt="Python">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/Go-00ADD8?logo=go&logoColor=white" alt="Go">
  <img src="https://img.shields.io/badge/Ruby-CC342D?logo=ruby&logoColor=white" alt="Ruby">
  <img src="https://img.shields.io/badge/PHP-777BB4?logo=php&logoColor=white" alt="PHP">
  <img src="https://img.shields.io/badge/Java-ED8B00?logo=openjdk&logoColor=white" alt="Java">
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

- **Multi-Language Support**: Node.js, Python, Rust, Go, Ruby, PHP, Java
- **Manifest Updates**: Directly updates version specifications in manifest files
- **Smart Version Handling**: Preserves version range formats (^, ~, >=)
- **Pinned Version Detection**: Skips intentionally pinned versions by default
- **Age Filter**: Only update to versions released N days/weeks ago
- **pnpm Integration**: Respects `minimumReleaseAge` from pnpm settings
- **Monorepo Support**: pnpm workspaces and Tauri projects
- **Release Date Display**: Shows when each new version was released
- **Multiple Output Formats**: Text (colored), JSON, diff

## Supported Languages

| Language | Manifest | Registry | Lock Files |
|----------|----------|----------|------------|
| <img src="https://img.shields.io/badge/-339933?logo=nodedotjs&logoColor=white" height="16"> Node.js | package.json | npm | package-lock.json, pnpm-lock.yaml, yarn.lock |
| <img src="https://img.shields.io/badge/-3776AB?logo=python&logoColor=white" height="16"> Python | pyproject.toml | PyPI | uv.lock, rye.lock, poetry.lock |
| <img src="https://img.shields.io/badge/-000000?logo=rust&logoColor=white" height="16"> Rust | Cargo.toml | crates.io | Cargo.lock |
| <img src="https://img.shields.io/badge/-00ADD8?logo=go&logoColor=white" height="16"> Go | go.mod | Go Proxy | go.sum |
| <img src="https://img.shields.io/badge/-CC342D?logo=ruby&logoColor=white" height="16"> Ruby | Gemfile | RubyGems | Gemfile.lock |
| <img src="https://img.shields.io/badge/-777BB4?logo=php&logoColor=white" height="16"> PHP | composer.json | Packagist | composer.lock |
| <img src="https://img.shields.io/badge/-ED8B00?logo=openjdk&logoColor=white" height="16"> Java | build.gradle, build.gradle.kts | Maven Central | gradle.lockfile |

## Requirements

- **OS**: macOS, Linux, Windows
- **Rust**: 1.70+ (for building from source)

## Installation

### From Source

```bash
git clone https://github.com/owayo/depup.git
cd depup
cargo install --path .
```

### From GitHub Releases

Download the latest binary from [GitHub Releases](https://github.com/owayo/depup/releases).

## Quickstart

```bash
# Update all dependencies (dry run)
depup -n

# Update Node.js dependencies only
depup --node

# Update with age filter (2 weeks minimum)
depup --age 2w

# Update and show diff
depup --diff
```

## Usage

### Basic Syntax

```bash
depup [OPTIONS] [PATH]
```

### Options

| Option | Short | Description |
|--------|-------|-------------|
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
| `--exclude <PKG>` | | Exclude specific packages (repeatable) |
| `--only <PKG>` | | Update only specific packages (repeatable) |
| `--include-pinned` | | Include pinned versions in update |
| `--age <DURATION>` | | Minimum release age (e.g., 2w, 10d, 1m) |
| `--json` | | Output results in JSON format |
| `--diff` | | Show changes in diff format |
| `--install` | | Run package manager install after update |
| `--version` | `-v` | Show version |
| `--help` | `-h` | Show help |

### Examples

```bash
# Preview all updates
depup -n

# Update only lodash and typescript
depup --only lodash --only typescript

# Exclude react from updates
depup --exclude react

# Update packages at least 2 weeks old
depup --age 2w

# Update Python and Rust only
depup --python --rust

# Update Java (Gradle) dependencies
depup --java

# JSON output for CI/CD
depup --json

# Update and run npm install
depup --node --install
```

## Version Handling

### Pinned Versions (Excluded by Default)

Pinned versions are intentionally fixed and excluded from updates by default:

| Language | Pinned Example | Updated |
|----------|----------------|---------|
| Node.js | `"1.2.3"` | ❌ |
| Node.js | `"^1.2.3"`, `"~1.2.3"` | ✅ |
| Python | `"==1.2.3"` | ❌ |
| Python | `">=1.2.3"`, `"^1.2.3"` | ✅ |
| Rust | `"=1.2.3"` | ❌ |
| Rust | `"1.2.3"`, `"^1.2.3"` | ✅ |
| Go | `// pinned` comment | ❌ |
| Ruby | `'= 1.2.3'` | ❌ |
| Ruby | `'~> 1.2.3'`, `'>= 1.2.3'` | ✅ |
| PHP | `"1.2.3"` | ❌ |
| PHP | `"^1.2.3"`, `"~1.2.3"` | ✅ |
| Java | Fixed version in Gradle | ✅ |

Use `--include-pinned` to update pinned versions.

### Range Preservation

depup preserves the original version range format:

```
"^1.2.3" → "^2.0.0"  (caret preserved)
"~1.2.3" → "~1.3.0"  (tilde preserved)
">=1.0.0" → ">=2.0.0" (range preserved)
```

## Age Filter

The `--age` option ensures stability by only updating to versions that have been released for a certain period:

```bash
# Only update to versions at least 2 weeks old
depup --age 2w

# Only update to versions at least 10 days old
depup --age 10d

# Only update to versions at least 1 month old
depup --age 1m
```

### pnpm Integration

depup automatically reads `minimumReleaseAge` from pnpm configuration:

**Priority order:**
1. CLI `--age` flag (highest)
2. `.npmrc` (`minimum-release-age=10d`)
3. `pnpm-workspace.yaml` (`minimumReleaseAge: 14400` in minutes)
4. `package.json` (`pnpm.settings.minimumReleaseAge`)

## Output

### Progress Display

<p align="center">
  <img src="docs/images/scanning.png" alt="depup scanning">
</p>

### Text Output (Default)

- `🔧` indicates devDependencies
- Release date shown in `(yyyy/mm/dd HH:MM)` format
- Change type: `[major]`, `[minor]`, `[patch]`

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

## Monorepo Support

### pnpm Workspaces

depup detects `pnpm-workspace.yaml` and processes all workspace packages.

### Tauri Projects

depup automatically detects `src-tauri/Cargo.toml` in Tauri projects.

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
