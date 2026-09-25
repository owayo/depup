# Command-Line Reference

The syntax, every option, more examples, and the output formats and exit codes of depup. How depup chooses versions is explained in the [Usage Guide](usage.md).

## Basic Syntax

```bash
depup [OPTIONS] [PATH]
```

`PATH` is the directory to process (defaults to the current directory). With `--cd`, depup changes to that directory first and resolves `PATH` from there.

## Options

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

## Examples

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

## Output and Exit Codes

### Progress Display

<p align="center">
  <img src="images/scanning.png" alt="depup scanning">
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
| `1` | A package manager install failed, `--cd` could not change the directory, or depup itself could not continue (for example, the HTTP client failed to initialize or the results could not be written). |
| `2` | Part of the run failed: a manifest could not be read, parsed, or written (including a refused ambiguous write), or a registry lookup failed. Invalid command-line arguments also exit with `2`. |

A code-2 failure does not stop the run. A failed lookup skips that dependency, and a manifest that cannot be read or parsed is left out; everything else is still written and installed. A failed write leaves that file unchanged, but its dependencies are still listed as updated and `--install` still runs for it. When both `1` and `2` apply, `1` wins.

These lookup problems leave the exit code unchanged:

- A failed `git ls-remote` for a Cargo git dependency (the dependency is skipped)
- A registry that returns no usable versions (`fetch failed: no versions available`)
- A missing `mise` command (mise config files are ignored with a warning)

There is no dedicated code for "updates available"; use `--json` to inspect the result in CI.

Errors are listed in the `Errors:` section of the text output and in the `errors` array of the JSON output. With `--diff`, or with `--quiet` in text output, the list is not printed unless `--verbose` is also given (it then goes to stderr), so check the exit code to detect failures.
