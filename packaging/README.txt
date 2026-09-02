bindb
=====

Official compiled release of bindb.

bindb is a code intelligence and repository analysis storage engine.
This archive contains the public executable only. Source code is privately
maintained and is not included.

Installation
------------

1. Verify the archive against the matching `*.tar.xz.sha256` file from the
   GitHub Release (or use the release’s `sha256.sum`).
2. Extract this archive (`tar -xJf …` for `.tar.xz`).
3. Place `bindb` (or `bindb.exe`) on your PATH.

Verify
------

    bindb --version
    bindb --help

Phase 1 of the public binary supports only `--version` and `--help`.
It does not open or query on-disk stores. Database access remains via the
private Rust BinderStore API until a later release.

License
-------

See LICENSE.txt in this archive. This is proprietary binary distribution,
not open-source software.

Support
-------

See https://github.com/dpkimball/bindb-releases
