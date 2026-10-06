"""Regenerate the offline Go proxy archives with Python's standard library."""

from pathlib import Path
from zipfile import ZIP_STORED, ZipFile, ZipInfo

MODULES = [
    (
        "direct",
        "v1.0.0",
        "module example.com/direct\n\ngo 1.17\n",
        'package direct\nfunc Message() string { return "old" }\n',
    ),
    (
        "direct",
        "v1.1.0",
        "module example.com/direct\n\ngo 1.17\n\nrequire example.com/indirect v1.0.0\n",
        (
            'package direct\nimport "example.com/indirect"\n'
            "func Message() string { return indirect.Message() }\n"
        ),
    ),
    (
        "indirect",
        "v1.0.0",
        "module example.com/indirect\n\ngo 1.17\n",
        'package indirect\nfunc Message() string { return "new" }\n',
    ),
]


if __name__ == "__main__":
    for name, version, manifest, source in MODULES:
        archive = Path(__file__).parent / f"{name}-{version}.zip"
        with ZipFile(archive, "w", compression=ZIP_STORED) as output:
            for filename, contents in [("go.mod", manifest), ("message.go", source)]:
                entry = ZipInfo(f"example.com/{name}@{version}/{filename}")
                entry.create_system = 3
                entry.external_attr = 0o100644 << 16
                output.writestr(entry, contents.encode("utf-8"))
