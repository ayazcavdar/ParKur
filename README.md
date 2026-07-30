# ParKur — Linux Installer for Windows

Desktop app that installs Pardus/Debian-based **live** ISOs onto an existing Windows PC
**without repartitioning**, into a single `root.disk` file on an NTFS volume
(loop-mount architecture). Built with Tauri v2 (Rust backend + vanilla HTML/CSS/JS).

> Boot entry and on-disk artifacts are branded **NextOS**. **ParKur** is the installer UI.

[Türkçe README](README.tr.md)

---

## What it does

1. Choose a local live ISO, or download the latest Pardus desktop ISO in-app (XFCE / GNOME)
2. Enter account details (username, password, hostname), locale, and timezone
3. Pick a target NTFS drive and a `root.disk` size (min/max from ISO + free space)
4. ParKur copies the payload and configures the bootloader — **no partition is shrunk,
   moved, or formatted**
5. After reboot, the “NextOS” UEFI entry boots Linux from the NTFS volume; first boot
   formats and provisions the system automatically

UI language: **Turkish** / **English** (sidebar switcher).

## How it works

Everything lives under `NextOS\` on the chosen NTFS drive:

| File | Purpose |
|---|---|
| `NextOS\root.disk` | Preallocated raw file; formatted ext4 on first boot (Linux root) |
| `NextOS\filesystem.squashfs` | Root payload copied from the ISO |
| `NextOS\boot\vmlinuz`, `initrd.img` | Kernel + stock initrd from the ISO |
| `NextOS\boot\overlay.cpio.gz` | Second initrd built by ParKur (native Rust cpio+gzip); rebuilt without secrets after first boot |
| `NextOS\nextos.conf` | Provisioning config (username, SHA-512 password hash, hostname, locale, timezone) — destroyed after first boot |
| `NextOS\.nextos-formatted` | Marker after successful ext4 format — prevents accidental reformat |

**Install (Windows):** environment checks (admin, UEFI, Secure Boot, BitLocker, Fast Startup,
free space, existing NextOS), ISO preflight (UEFI kernel + NTFS via initrd and/or squashfs),
`root.disk` preallocation, squashfs + boot files copy, shim/GRUB on ESP (with backups),
UEFI “NextOS” entry via `bcdedit`. If the live initrd lacks `ntfs3`, ParKur injects it from
the squashfs into the overlay.

**First boot (Linux):** overlay `/init` replaces live-boot; finds the NTFS host by marker,
loop-mounts `root.disk`, formats/extracts on first boot, runs `nextos-firstboot` (user,
locale, purge live packages, sync hooks), then reboots.

**Uninstall:** removes `NextOS\`, UEFI entries, and ESP payload; restores backed-up bootloader files.

## Requirements

- Windows 10/11, **UEFI** (Legacy BIOS not supported)
- Administrator rights (UAC self-elevation in release builds)
- NTFS volume with enough free space (`root.disk` + squashfs + headroom)
- No BitLocker on the target; Fast Startup must be off (ParKur can disable it)

## Build

```bash
npm install
npx tauri build
```

Bundles appear under `src-tauri/target/release/bundle/`.

## Project structure

```
src/                       # Frontend wizard (index.html + i18n.js)
src-tauri/src/             # Rust backend (install orchestration, disk/ISO/boot ops)
src-tauri/overlay-template/
  init                     # Custom initramfs /init
  usr/local/sbin/nextos-firstboot
```

## License

Apache 2.0 with Commons Clause — see [LICENSE](LICENSE).

> Source may be used, modified, and distributed; this software may **not** be sold
> as a paid product or service.

---

© 2026 Ayaz Çavdar
