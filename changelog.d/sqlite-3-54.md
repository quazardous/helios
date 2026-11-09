kind: security

The SQLite built into Helios is updated from 3.53.2 to 3.54.1 (RUSTSEC-2026-0459, CVE-2026-119375): older versions could return stale memory when opening a database whose -wal file has a different page size. Helios only opens its own library and darktable library files, so the risk was low.
