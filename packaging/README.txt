bindb
=====

Official compiled release of bindb.

bindb is a code intelligence and repository analysis storage engine.
This archive contains the public executable only. Source code is privately
maintained and is not included.

There are no SQL tables. A store is a directory that contains commits.wal.
Object IDs look like URIs (demo://notes/1); prefixes group related objects.

Installation
------------

1. Verify the archive against the matching `*.tar.xz.sha256` file from the
   GitHub Release (or use the release’s `sha256.sum`).
2. Extract this archive (`tar -xJf …` for `.tar.xz`).
3. Place `bindb` (or `bindb.exe`) on your PATH.

Quick start
-----------

    bindb init ./mystore
    bindb put  ./mystore "demo://hello" "world"
    bindb get  ./mystore "demo://hello"
    bindb info ./mystore
    bindb list ./mystore "demo://"

See GETTING_STARTED.md on https://github.com/dpkimball/bindb-releases

Verify
------

    bindb --version
    bindb --help

License
-------

See LICENSE.txt in this archive. This is proprietary binary distribution,
not open-source software.

Support
-------

See https://github.com/dpkimball/bindb-releases
