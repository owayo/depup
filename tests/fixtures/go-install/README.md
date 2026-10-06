# Offline Go install fixtures

These archives contain fictional modules for the Go install regression test in
`src/package_manager.rs`. Version `v1.1.0` of `example.com/direct` imports a new
transitive dependency, `example.com/indirect v1.0.0`; version `v1.0.0` does not.

Regenerate the deterministic, uncompressed ZIP archives with:

```sh
python3 tests/fixtures/go-install/generate.py
```

The Rust test creates the proxy metadata and a sample application in a temporary
directory. It uses this file proxy with the checksum server disabled and isolated
module/build caches, so it needs Go 1.17 or later but no network access.
The test is skipped when Go is unavailable. Python is only needed to regenerate
the committed archives.
