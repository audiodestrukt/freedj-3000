#!/usr/bin/env python3
"""Pack OpenDeck into CDJ-3000 `.UPD` files.

    updpack.py emu  --rootfs rootfs.tar --add SRC:DEST ... --version V --out DIR [--key FILE]
    updpack.py deck --payload DIR --version V --out DIR [--key FILE]
    updpack.py luks --key FILE IN.iso OUT.UPD
    updpack.py open --key FILE IN.UPD OUT.iso      (check a UPD: decrypt it again)

The layout of each file and how cdj3k-emu / a deck consume it are in
docs/hardware/cdj-3000-upd.md. Everything up to the encryption is built
without a key, so anyone can inspect what would go into a UPD: without
--key the plaintext ISO stays in the output directory and nothing is
encrypted.

The key is read only from the path given. It is used as a LUKS1 keyslot
passphrase (the way `cryptsetup --key-file` uses a file) and is never
copied, logged or written anywhere: the UPD's header holds a PBKDF2 salt
and the master key wrapped by the derived slot key, not the key itself.

Needs: python3 with `cryptography`, genisoimage (or mkisofs).
"""

import argparse
import gzip
import hashlib
import io
import os
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import uuid as uuidlib

# ── LUKS1 ─────────────────────────────────────────────────────────────────────
# Same parameters as Pioneer's own CDJ-3000 releases (read from the 3.22
# header): aes / xts-plain64 / sha256, a 64-byte master key, one active slot
# at sector 8 with 4000 stripes, payload at sector 4096.

SECTOR = 512
KEY_BYTES = 64
STRIPES = 4000
PAYLOAD_SECTOR = 4096
SLOT_ITERATIONS = 1_000_000
DIGEST_ITERATIONS = 100_000
SLOT_ACTIVE, SLOT_INACTIVE = 0x00AC71F3, 0x0000DEAD


def _xts(key64):
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

    def encrypt_sector(data, sector):
        tweak = struct.pack("<QQ", sector, 0)  # plain64: little-endian sector number
        enc = Cipher(algorithms.AES(key64), modes.XTS(tweak)).encryptor()
        return enc.update(data) + enc.finalize()

    return encrypt_sector


def _af_diffuse(block, hash_name):
    h = hashlib.new(hash_name)
    ds = h.digest_size
    out = bytearray()
    for i in range(0, len(block), ds):
        chunk = block[i:i + ds]
        h = hashlib.new(hash_name)
        h.update(struct.pack(">I", i // ds) + chunk)
        out += h.digest()[:len(chunk)]
    return bytes(out)


def _af_split(key, stripes, hash_name):
    """LUKS1 anti-forensic split: stripes-1 random blocks, then the key
    XORed with the diffused running XOR."""
    n = len(key)
    d = bytes(n)
    out = bytearray()
    for _ in range(stripes - 1):
        r = os.urandom(n)
        out += r
        d = _af_diffuse(bytes(a ^ b for a, b in zip(d, r)), hash_name)
    out += bytes(a ^ b for a, b in zip(d, key))
    return bytes(out)


def luks1_encrypt(plain_path, out_path, passphrase):
    mk = os.urandom(KEY_BYTES)
    mk_salt = os.urandom(32)
    mk_digest = hashlib.pbkdf2_hmac("sha256", mk, mk_salt, DIGEST_ITERATIONS, 20)

    km_bytes = KEY_BYTES * STRIPES
    km_sectors = -(-km_bytes // SECTOR)
    km_stride = -(-km_sectors // 8) * 8  # each slot's area, 4 KiB aligned
    slot_offsets = [8 + i * km_stride for i in range(8)]
    assert slot_offsets[-1] + km_sectors <= PAYLOAD_SECTOR

    salt = os.urandom(32)
    slot_key = hashlib.pbkdf2_hmac("sha256", passphrase, salt, SLOT_ITERATIONS, KEY_BYTES)
    material = _af_split(mk, STRIPES, "sha256")
    material += bytes(km_sectors * SECTOR - len(material))
    enc = _xts(slot_key)
    # cryptsetup numbers key-material sectors from 0, whatever their offset.
    km_cipher = b"".join(enc(material[i:i + SECTOR], i // SECTOR)
                         for i in range(0, len(material), SECTOR))

    def cstr(s, n):
        b = s.encode()
        return b + bytes(n - len(b))

    hdr = bytearray()
    hdr += b"LUKS\xba\xbe" + struct.pack(">H", 1)
    hdr += cstr("aes", 32) + cstr("xts-plain64", 32) + cstr("sha256", 32)
    hdr += struct.pack(">II", PAYLOAD_SECTOR, KEY_BYTES)
    hdr += mk_digest + mk_salt + struct.pack(">I", DIGEST_ITERATIONS)
    hdr += cstr(str(uuidlib.uuid4()), 40)
    for i, off in enumerate(slot_offsets):
        if i == 0:
            hdr += struct.pack(">II", SLOT_ACTIVE, SLOT_ITERATIONS) + salt
        else:
            hdr += struct.pack(">II", SLOT_INACTIVE, 0) + bytes(32)
        hdr += struct.pack(">II", off, STRIPES)
    assert len(hdr) == 592

    penc = _xts(mk)
    with open(plain_path, "rb") as src, open(out_path, "wb") as out:
        out.write(hdr + bytes(slot_offsets[0] * SECTOR - len(hdr)))
        out.write(km_cipher)
        out.write(bytes(PAYLOAD_SECTOR * SECTOR - out.tell()))
        sector = 0
        while True:
            buf = src.read(SECTOR * 2048)
            if not buf:
                break
            if len(buf) % SECTOR:
                buf += bytes(SECTOR - len(buf) % SECTOR)
            out.write(b"".join(penc(buf[i:i + SECTOR], sector + i // SECTOR)
                               for i in range(0, len(buf), SECTOR)))
            sector += len(buf) // SECTOR


def luks1_decrypt(upd_path, out_path, passphrase):
    """The reverse, for checking a UPD this tool wrote (one slot, xts-plain64)."""
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

    def dec(key, data, sector):
        d = Cipher(algorithms.AES(key), modes.XTS(struct.pack("<QQ", sector, 0))).decryptor()
        return d.update(data) + d.finalize()

    with open(upd_path, "rb") as f:
        h = f.read(592)
        assert h[:6] == b"LUKS\xba\xbe", "not LUKS"
        payload, kb = struct.unpack(">II", h[104:112])
        digest, mk_salt = h[112:132], h[132:164]
        (dit,) = struct.unpack(">I", h[164:168])
        for i in range(8):
            o = 208 + i * 48
            act, it = struct.unpack(">II", h[o:o + 8])
            if act != SLOT_ACTIVE:
                continue
            salt = h[o + 8:o + 40]
            km_off, stripes = struct.unpack(">II", h[o + 40:o + 48])
            sk = hashlib.pbkdf2_hmac("sha256", passphrase, salt, it, kb)
            f.seek(km_off * SECTOR)
            n = kb * stripes
            km = f.read(-(-n // SECTOR) * SECTOR)
            mat = b"".join(dec(sk, km[j:j + SECTOR], j // SECTOR) for j in range(0, len(km), SECTOR))[:n]
            d = bytes(kb)
            for s in range(stripes - 1):
                d = _af_diffuse(bytes(a ^ b for a, b in zip(d, mat[s * kb:(s + 1) * kb])), "sha256")
            mk = bytes(a ^ b for a, b in zip(d, mat[(stripes - 1) * kb:]))
            if hashlib.pbkdf2_hmac("sha256", mk, mk_salt, dit, 20) == digest:
                break
        else:
            sys.exit("no keyslot opens with this key")
        f.seek(payload * SECTOR)
        with open(out_path, "wb") as out:
            sector = 0
            while True:
                buf = f.read(SECTOR * 2048)
                if not buf:
                    break
                out.write(b"".join(dec(mk, buf[i:i + SECTOR], sector + i // SECTOR)
                                   for i in range(0, len(buf), SECTOR)))
                sector += len(buf) // SECTOR


# ── cpio (newc) from the rootfs tar ──────────────────────────────────────────

S_IFMT = {"dir": 0o040000, "reg": 0o100000, "lnk": 0o120000,
          "chr": 0o020000, "blk": 0o060000, "fifo": 0o010000}


def _newc(out, name, mode, uid, gid, nlink, mtime, data, ino, rdev=(0, 0)):
    nb = name.encode() + b"\0"
    out.write(b"070701" + b"".join(b"%08X" % v for v in (
        ino, mode, uid, gid, nlink, int(mtime), len(data), 0, 0, rdev[0], rdev[1], len(nb), 0)))
    out.write(nb)
    out.write(bytes((-(110 + len(nb))) % 4))
    out.write(data)
    out.write(bytes((-len(data)) % 4))


def rootfs_cpio(tar_path, extra, out):
    """Write the rootfs tar plus the `extra` (src, dest, mode) files as a
    newc cpio. Hardlinks stay hardlinks: the first name carries the data,
    later names share its inode with no data, which is what the kernel's
    initramfs unpacker links."""
    tf = tarfile.open(tar_path)
    members = tf.getmembers()
    replaced = {d.strip("/") for _, d, _ in extra}
    links = {}
    for m in members:
        if m.islnk():
            links.setdefault(m.linkname.lstrip("./").strip("/"), []).append(m)
    ino = 1000
    inode_of = {}
    for m in members:
        name = m.name.lstrip("./").strip("/") if m.name not in (".", "./") else ""
        if not name or name in replaced:
            continue
        ino += 1
        if m.isdir():
            _newc(out, name, S_IFMT["dir"] | m.mode, m.uid, m.gid, 2, m.mtime, b"", ino)
        elif m.issym():
            _newc(out, name, S_IFMT["lnk"] | 0o777, m.uid, m.gid, 1, m.mtime, m.linkname.encode(), ino)
        elif m.islnk():
            target = m.linkname.lstrip("./").strip("/")
            t_ino, t_mode, t_n = inode_of[target]
            _newc(out, name, t_mode, m.uid, m.gid, t_n, m.mtime, b"", t_ino)
        elif m.isreg():
            n = 1 + len(links.get(name, []))
            mode = S_IFMT["reg"] | m.mode
            inode_of[name] = (ino, mode, n)
            _newc(out, name, mode, m.uid, m.gid, n, m.mtime, tf.extractfile(m).read(), ino)
        elif m.ischr() or m.isblk():
            kind = "chr" if m.ischr() else "blk"
            _newc(out, name, S_IFMT[kind] | m.mode, m.uid, m.gid, 1, m.mtime, b"", ino,
                  (m.devmajor, m.devminor))
        elif m.isfifo():
            _newc(out, name, S_IFMT["fifo"] | m.mode, m.uid, m.gid, 1, m.mtime, b"", ino)
    dirs_seen = {m.name.lstrip("./").strip("/") for m in members if m.isdir()}
    for src, dest, mode in extra:
        dest = dest.strip("/")
        parts = dest.split("/")[:-1]
        for i in range(1, len(parts) + 1):
            d = "/".join(parts[:i])
            if d not in dirs_seen:
                ino += 1
                _newc(out, d, S_IFMT["dir"] | 0o755, 0, 0, 2, 0, b"", ino)
                dirs_seen.add(d)
        ino += 1
        with open(src, "rb") as f:
            _newc(out, dest, S_IFMT["reg"] | mode, 0, 0, 1, os.path.getmtime(src), f.read(), ino)
    _newc(out, "TRAILER!!!", 0, 0, 0, 1, 0, b"", 0)


# ── ISO 9660 ─────────────────────────────────────────────────────────────────

def make_iso(src_dir, out_path, volid):
    tool = shutil.which("genisoimage") or shutil.which("mkisofs") or sys.exit(
        "needs genisoimage or mkisofs")
    # -relaxed-filenames keeps the '-' in CDJ3K-RK3399.ISO, which the
    # emulator looks up by its ISO 9660 name; -allow-multidot keeps
    # IMAGES.TAR.GZ from becoming IMAGES_TAR.GZ; -l allows long names.
    subprocess.run([tool, "-quiet", "-iso-level", "3", "-l", "-relaxed-filenames", "-allow-multidot",
                    "-R", "-J", "-V", volid, "-o", out_path, src_dir], check=True)


# ── the two images ───────────────────────────────────────────────────────────

CARRIER_HEAD = (b"OpenDeck rootfs carrier for cdj3k-emu (rk3399).\n"
                b"Not a kernel: cdj3k-emu boots its own kernel and takes the root\n"
                b"filesystem from the gzip cpio that follows. See\n"
                b"docs/hardware/cdj-3000-upd.md in the OpenDeck repository.\n")


def build_emu(a):
    os.makedirs(a.out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="upd-emu-", dir=a.out)
    extra = []
    for spec in a.add or []:
        src, dest = spec.split(":", 1)
        mode = 0o755 if os.access(src, os.X_OK) else 0o644
        extra.append((src, dest, mode))

    cpio_gz = os.path.join(a.out, "rootfs.cpio.gz")
    with gzip.open(cpio_gz, "wb", compresslevel=9) as gz:
        rootfs_cpio(a.rootfs, extra, gz)
    print(f"rootfs   {cpio_gz} ({os.path.getsize(cpio_gz) >> 20} MiB)")

    carrier = os.path.join(work, "Image")
    with open(carrier, "wb") as f, open(cpio_gz, "rb") as g:
        f.write(CARRIER_HEAD + bytes((-len(CARRIER_HEAD)) % 512))
        shutil.copyfileobj(g, f)

    inner = os.path.join(work, "inner", "IMAGES")
    os.makedirs(inner)
    with tarfile.open(os.path.join(inner, "IMAGES.TAR.GZ"), "w:gz", compresslevel=1) as t:
        t.add(carrier, arcname="rk3399/Image")
    for name, text in (("RELEASE.TXT", f"OpenDeck {a.version} (cdj3k-emu image)"),
                       ("APP.REV", a.version),
                       ("SYSTEM.REV", a.system_rev or a.version)):
        with open(os.path.join(inner, name), "w") as f:
            f.write(text + "\n")
    # The emulator only hashes MINILOADER.IMG for its U-Boot environment; a
    # deck would flash it, which is one more reason this file never goes near one.
    with open(os.path.join(inner, "MINILOADER.IMG"), "wb") as f:
        f.write(b"OPENDECK-EMU: no bootloader. Never install this UPD on a CDJ.\n")
    outer = os.path.join(work, "outer", "IMAGES")
    os.makedirs(outer)
    make_iso(os.path.dirname(inner), os.path.join(outer, "CDJ3K-RK3399.ISO"), "CDJ3K_RK3399")
    iso = os.path.join(a.out, f"CDJ3Kv{a.fw}-OPENDECK-EMU.iso")
    make_iso(os.path.dirname(outer), iso, "OPENDECK_EMU")
    shutil.rmtree(work)
    print(f"iso      {iso} ({os.path.getsize(iso) >> 20} MiB)")
    finish(a, iso, os.path.join(a.out, f"CDJ3Kv{a.fw}-OPENDECK-EMU.UPD"))


def build_deck(a):
    os.makedirs(a.out, exist_ok=True)
    iso = os.path.join(a.out, f"CDJ3Kv{a.fw}-OPENDECK.iso")
    make_iso(a.payload, iso, "OPENDECK")
    print(f"iso      {iso} ({os.path.getsize(iso) >> 20} MiB)")
    finish(a, iso, os.path.join(a.out, f"CDJ3Kv{a.fw}.UPD"))


def finish(a, iso, upd):
    if not a.key:
        print("no --key: stopping before encryption; the plaintext ISO above is what a UPD would hold")
        return
    with open(a.key, "rb") as f:
        passphrase = f.read()
    luks1_encrypt(iso, upd, passphrase)
    del passphrase
    print(f"upd      {upd} ({os.path.getsize(upd) >> 20} MiB)")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("emu", "deck"):
        s = sub.add_parser(name)
        s.add_argument("--version", required=True)
        s.add_argument("--fw", default="999", help="the ### in CDJ3Kv###; the emulator shows it as the slot's firmware number")
        s.add_argument("--out", required=True)
        s.add_argument("--key")
        if name == "emu":
            s.add_argument("--rootfs", required=True, help="rootfs tar (build-rootfs.sh)")
            s.add_argument("--add", action="append", help="SRC:DEST, a file to add to the rootfs")
            s.add_argument("--system-rev")
        else:
            s.add_argument("--payload", required=True, help="directory laid out as the deck payload")
    for name in ("luks", "open"):
        s = sub.add_parser(name)
        s.add_argument("--key", required=True)
        s.add_argument("src")
        s.add_argument("dst")
    a = p.parse_args()
    if a.cmd == "emu":
        build_emu(a)
    elif a.cmd == "deck":
        build_deck(a)
    else:
        with open(a.key, "rb") as f:
            k = f.read()
        (luks1_encrypt if a.cmd == "luks" else luks1_decrypt)(a.src, a.dst, k)


if __name__ == "__main__":
    main()
