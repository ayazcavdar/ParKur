# ParKur — Windows için Linux Kurulum Aracı

Pardus/Debian **canlı** ISO’larını Windows’a **bölümleme yapmadan**, NTFS’teki tek bir
`root.disk` dosyasına kurar (loop-mount). Tauri v2.

Önyükleme / disk dosyaları: **NextOS**. Uygulama: **ParKur**.

[English](README.md) · [Kurulum videosu](video.mp4)

## Akış

1. Yerel ISO seçin veya uygulama içinden Pardus indirin (XFCE / GNOME)  
2. Kullanıcı, parola, bilgisayar adı, dil, saat dilimi  
3. NTFS sürücü ve `root.disk` boyutu  
4. Kurulum — bölümler değişmez / biçimlenmez  
5. Yeniden başlat → **NextOS**; ilk açılış otomatik yapılandırır  

Arayüz: Türkçe / İngilizce.

## Gereksinimler

- Windows 10/11, **UEFI**, yönetici  
- Yeterli NTFS boş alan; hedefte BitLocker yok; Hızlı Başlatma kapalı  

## Derleme

```bash
npm install
npx tauri build
```

Çıktı: `src-tauri/target/release/bundle/`

## Lisans

Apache 2.0 + Commons Clause — [LICENSE](LICENSE).  
Ücretli ürün/hizmet olarak satılamaz.

© 2026 Ayaz Çavdar
