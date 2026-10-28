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
| `--osv` | | Check OSV.dev and prefer versions without known vulnerabilities; allow updates that fix existing vulnerabilities under severity and count limits by default |
| `--no-osv` | | Disable OSV vulnerability check for this run (overrides global config and default) |
| `--no-osv-risk-tradeoff` | | Reject every candidate with known vulnerabilities for this run, even if it fixes existing vulnerabilities |
| `--max-change <LEVEL>` | | Limit allowed bumps: `patch` (patch only), `minor` (patch + minor), `major` (default — all) |
| `--json` | | Output results in JSON format |
| `--diff` | | Show changes in diff format |
| `--install` | | Run package manager install after update |
| `--refresh-lock` | | Re-resolve pnpm/Cargo lockfiles, including transitive dependencies; requires `--install`; incompatible with `--only`, `--exclude`, and `--max-change` (including global defaults) |
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
- Known release dates are shown in `(yyyy/mm/dd HH:MM)` format; unavailable dates appear as `(release date unknown)`
- Change type: `[major]`, `[minor]`, `[patch]`
- `✓ OSV` marks versions with no known OSV advisories. Updates allowed with remaining vulnerabilities carry no such mark; text reports fixed vulnerabilities, new vulnerabilities, and existing vulnerabilities that remain, and JSON adds `osv_risk_tradeoff` to the update with `baseline_versions`, `candidate_versions`, `fixed`, `introduced`, and `retained` arrays. Each issue group has deduplicated `ids` and a normalized `severity`. A `↳ OSV skipped:` line below an update lists the vulnerable versions that were removed from the candidates; the dependency itself was still updated
- When OSV excludes every eligible update, the reason is `no eligible update without known vulnerabilities`. Normal output includes the manifest baseline's vulnerability status and, when confirmed, a safe newer version with its age eligibility time. JSON includes this skip with reason `known_vulnerabilities` and structured `osv` details even without `--verbose`. Cargo age rollback notices also flag vulnerable destinations remaining in the final lockfile; these notices do not change the exit code
- Skipped dependencies are counted in each manifest heading (`— N updates, M skips`) and in the summary; a manifest with no updates also shows the count per reason
- An update that could not be written to the manifest or was canceled after an install failure ends with `✗ failed: <reason>` (`[failed: <reason>]` without color). Multiline reasons are displayed on one line with whitespace collapsed. Failed updates are not counted as updates: the heading becomes `— N updates, K failed, M skips`, and the summary adds `K package(s) failed to update`.
- With `--verbose`, every skipped dependency is listed under its reason, and the summary is broken down by reason and language
- The `latest` and `newer than --age` skip groups show the current version's own publication date when confirmed. Unknown dates and floating wildcard/prefix specifications have no date. `newer than --age` means the current release is too young for the effective age policy and is not publisher-exempt; it does not downgrade the dependency
- Errors, including OSV notices, are listed in the `Errors:` section

### JSON Output

With `--refresh-lock`, `lock_refreshes` reports each lock scope's `directory`, `language`, `status` (`completed`, `failed`, or `unsupported`), `changed`, and `error`. `changed` means an accepted refresh changed the lock; a failed refresh has `changed: false`, including when an external edit prevents recovery. If no eligible scope exists, `language` is `null`. Direct dependency counts retain their existing meaning. A Cargo refresh can be `completed` while the command exits with code 2 when only pre-run versions remain outside the age policy. See [Refreshing Lockfiles](usage.md#refreshing-lockfiles---install---refresh-lock) for verification and recovery.

With `--json --install`, output is deferred until install recovery and age audits finish, then one JSON value is flushed even if install fails. Updates canceled by a failed project, including recovery conflicts and lock-only Git updates, move from `updates` to `failed` with a reason. Successful independent projects remain in `updates`. Recovery details and copy locations go to stderr.

```bash
depup --json
```

```json
{
  "dry_run": false,
  "summary": {
    "updates": 1,
    "failed": 1,
    "skips": 1
  },
  "manifests": [
    {
      "path": "./package.json",
      "language": "Node.js",
      "updates": [
        {
          "name": "lodash",
          "kind": "registry",
          "from": "4.17.20",
          "to": "4.17.21",
          "dev": false
        }
      ],
      "failed": [
        {
          "name": "react",
          "kind": "registry",
          "from": "18.0.0",
          "to": "19.2.0",
          "dev": false,
          "error": "Refusing to update ambiguous dependency 'react' because it has multiple declarations or a shared version target"
        }
      ]
    }
  ],
  "errors": [
    "Failed to write ./package.json: Refusing to update ambiguous dependency 'react' because it has multiple declarations or a shared version target"
  ]
}
```

`summary.updates` and each manifest's `updates` cover only the updates that were written (with `--dry-run`, the updates that would be written). Updates that could not be written are counted in `summary.failed` and listed in the manifest's `failed` array, with the reason in `error`. Git dependencies use `"kind": "git"` and add `source` (the repository URL with any credentials masked) and `reference`. With `--verbose`, each manifest also lists its `skips`, and the summary adds `by_language`.

Verbose skip entries use `reason: "current_version_too_recent"` when the confirmed current release is younger than the effective age policy and no update is selected. Other filters and errors keep their own reasons. This replaces `already_latest` for those age cases and also applies to the default and configured age values. Skip `released_at` belongs to the current release, rather than the latest eligible candidate; it is omitted when unknown or when the specification does not identify one release. For example:

```json
{
  "name": "library",
  "version": "2.0.0",
  "reason": "current_version_too_recent",
  "released_at": "2024-05-30T12:00:00+00:00"
}
```

The reporting rules and other skip reasons are described in [When a Dependency Is Not Updated](usage.md#when-a-dependency-is-not-updated).

### Diff Output

```bash
depup --diff
```

```diff
--- a/./package.json
+++ b/./package.json
@@ lodash @@
-  "lodash": "^4.17.20"
+  "lodash": "^4.17.21"

# 1 package(s) would be updated, 1 failed
```

Updates that could not be written get no hunk. The last line counts updates the same way as the text and JSON output, and adds `, K failed` only when some updates could not be written.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | No failures. This includes runs with no updates, dry runs, and runs whose only `Errors:` entries are OSV notices (fallbacks or failed lookups). |
| `1` | A package manager install failed or could not be prepared (whether or not its recovery completed), `--cd` could not change the directory, or depup itself could not continue (for example, the HTTP client failed to initialize, or the results or the recovery report could not be written). |
| `2` | Part of the run failed: a manifest could not be read, parsed, or written (including a refused ambiguous write), a registry lookup failed, or a Rust or Node lockfile age audit left violations or unverified entries (including an unreadable lock or an exhausted time budget). This includes `--install` runs that restored a unit because a version they added failed the audit, even when some files were kept for manual recovery. Invalid command-line arguments also exit with `2`. |

A code-2 failure does not stop the run. A failed lookup skips that dependency, and a manifest that cannot be read or parsed is left out; everything else is still written and installed. If a file cannot be saved, it keeps its original content. If only some updates in a manifest cannot be written (for example, one refused as ambiguous), the rest are still saved. Either way, the updates that could not be written are reported as failed instead of updated, and they do not make `--install` run. With `--install`, when a version this run added to a lockfile fails the age audit, the manifests and lockfiles of that install unit are restored to their state before the run and its updates are reported as failed (see [When the Final Lockfile Fails the Age Audit](usage.md#when-the-final-lockfile-fails-the-age-audit)). When both `1` and `2` apply, `1` wins.

These lookup problems leave the exit code unchanged:

- A failed `git ls-remote` for a Cargo git dependency (the dependency is skipped)
- A registry that returns no usable versions (`fetch failed: no versions available`)
- A missing `mise` command (mise config files are ignored with a warning)

There is no dedicated code for "updates available"; use `--json` to inspect the result in CI.

Errors are listed in the `Errors:` section of the text output and in the `errors` array of the JSON output. With `--diff`, or with `--quiet` in text output, the list is not printed unless `--verbose` is also given (it then goes to stderr), so check the exit code to detect failures.
