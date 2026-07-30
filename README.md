# ParKur — Linux Installer for Windows

Installs Pardus/Debian **live** ISOs onto Windows **without repartitioning**, into a single
`root.disk` on NTFS (loop-mount). Built with Tauri v2.

Boot entry / on-disk files: **NextOS**. App UI: **ParKur**.

[Türkçe](README.tr.md) · [Demo video](video.mp4)

## Flow

1. Select a local ISO or download Pardus (XFCE / GNOME) in-app  
2. Set user, password, hostname, locale, timezone  
3. Pick an NTFS drive and `root.disk` size  
4. Install — partitions are not resized or formatted  
5. Reboot → **NextOS** UEFI entry; first boot provisions automatically  

UI: Turkish / English.

## Requirements

- Windows 10/11, **UEFI**, Administrator  
- Enough free NTFS space; no BitLocker on target; Fast Startup off  

## Build

```bash
npm install
npx tauri build
```

Output: `src-tauri/target/release/bundle/`

## License

Apache 2.0 with Commons Clause — [LICENSE](LICENSE).  
May not be sold as a paid product/service.

© 2026 Ayaz Çavdar
