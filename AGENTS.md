# ParKur (Next OS Installer)

Windows üzerinden Pardus/Debian tabanlı Linux ISO'larını diske kalıcı kuran bir masaüstü uygulaması. Tauri v2 (Rust backend + vanilla HTML/CSS/JS frontend) ile geliştirilmiştir.

## Ne Yapıyor?

1. Kullanıcıdan bir Pardus/Debian ISO dosyası seçmesini istiyor
2. Kullanıcı bilgilerini alıyor (kullanıcı adı, şifre, hostname)
3. Hedef diski belirlettiriyor
4. Preseed.cfg oluşturup ISO'yu otomatik kurulum için hazırlıyor
5. GRUB bootloader'ı ESP'ye kurup BCD kaydı oluşturuyor
6. Yeniden başlatarak Debian Installer'ı unattended modda çalıştırıyor

## Mimari: Preseed + Debian Installer (Unattended)

GUI tabanlı Live masaüstü **kullanmaz**. Debian'ın resmi preseed mekanizmasını kullanır:
- ISO içindeki `install/vmlinuz` + `install/initrd.gz` (d-i kernel) ile boot
- Preseed.cfg küçük bir cpio initrd olarak ek initrd şeklinde yüklenir
- Debian Installer tamamen otomatik çalışır: disk bölümleme, dosya sistemi, paket kurulumu, kullanıcı oluşturma, GRUB kurulumu
- ISO remaster gerekmez — orijinal ISO loopback ile boot edilir

## Proje Yapısı

- **src/** — Frontend (HTML/CSS/JS). Wizard arayüzü: ISO Seç → Kullanıcı Bilgileri → Disk Seç → Kurulum → Tamamlandı
- **src-tauri/src/** — Rust backend:
  - `lib.rs` — Tauri command handler'ları, kurulum akışı orchestration
  - `disk_ops.rs` — PowerShell ile disk/bölüm listeleme, yönetici yetkisi kontrolü
  - `iso_ops.rs` — ISO montaj/demontaj, Linux kernel dosyası arama (d-i + live)
  - `boot_ops.rs` — UEFI/BIOS tespiti, ESP montaj, BCD yapılandırma, reboot
  - `error.rs` — Merkezi hata tipi (`InstallerError`)
  - `main.rs` — Giriş noktası, UAC yönetici yükseltme (release build)

## Yapılacaklar (Preseed Yaklaşımı)

- [ ] `preseed_ops.rs` — Preseed.cfg üretici (disk, kullanıcı, locale, paket seçimi)
- [ ] `cpio_ops.rs` — cpio newc arşiv üretici (preseed.cfg'yi initrd olarak paketleme)
- [ ] `boot_ops.rs` — GRUB setup yeniden yazılacak (loopback ISO boot + dual initrd)
- [ ] Frontend: Kullanıcı bilgileri adımı eklenmeli
- [ ] Frontend: Kurulum akışı yeni mimariyle bağlanmalı
- [ ] `lib.rs` — Yeni `start_installation` komutu (preseed akışı)

## Mevcut Durum

- Tauri v2 proje iskeleti kuruldu
- Yönetici yetkisi kontrolü ve otomatik UAC yükseltme
- UEFI/Legacy BIOS modu tespiti
- Disk bölümlerini listeleme
- ISO montaj/demontaj
- Linux kernel (vmlinuz/initrd) arama (d-i + live paths)
- ESP montaj, BCD kayıt oluşturma/temizleme
- Otomatik reboot
- 4 adımlı wizard arayüzü (ISO seç → Disk → Kurulum → Tamamlandı)

## Derleme

```bash
npm install
npx tauri build
```

Çıktılar `src-tauri/target/release/bundle/` altında oluşur.
