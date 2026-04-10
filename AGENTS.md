# ParKur (Next OS Installer)

Windows üzerinden Pardus/Debian tabanlı Linux ISO'larını diske kuran bir masaüstü uygulaması. Tauri v2 (Rust backend + vanilla HTML/CSS/JS frontend) ile geliştirilmiştir.

## Ne Yapıyor?

1. Kullanıcıdan bir Linux ISO dosyası, hesap bilgileri ve hedef disk bölümü seçmesini istiyor
2. Seçilen NTFS bölümü küçültüp yeni bir bölüm oluşturuyor
3. ISO'yu ve kullanıcı yapılandırmasını (parkur.conf) yeni bölüme kopyalıyor
4. Supplementary initrd (parkur-hook.img) oluşturup diske yazıyor
5. ESP'ye GRUB EFI bootloader kuruyor (headless boot parametreleriyle)
6. Windows BCD'ye boot kaydı ekliyor
7. Sistemi yeniden başlatarak headless otonom kurulum başlatıyor

## Mimari: Headless/Unattended Kurulum

Sistem GUI tabanlı Live masaüstü **kullanmaz**. Boot sonrası:
- `toram` ile ISO RAM'e yüklenir, NTFS bölümü serbest kalır
- `systemd.unit=multi-user.target` ile grafik arayüz atlanır
- Supplementary initrd üzerinden live-bottom hook ile kurulum motoru enjekte edilir
- `parkur-engine.sh` otonom olarak: NTFS→ext4 dönüşümü, squashfs açılımı, chroot yapılandırması ve GRUB kurulumu yapar
- 3 katmanlı reboot stratejisiyle (systemctl → reboot -f → sysrq) donma önlenir

## Proje Yapısı

- **src/** — Frontend (HTML/CSS/JS). 5 adımlı wizard: ISO Seç → Kullanıcı Bilgileri → Disk Seç → Kurulum → Tamamlandı
- **src-tauri/src/** — Rust backend:
  - `lib.rs` — Tauri command handler'ları, kurulum akışı orchestration
  - `disk_ops.rs` — Disk bölümleme (diskpart/PowerShell), NTFS bölüm listeleme, küçültme
  - `iso_ops.rs` — ISO montaj/demontaj, Linux kernel dosyası arama, ISO kopyalama
  - `boot_ops.rs` — UEFI/BIOS tespiti, ESP montaj, GRUB kurulumu, BCD yapılandırma, reboot
  - `config_ops.rs` — Kullanıcı yapılandırması, SHA-512 şifre hash, parkur.conf yazma
  - `cpio_ops.rs` — cpio newc arşiv üretici, supplementary initrd builder, kurulum motoru
  - `error.rs` — Merkezi hata tipi (`InstallerError`)
  - `main.rs` — Giriş noktası, UAC yönetici yükseltme (release build)

## Yapılanlar

- Tauri v2 proje iskeleti kuruldu
- 5 adımlı kurulum wizard arayüzü (ISO seç → Kullanıcı Bilgileri → Disk seç → Kurulum → Tamamlandı)
- Yönetici yetkisi kontrolü ve otomatik UAC yükseltme
- UEFI/Legacy BIOS modu tespiti
- Disk bölümlerini listeleme ve kullanıcıya sunma
- Bölüm küçültme + yeni NTFS bölüm oluşturma (diskpart)
- ISO montaj/demontaj (PowerShell Mount-DiskImage)
- Linux kernel (vmlinuz/initrd) otomatik arama
- Kullanıcı bilgilerini toplama ve SHA-512 crypt hash ile güvenli saklama
- parkur.conf JSON yapılandırma dosyası oluşturma
- cpio newc + gzip ile supplementary initrd (parkur-hook.img) üretimi
- live-bottom hook ile switch_root öncesi enjeksiyon mekanizması
- systemd oneshot servis ile otonom kurulum motoru tetikleme
- parkur-engine.sh: NTFS→ext4 dönüşümü, unsquashfs, chroot yapılandırma
- GRUB EFI bootloader'ı ESP'ye kurma (headless parametrelerle)
- grub.cfg: toram + systemd.unit=multi-user.target + noprompt + noeject + dual initrd
- Windows BCD'ye boot girişi ekleme (bcdedit)
- Eski NextOS boot kayıtlarını temizleme
- 3 katmanlı pürüzsüz reboot stratejisi (systemctl → reboot -f → sysrq-trigger)
- Gerçek zamanlı ilerleme bildirimleri (Tauri event sistemi)
- Release build: EXE, MSI ve NSIS installer çıktısı

## Derleme

```bash
npm install
npx tauri build
```

Çıktılar `src-tauri/target/release/bundle/` altında oluşur.
