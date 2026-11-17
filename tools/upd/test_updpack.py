#!/usr/bin/env python3
"""Checks for updpack.py that need no container and no emulator.

    python3 tools/upd/test_updpack.py

The emulator image's rootfs comes from a container export, which leaves
/dev empty.  cdj3k-emu's provisioning guest runs a script as init inside
it and reads that script's console output for a "handed back" marker;
with no /dev/console the output is lost and the install fails with "the
provisioning guest did not hand back a rootfs" (v0.3.23, reproduced and
fixed 2026-11-17).  So the cpio must carry /dev/console itself, as the
device: docker's export (CI) leaves an empty regular file there instead,
which is how 0.3.29 still shipped broken.
"""
import gzip, io, os, sys, tarfile, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import updpack


def cpio_entries(data):
    """(name, mode, rdevmajor, rdevminor) of each newc entry."""
    out, i = [], 0
    while True:
        h = data[i:i + 110]
        f = [int(h[6 + 8 * k:14 + 8 * k], 16) for k in range(13)]
        mode, size, rmaj, rmin, nsz = f[1], f[6], f[9], f[10], f[11]
        name = data[i + 110:i + 110 + nsz - 1].decode()
        if name == "TRAILER!!!":
            return out
        out.append((name, mode, rmaj, rmin))
        i += 110 + nsz
        i += (-i) % 4
        i += size
        i += (-i) % 4


def rootfs_tar(console):
    """A tiny rootfs as a container export writes it.  `console`: None (podman:
    /dev a bare folder), "file" (docker: an empty regular file), "chr" (the
    device itself)."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as t:
        for d in ("dev", "usr", "usr/bin"):
            ti = tarfile.TarInfo(d); ti.type = tarfile.DIRTYPE; ti.mode = 0o755; t.addfile(ti)
        ti = tarfile.TarInfo("usr/bin/bash"); body = b"#!fake\n"; ti.size = len(body); ti.mode = 0o755
        t.addfile(ti, io.BytesIO(body))
        if console == "chr":
            ti = tarfile.TarInfo("dev/console"); ti.type = tarfile.CHRTYPE; ti.devmajor, ti.devminor = 5, 1; ti.mode = 0o600
            t.addfile(ti)
        elif console == "file":
            ti = tarfile.TarInfo("dev/console"); ti.size = 0; ti.mode = 0o755
            t.addfile(ti, io.BytesIO(b""))
    f = tempfile.NamedTemporaryFile(suffix=".tar", delete=False)
    f.write(buf.getvalue()); f.close()
    return f.name


def main():
    for console in (None, "file", "chr"):
        out = io.BytesIO()
        updpack.rootfs_cpio(rootfs_tar(console), [], out)
        ents = cpio_entries(out.getvalue())
        consoles = [e for e in ents if e[0] == "dev/console"]
        assert len(consoles) == 1, f"one dev/console (rootfs had: {console}): {ents}"
        _, mode, maj, minor = consoles[0]
        assert mode & 0o170000 == 0o020000 and (maj, minor) == (5, 1), f"char device 5,1: {consoles[0]}"
        assert "usr/bin/bash" in [e[0] for e in ents]
    print("ok: the emulator rootfs carries /dev/console")


if __name__ == "__main__":
    main()
