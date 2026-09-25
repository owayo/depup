# Configuration

depup has two configuration files of its own: the global configuration file for your defaults, and `.depup` for the directories of a monorepo. A minimum release age written in a project's pnpm, Bun, or mise settings also applies; how it combines with `--age` and the global configuration is described in [Resolution Priority](usage.md#resolution-priority).

## Global Configuration File

On its first run, depup creates `~/.config/depup/config.toml`, which lists the default settings with explanatory comments; it never overwrites an existing file. Edit it to change the defaults for every project. A command-line flag still wins for a single run (see the priority lists in [Filtering Candidates](usage.md#filtering-candidates)).

The path is the same on every OS: `.config/depup/config.toml` under your home directory (on Windows, `%USERPROFILE%\.config\depup\config.toml`).

The generated file looks like this:

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

## `.depup` Configuration File

For monorepo projects with multiple subdirectories, create a `.depup` file at the project root to list additional directories to process:

```text
# .depup
gui       # Frontend app
api       # Backend API
shared    # Shared libraries
```

Run `depup` from the root directory to update dependencies across all listed directories at once. The root directory itself is always scanned in addition to the listed directories. Version lookups are cached, so shared packages are only fetched once.

When `--install` is used, depup runs each package manager in the deepest listed directory that contains the updated manifest, so nested apps install in their own directories instead of the repository root ([Running the Package Manager](usage.md#running-the-package-manager---install)).

The `.depup` format:

- `#` starts a comment (line or inline)
- Empty lines are ignored
- Paths are relative to the `.depup` file location
- If any entry is an absolute path, contains `..`, or is a symlink that resolves outside the directory containing `.depup`, depup prints a warning and ignores the whole `.depup` file. The run then behaves as if there were no `.depup`: the root and its automatically detected workspaces are still processed
- Non-existent directories are ignored with a warning
