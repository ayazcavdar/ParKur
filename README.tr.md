# ParKur — Windows için Linux Kurulum Aracı

Mevcut Windows bilgisayara Pardus/Debian tabanlı **canlı (live)** ISO’ları
**bölümleme yapmadan**, NTFS üzerindeki tek bir `root.disk` dosyasının içine kuran
masaüstü uygulaması (loop-mount mimarisi). Tauri v2 (Rust backend + vanilla HTML/CSS/JS).

> Önyükleme girdisi ve disk üzerindeki dosyalar **NextOS** olarak adlandırılır.
> **ParKur** kurulum arayüzünün adıdır.

[English README](README.md)

---

## Ne yapar?

1. Yerel bir canlı ISO seçin veya uygulama içinden güncel Pardus masaüstü ISO’sunu indirin (XFCE / GNOME)
2. Hesap bilgilerini girin (kullanıcı adı, parola, bilgisayar adı), dil ve saat dilimi
3. Hedef NTFS sürücüyü ve `root.disk` boyutunu seçin (alt/üst sınır ISO + boş alana göre)
4. ParKur dosyaları kopyalar ve önyükleyiciyi ayarlar — **hiçbir bölüm küçültülmez,
   taşınmaz veya biçimlendirilmez**
5. Yeniden başlatınca “NextOS” UEFI girdisi Linux’u NTFS içinden açar; ilk açılışta
   sistem otomatik biçimlenir ve yapılandırılır

Arayüz dili: **Türkçe** / **İngilizce** (kenar çubuğundan).

## Nasıl çalışır?

Seçilen NTFS sürücüde her şey `NextOS\` klasöründedir:

| Dosya | Amaç |
|---|---|
| `NextOS\root.disk` | Önceden ayrılmış ham dosya; ilk açılışta ext4 (Linux kök) |
| `NextOS\filesystem.squashfs` | ISO’dan kopyalanan kök yük |
| `NextOS\boot\vmlinuz`, `initrd.img` | ISO çekirdeği ve stok initrd |
| `NextOS\boot\overlay.cpio.gz` | ParKur’un ürettiği ikinci initrd; ilk açılıştan sonra sırlar silinerek yeniden yazılır |
| `NextOS\nextos.conf` | Kurulum yapılandırması (kullanıcı, SHA-512 parola özeti, hostname, dil, saat dilimi) — ilk açılıştan sonra imha edilir |
| `NextOS\.nextos-formatted` | Biçimlendirme işaretçisi — yanlışlıkla yeniden formatı önler |

**Kurulum (Windows):** ortam denetimleri (yönetici, UEFI, Secure Boot, BitLocker, Hızlı Başlatma,
boş alan, mevcut NextOS), ISO ön kontrolü, `root.disk` ayırma, squashfs + önyükleme kopyası,
ESP’ye shim/GRUB (yedekli), `bcdedit` ile “NextOS” UEFI girdisi. Canlı initrd’de `ntfs3` yoksa
ParKur modülü squashfs’ten overlay’e ekler.

**İlk açılış (Linux):** overlay `/init` live-boot’u ezer; NTFS host’u bulur, `root.disk`’i
loop’a bağlar, ilk açılışta format/çıkarma yapar, `nextos-firstboot` çalışır, yeniden başlar.

**Kaldırma:** `NextOS\`, UEFI girdileri ve ESP yükünü siler; yedeklenen önyükleyici dosyalarını geri yükler.

## Gereksinimler

- Windows 10/11, **UEFI** (Legacy BIOS desteklenmez)
- Yönetici yetkisi (sürüm derlemesinde UAC ile yükselme)
- Yeterli boş alanı olan NTFS sürücü (`root.disk` + squashfs + pay)
- Hedefte BitLocker olmamalı; Hızlı Başlatma kapalı olmalı (ParKur kapatabilir)

## Derleme

```bash
npm install
npx tauri build
```

Çıktılar `src-tauri/target/release/bundle/` altındadır.

## Proje yapısı

```
src/                       # Arayüz sihirbazı (index.html + i18n.js)
src-tauri/src/             # Rust backend
src-tauri/overlay-template/
  init                     # Özel initramfs /init
  usr/local/sbin/nextos-firstboot
```

## Lisans

Apache 2.0 + Commons Clause — [LICENSE](LICENSE).

> Kaynak kod serbestçe kullanılabilir, değiştirilebilir ve dağıtılabilir;
> bu yazılım **ücretli ürün veya hizmet olarak satılamaz**.

---

© 2026 Ayaz Çavdar
