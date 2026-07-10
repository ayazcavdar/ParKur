# ParKur — Linux Installer for Windows

A desktop application that installs Pardus/Debian-based Linux live ISOs onto an existing
Windows machine **without repartitioning** — Wubi-style, into a single `root.disk` file
on an NTFS volume. Built with Tauri v2 (Rust backend + vanilla HTML/CSS/JS frontend).

> The boot entry and on-disk artifacts are branded **NextOS**; ParKur is the installer
> application itself.

---

## What It Does

1. The user selects a Pardus/Debian-based **live** ISO (64-bit UEFI)
2. Enters account credentials (username, password, hostname), locale and timezone
3. Picks a target NTFS partition and a `root.disk` size (dynamic min/max from ISO + free space)
4. ParKur copies the payload and configures the bootloader — **no partition is shrunk,
   moved or formatted**
5. On reboot, the "NextOS" UEFI entry boots Linux from inside the NTFS volume;
   first boot formats and provisions the system automatically

## How It Works (Loop-Mount Architecture)

Everything lives in a `NextOS\` folder on the chosen NTFS drive:

| File | Purpose |
|---|---|
| `NextOS\root.disk` | Preallocated raw file, formatted ext4 on first boot — the Linux root filesystem |
| `NextOS\filesystem.squashfs` | Root payload copied from the ISO |
| `NextOS\boot\vmlinuz`, `initrd.img` | Kernel + stock initrd copied from the ISO |
| `NextOS\boot\overlay.cpio.gz` | ParKur-built second initrd (cpio newc, built natively in Rust); rebuilt without secrets after first boot |
| `NextOS\nextos.conf` | Provisioning config (username, **SHA-512 password hash**, hostname, locale, timezone) — destroyed after first boot |
| `NextOS\.nextos-formatted` | Marker written after successful ext4 format — prevents accidental reformat |

**Install phase (Windows):**

- Environment probe: admin rights, UEFI firmware, Secure Boot, free space, **BitLocker**
  (BitLocker-encrypted targets are rejected); ISO preflight checks for UEFI kernel and
  NTFS modules in the stock initrd
- `root.disk` is preallocated with `fsutil`, head/tail zeroed to kill stale filesystem signatures
- The EFI shim/GRUB chain is copied from the ISO to `ESP:\EFI\NextOS` (plus the
  `EFI\BOOT\BOOTX64.EFI` removable fallback — the original file is backed up first)
- A `grub.cfg` that locates the NTFS volume by file search and boots
  `vmlinuz + initrd.img + overlay.cpio.gz` is written to every GRUB prefix candidate
  (existing configs are backed up)
- A UEFI firmware entry ("NextOS") is registered via `bcdedit /copy {bootmgr}`

**First boot (Linux):**

- The kernel unpacks both initrds; the overlay's `/init` **replaces** live-boot's init
- `/init` scans block devices, finds the NTFS volume containing `NextOS\root.disk`,
  mounts it read-write and loop-attaches `root.disk`
- First boot only: `mkfs.ext4` runs *from inside the squashfs via chroot*, then the
  rootfs is extracted with multi-threaded `unsquashfs` (cp -a fallback)
- `nextos-firstboot.service` provisions the system: creates the user (password applied
  from a one-time `install.pass` embedded in the overlay initrd, with SHA-512 hash
  fallback; login screen asks for a password each boot);
  **removes live-image users only after password is verified**;
  hostname, locale, timezone, keyboard layout;
  purges live/installer packages (Calamares etc.);
  rebuilds `overlay.cpio.gz` on NTFS without secrets;
  installs initramfs hooks so kernel updates keep syncing to `NextOS\boot`; then reboots

**Uninstall:** removes the `NextOS\` folder, UEFI entries and ESP payload, restores
backed-up bootloader files. Only drives with an existing NextOS install are offered.

## Requirements

- Windows 10/11, **UEFI** firmware (Legacy BIOS is not supported)
- Administrator rights (the app self-elevates via UAC)
- An NTFS volume with enough free space (root.disk + squashfs + 2 GB headroom)
- No BitLocker on the target volume; Windows **Fast Startup should be disabled**
  (the app shows its status on the first screen)

## Build

```bash
npm install
npx tauri build
```

Output bundles are generated under `src-tauri/target/release/bundle/`.

## Project Structure

```
src/                       # Frontend — single-file 5-step wizard (index.html)
src-tauri/src/
  lib.rs                   # Tauri commands, installation orchestration, password hashing
  main.rs                  # Entry point, UAC self-elevation (release builds)
  disk_ops.rs              # Environment probe (admin/UEFI/SecureBoot/BitLocker/serial),
                           # NTFS partition listing, Fast Startup check
  iso_ops.rs               # ISO mount/unmount, kernel/initrd/squashfs discovery
  image_ops.rs             # root.disk preallocation, squashfs copy, provisioning config
  initramfs_ops.rs         # Overlay initrd builder (cpio newc + gzip, pure Rust)
  boot_ops.rs              # ESP mount, EFI payload + grub.cfg (with backups), BCD/UEFI
                           # entries, uninstall restore, reboot
  error.rs                 # Central error type (InstallerError)
  util.rs                  # PowerShell runner, helpers
src-tauri/overlay-template/
  init                     # Custom initramfs /init (host discovery, loop-mount, bootstrap)
  usr/local/sbin/nextos-firstboot   # First-boot provisioning script
```

## License

Apache 2.0 with Commons Clause — see [LICENSE](LICENSE)

> Source code may be freely used, modified, and distributed;
> however, this software may **not** be sold as a paid product or service.

---

© 2026 Ayaz Çavdar
