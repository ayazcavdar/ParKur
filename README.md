# ParKur — Linux Installer for Windows

A desktop application that installs Pardus/Debian-based Linux ISOs to disk from within Windows.  
Built with Tauri v2 (Rust backend + vanilla HTML/CSS/JS frontend).

---

## What It Does

1. Prompts the user to select a Linux ISO, account credentials, and a target disk partition
2. Shrinks the selected NTFS partition and creates a new one
3. Copies the ISO and user configuration (`parkur.conf`) to the new partition
4. Builds a supplementary initrd (`parkur-hook.img`) and writes it to disk
5. Installs a GRUB EFI bootloader to the ESP (with headless boot parameters)
6. Adds a boot entry to the Windows BCD
7. Reboots the system to start the headless autonomous installation

## How It Works (Headless Installation)

The system does **not** use a GUI-based Live desktop. After boot:

- The ISO is loaded into RAM via `toram`, freeing the NTFS partition
- The graphical interface is bypassed with `systemd.unit=multi-user.target`
- The installation engine is injected via a live-bottom hook in the supplementary initrd
- `parkur-engine.sh` autonomously handles: NTFS→ext4 conversion, squashfs extraction, chroot configuration, and GRUB installation
- A 3-layer reboot strategy (systemctl → reboot -f → sysrq) prevents hangs

## Build

```bash
npm install
npx tauri build
```

Output bundles are generated under `src-tauri/target/release/bundle/`.

## Project Structure

```
src/              # Frontend (HTML/CSS/JS) — 5-step wizard
src-tauri/src/    # Rust backend
  lib.rs          # Tauri command handlers
  disk_ops.rs     # Disk partitioning, NTFS listing/shrinking
  iso_ops.rs      # ISO mount/unmount, kernel file lookup
  boot_ops.rs     # UEFI/BIOS detection, GRUB, BCD
  config_ops.rs   # User configuration, SHA-512 password hash
  cpio_ops.rs     # cpio/initrd builder, installation engine
  error.rs        # Central error type
```

## License

Apache 2.0 with Commons Clause — see [LICENSE](LICENSE)

> Source code may be freely used, modified, and distributed;  
> however, this software may **not** be sold as a paid product or service.

---

© 2026 Ayaz Çavdar
