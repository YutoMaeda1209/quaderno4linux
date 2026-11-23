# Patched xattr 0.2.3

`gpui 0.2.2` depends (via `zed-async-tar`) on `xattr 0.2.3`, which no longer
builds against `libc >= 0.2.190` because `libc::ENOATTR` was removed for Linux.
This copy only replaces `libc::ENOATTR` with `libc::ENODATA` (same value on
Linux) in `src/sys/mod.rs`. Remove it once gpui moves to a newer `xattr`.
