# ParKur (NextOS Installer)

Windows üzerinden Pardus/Debian tabanlı **canlı (live)** ISO'ları, bölümleme yapmadan,
NTFS üzerindeki tek bir `root.disk` dosyasının içine kuran masaüstü uygulaması
(Wubi tarzı loop-mount mimarisi). Tauri v2 (Rust backend + vanilla HTML/CSS/JS frontend).

UI/uygulama adı **ParKur**, boot girdisi ve disk üzerindeki artifact'lar **NextOS** olarak adlandırılır.

## Mimari: Loop-Mount (Wubi tarzı)

Preseed/Debian Installer **kullanılmaz**; live ISO'nun squashfs'i doğrudan açılır:

1. **Windows tarafı (kurulum):**
   - Ortam denetimi tek PowerShell probe ile: admin, UEFI, Secure Boot, boş alan,
     volume serial, **BitLocker** (şifreliyse kurulum reddedilir)
   - `X:\NextOS\root.disk` `fsutil` ile prealloc edilir (baş/son sıfırlanır),
     `filesystem.squashfs` + kernel + initrd ISO'dan kopyalanır
   - `overlay.cpio.gz` Rust içinde native üretilir (cpio newc + gzip) — içinde
     yalnızca `init` ve `nextos-firstboot` vardır
   - Shim/GRUB zinciri ESP'ye kopyalanır (`EFI\NextOS` + `EFI\BOOT` fallback),
     grub.cfg tüm olası GRUB prefix'lerine yazılır; **üzerine yazılan her dosya
     önce `.parkur-backup` olarak yedeklenir** (uninstall geri yükler)
   - `bcdedit /copy {bootmgr}` ile UEFI firmware girdisi ("NextOS") oluşturulur
2. **İlk açılış (Linux):**
   - GRUB iki initrd yükler: stok `initrd.img` + `overlay.cpio.gz`; overlay'deki
     `/init` live-boot'un init'ini **ezer**
   - `/init`: NTFS host'u marker dosyasıyla bulur (serial ile DEĞİL — Windows 32-bit
     serial ile blkid'in 64-bit NTFS serial'ı asla eşleşmez), rw mount eder,
     `root.disk`'i loop'a bağlar, ilk açılışta squashfs içinden chroot'la
     `mkfs.ext4` + `unsquashfs` çalıştırır, `switch_root` yapar
   - `nextos-firstboot.service`: kullanıcı oluşturma (parola **SHA-512 crypt hash**
     olarak gelir, `chpasswd -e` ile uygulanır), hostname/locale/timezone/klavye,
     canlı imaj kullanıcılarının (`pardus`, `user`, `live`, UID≥1000 artıkları) ve
     autologin config'lerinin temizliği, Calamares/live paketlerinin purge edilmesi,
     kalıcı initramfs hook'ları (kernel güncellemeleri `NextOS\boot`'a senkronlanır),
     provisioning config'in (host taraf dahil) imha edilmesi, reboot

## Proje Yapısı

- **src/index.html** — Frontend tamamı (stil + JS gömülü). 5 adımlı wizard:
  ISO Seç → Kullanıcı Hesabı → Disk ve Boyut → Kurulum → Tamamlandı
- **src-tauri/src/**
  - `lib.rs` — Tauri komutları, kurulum orkestrasyon akışı, SHA-512 parola hash'leme
  - `main.rs` — Giriş noktası, UAC self-elevation (yalnızca release build)
  - `disk_ops.rs` — Ortam probe'u (admin/UEFI/SecureBoot/BitLocker/serial/boş alan),
    NTFS bölüm listeleme, Fast Startup denetimi
  - `iso_ops.rs` — ISO mount/dismount, kernel/initrd/squashfs arama
  - `image_ops.rs` — root.disk prealloc, squashfs kopyalama (progress'li),
    provisioning config yazımı (tek tırnak quote'lu, yalnızca hash)
  - `initramfs_ops.rs` — Overlay initrd üretici (cpio newc + gzip, saf Rust)
  - `boot_ops.rs` — ESP mount, EFI payload + grub.cfg (yedekli yazım), BCD/UEFI
    girdi yönetimi, uninstall'da geri yükleme, reboot
  - `error.rs` — Merkezi hata tipi (`InstallerError`)
  - `util.rs` — PowerShell çalıştırıcı, yardımcılar
- **src-tauri/overlay-template/**
  - `init` — Özel initramfs `/init` (overlay'e gömülür; busybox uyumlu olmalı!)
  - `usr/local/sbin/nextos-firstboot` — İlk açılış provisioning script'i (bash)

## Kritik Kurallar / Bilinmesi Gerekenler

- `overlay-template/init` ve `nextos-firstboot` **`include_str!` ile derlemeye gömülür**
  (`initramfs_ops.rs`); yeni overlay dosyası eklemek için `stage_overlay_entries()`
  güncellenmelidir. CRLF → LF dönüşümü otomatik yapılır.
- `init` ilk açılışta yalnızca **stok ISO initrd'sinin busybox araçlarıyla** çalışır;
  util-linux'a özgü sözdizimi ancak opsiyonel hızlı yol olarak kullanılabilir.
- `nextos.conf` değerleri POSIX tek tırnakla quote'lanır (`sh_quote`), parola asla
  düz metin yazılmaz (`NEXTOS_PASSWORD_HASH`, `$6$...`).
- root.disk format kararı blkid ile değil, host'taki `.nextos-formatted` marker
  dosyasıyla verilir (yanlış pozitif/negatif format felaketini önler).
- ESP'de üzerine yazılan yabancı dosyalar (`BOOTX64.EFI`, mevcut `grub.cfg`'ler)
  ilk kurulumda bir kez `.parkur-backup` uzantısıyla yedeklenir; uninstall
  (`cleanup_esp_payload`) bunları geri yükler.
- Legacy BIOS desteklenmez; BitLocker'lı hedef reddedilir; Fast Startup durumu
  UI'da gösterilir (engellemez, uyarır).
- Dev modunda self-elevation atlanır — terminali "Yönetici olarak çalıştır" ile açın.

## Derleme

```bash
npm install
npx tauri build
```

Çıktılar `src-tauri/target/release/bundle/` altında oluşur.
