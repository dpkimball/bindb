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

Inspect a store
---------------

The store directory is the folder that contains `commits.wal`.

    bindb info /path/to/store
    bindb list /path/to/store "code://"
    bindb list /path/to/store ""

License
-------

See LICENSE.txt in this archive. This is proprietary binary distribution,
not open-source software.

Support
-------

See https://github.com/dpkimball/bindb-releases
