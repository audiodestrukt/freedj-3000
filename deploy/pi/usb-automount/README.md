# USB automount for the OpenDeck appliance

A CDJ mounts a stick the instant it is inserted and survives it being yanked.
On a headless Raspberry Pi nothing does that by default, so this adds it with
the pieces already on the system: a **udev rule** that, for every filesystem
on a USB-attached block device, wants `opendeck-usb-mount@<dev>.service`; a
**systemd template unit** bound to that device, so systemd stops it (and
unmounts) when the stick disappears; and a **mount script** that puts the
stick at `/media/usb/<LABEL>`, read-only, world-readable, `noexec`.

```bash
sudo deploy/pi/usb-automount/install.sh      # also mounts whatever is plugged in now
journalctl -t opendeck-usb -f                # watch sticks come and go
```

OpenDeck scans `/media/usb` (with `/media`, `/media/$USER`, `/run/media/$USER`
and `/mnt`), lists each mount as a `USB: <label>` row, and re-scans once a
second — so the row appears about a second after insertion.

| file | installs to | role |
|---|---|---|
| `99-opendeck-usb.rules` | `/etc/udev/rules.d/` | `ID_BUS=usb` + `ID_FS_USAGE=filesystem` → `SYSTEMD_WANTS=opendeck-usb-mount@%k.service` |
| `opendeck-usb-mount@.service` | `/etc/systemd/system/` | `BindsTo=dev-%i.device`; ExecStart mounts, ExecStop unmounts |
| `opendeck-usb-mount` | `/usr/local/bin/` | `add`/`remove <dev>`: blkid label + type, sanitised dir name, per-fs options, lazy unmount |
| `opendeck-usb.default` | `/etc/default/opendeck-usb` | `OPENDECK_MEDIA_ROOT`, `OPENDECK_USB_RW` |

Choices worth knowing:

- **Read-only.** A read-only mount can never leave a dirty FAT behind when the
  stick is pulled mid-set, which is what a DJ will do. OpenDeck writes nothing
  to a stick today; set `OPENDECK_USB_RW=1` when it does (history, My Settings).
- **Filesystems.** rekordbox sticks are FAT32 or exFAT (kernel exfat, 5.7+,
  is in Raspberry Pi OS) and, from Macs, sometimes HFS+ (`hfsplus` module,
  read-only is fine). NTFS mounts via `ntfs3`.
- **Only USB.** The Pi's own SD card and any NVMe are never matched.
- **Desktop images already automount** under `/media/<user>`; do not install
  this there or every stick mounts twice.
- **Dry run** without touching the system:
  `OPENDECK_USB_DRYRUN=1 OPENDECK_USB_LABEL='CDJ 1' ./opendeck-usb-mount add sdz1`.
