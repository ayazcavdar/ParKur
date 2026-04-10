#!/bin/bash
# =============================================================================
# ParKur Otonom Kurulum Motoru
# Headless/unattended Pardus kurulumu
# =============================================================================
set -euo pipefail
export PATH="/usr/sbin:/usr/bin:/sbin:/bin"
export LANG=C.UTF-8
export DEBIAN_FRONTEND=noninteractive

LOG="/var/log/parkur-install.log"
CONF_LABEL="NextOS_Install"
MNT_DATA="/mnt/parkur-data"
MNT_TARGET="/mnt/target"
STAMP="/opt/parkur/.completed"

log() { echo "[$(date '+%H:%M:%S')] $*" | tee -a "$LOG"; }
die() { log "FATAL: $*"; tiered_reboot; }

# ── Üç Katmanlı Pürüzsüz Reboot ──
tiered_reboot() {
    log "Reboot başlatılıyor (3 katmanlı)..."
    sync

    # Katman 1: Temiz systemd reboot
    systemctl reboot 2>/dev/null || true
    sleep 12

    # Katman 2: Zorla kernel reboot
    log "Katman 2: reboot -f"
    sync
    reboot -f 2>/dev/null || true
    sleep 5

    # Katman 3: SysRq donanım tetiklemesi
    log "Katman 3: SysRq"
    sync
    echo 1 > /proc/sys/kernel/sysrq 2>/dev/null || true
    echo s > /proc/sysrq-trigger 2>/dev/null || true
    echo u > /proc/sysrq-trigger 2>/dev/null || true
    echo b > /proc/sysrq-trigger 2>/dev/null || true
}

# ── Trap: Herhangi bir hata olursa reboot ──
trap 'die "Beklenmeyen hata (satır $LINENO)"' ERR

# ────────────────────────────────────────────
# 0. Zaten tamamlandıysa atla
# ────────────────────────────────────────────
if [ -f "$STAMP" ]; then
    log "Kurulum zaten tamamlanmış, reboot."
    tiered_reboot
fi

log "=== ParKur Otonom Kurulum Başladı ==="

# ────────────────────────────────────────────
# 1. NTFS veri bölümünü bul ve config oku
# ────────────────────────────────────────────
log "Adım 1: Veri bölümü aranıyor (etiket: $CONF_LABEL)..."
DATA_DEV=""
for i in $(seq 1 10); do
    DATA_DEV=$(blkid -L "$CONF_LABEL" 2>/dev/null || true)
    [ -n "$DATA_DEV" ] && break
    log "  Bekleniyor... ($i/10)"
    sleep 2
done
[ -z "$DATA_DEV" ] && die "Veri bölümü bulunamadı (etiket: $CONF_LABEL)"
log "  Bulundu: $DATA_DEV"

mkdir -p "$MNT_DATA"
# ntfs-3g ile mount (live ortamda mevcut)
mount -t ntfs-3g -o ro "$DATA_DEV" "$MNT_DATA" 2>/dev/null \
    || mount -o ro "$DATA_DEV" "$MNT_DATA" \
    || die "Veri bölümü mount edilemedi"

CONF_FILE="$MNT_DATA/parkur.conf"
[ -f "$CONF_FILE" ] || die "parkur.conf bulunamadı"

# JSON parse (jq yoksa python3 fallback)
if command -v jq &>/dev/null; then
    USERNAME=$(jq -r '.username' "$CONF_FILE")
    PASSWORD_HASH=$(jq -r '.password_hash' "$CONF_FILE")
    HOSTNAME_VAL=$(jq -r '.hostname' "$CONF_FILE")
    LOCALE_VAL=$(jq -r '.locale' "$CONF_FILE")
    TIMEZONE_VAL=$(jq -r '.timezone' "$CONF_FILE")
    KEYBOARD_VAL=$(jq -r '.keyboard' "$CONF_FILE")
else
    USERNAME=$(python3 -c "import json;print(json.load(open('$CONF_FILE'))['username'])")
    PASSWORD_HASH=$(python3 -c "import json;print(json.load(open('$CONF_FILE'))['password_hash'])")
    HOSTNAME_VAL=$(python3 -c "import json;print(json.load(open('$CONF_FILE'))['hostname'])")
    LOCALE_VAL=$(python3 -c "import json;print(json.load(open('$CONF_FILE'))['locale'])")
    TIMEZONE_VAL=$(python3 -c "import json;print(json.load(open('$CONF_FILE'))['timezone'])")
    KEYBOARD_VAL=$(python3 -c "import json;print(json.load(open('$CONF_FILE'))['keyboard'])")
fi

log "  Kullanıcı: $USERNAME / Host: $HOSTNAME_VAL"
umount "$MNT_DATA" 2>/dev/null || true

# ────────────────────────────────────────────
# 2. Hedef bölümü ext4 olarak hazırla
# ────────────────────────────────────────────
log "Adım 2: $DATA_DEV → ext4 formatlanıyor..."
wipefs -a "$DATA_DEV" || true
mkfs.ext4 -F -L "pardus-root" "$DATA_DEV"
mkdir -p "$MNT_TARGET"
mount "$DATA_DEV" "$MNT_TARGET"
log "  ext4 mount edildi: $MNT_TARGET"

# ────────────────────────────────────────────
# 3. squashfs → hedef disk
# ────────────────────────────────────────────
log "Adım 3: squashfs açılıyor..."
SQUASH=""
# Live ortamda squashfs mount noktaları
for candidate in \
    /lib/live/mount/medium/live/filesystem.squashfs \
    /lib/live/mount/rootfs/filesystem.squashfs \
    /run/live/medium/live/filesystem.squashfs \
    /cdrom/live/filesystem.squashfs \
    /cdrom/casper/filesystem.squashfs; do
    [ -f "$candidate" ] && SQUASH="$candidate" && break
done

# Bulunamazsa find ile ara
if [ -z "$SQUASH" ]; then
    SQUASH=$(find /lib/live /run/live /cdrom 2>/dev/null -name "filesystem.squashfs" -print -quit || true)
fi
[ -z "$SQUASH" ] && die "filesystem.squashfs bulunamadı"
log "  squashfs: $SQUASH"

unsquashfs -f -d "$MNT_TARGET" "$SQUASH"
log "  unsquashfs tamamlandı"

# ────────────────────────────────────────────
# 4. Chroot yapılandırması
# ────────────────────────────────────────────
log "Adım 4: Chroot yapılandırması başlıyor..."

# Bind mount
log "  Bind mount yapılıyor..."
mount --bind /dev      "$MNT_TARGET/dev"
mount --bind /dev/pts  "$MNT_TARGET/dev/pts"
mount --bind /proc     "$MNT_TARGET/proc"
mount --bind /sys      "$MNT_TARGET/sys"
mount --bind /run      "$MNT_TARGET/run"

# DNS
cp /etc/resolv.conf "$MNT_TARGET/etc/resolv.conf" 2>/dev/null || true

# ESP bölümünü bul
log "  ESP bölümü aranıyor..."
ESP_DEV=$(blkid -t PARTLABEL="EFI System Partition" -o device 2>/dev/null | head -1 || true)
if [ -z "$ESP_DEV" ]; then
    ESP_DEV=$(lsblk -rno NAME,PARTTYPE | grep -i "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" | awk '{print "/dev/"$1}' | head -1 || true)
fi
log "  ESP: ${ESP_DEV:-bulunamadı}"

# fstab oluştur
log "  fstab oluşturuluyor..."
TARGET_UUID=$(blkid -s UUID -o value "$DATA_DEV")
cat > "$MNT_TARGET/etc/fstab" <<FSTAB
# ParKur tarafından oluşturuldu
UUID=$TARGET_UUID  /         ext4  errors=remount-ro  0  1
FSTAB

if [ -n "$ESP_DEV" ]; then
    ESP_UUID=$(blkid -s UUID -o value "$ESP_DEV")
    echo "UUID=$ESP_UUID  /boot/efi  vfat  umask=0077  0  2" >> "$MNT_TARGET/etc/fstab"
fi

# ── Chroot içinde servis başlatmayı engelle (hang önleme) ──
log "  policy-rc.d oluşturuluyor (servis restart engeli)..."
cat > "$MNT_TARGET/usr/sbin/policy-rc.d" <<'POLICYEOF'
#!/bin/sh
exit 101
POLICYEOF
chmod +x "$MNT_TARGET/usr/sbin/policy-rc.d"

# ── Chroot script ──
log "  Chroot script oluşturuluyor..."
cat > "$MNT_TARGET/tmp/parkur-chroot.sh" <<'CHROOTEOF'
#!/bin/bash
set -euo pipefail
export PATH="/usr/sbin:/usr/bin:/sbin:/bin"
export DEBIAN_FRONTEND=noninteractive
export DEBCONF_NONINTERACTIVE_SEEN=true

USERNAME="$1"
PASSWORD_HASH="$2"
HOSTNAME_VAL="$3"
LOCALE_VAL="$4"
TIMEZONE_VAL="$5"
KEYBOARD_VAL="$6"
ESP_DEV="$7"

# dpkg interaktif prompt'ları tamamen devre dışı bırak
DPKG_OPTS="-o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold"

echo "[CHROOT] 1/10 Kullanıcı oluşturuluyor: $USERNAME"
if ! id "$USERNAME" &>/dev/null; then
    useradd -m -s /bin/bash -G sudo "$USERNAME"
fi
echo "${USERNAME}:${PASSWORD_HASH}" | chpasswd -e

# root şifresini kilitle (güvenlik)
passwd -l root 2>/dev/null || true

echo "[CHROOT] 2/10 Hostname ayarlanıyor: $HOSTNAME_VAL"
echo "$HOSTNAME_VAL" > /etc/hostname
cat > /etc/hosts <<EOF
127.0.0.1   localhost
127.0.1.1   $HOSTNAME_VAL
::1         localhost ip6-localhost ip6-loopback
EOF

echo "[CHROOT] 3/10 Locale ayarlanıyor: $LOCALE_VAL"
if [ -f /etc/locale.gen ]; then
    sed -i "s/^# *\(${LOCALE_VAL}\)/\1/" /etc/locale.gen 2>/dev/null || true
    locale-gen 2>/dev/null || true
fi
echo "LANG=${LOCALE_VAL}" > /etc/default/locale 2>/dev/null || true

echo "[CHROOT] 4/10 Timezone ayarlanıyor: $TIMEZONE_VAL"
ln -sf "/usr/share/zoneinfo/${TIMEZONE_VAL}" /etc/localtime
echo "$TIMEZONE_VAL" > /etc/timezone 2>/dev/null || true

echo "[CHROOT] 5/10 Klavye ayarlanıyor: $KEYBOARD_VAL"
mkdir -p /etc/default
cat > /etc/default/keyboard <<EOF
XKBMODEL="pc105"
XKBLAYOUT="${KEYBOARD_VAL}"
XKBVARIANT=""
XKBOPTIONS=""
BACKSPACE="guess"
EOF

echo "[CHROOT] 6/10 Live-ortam paketleri kaldırılıyor..."
apt-get purge -y $DPKG_OPTS live-boot live-config live-tools 2>/dev/null || true
apt-get purge -y $DPKG_OPTS calamares 'calamares-*' 2>/dev/null || true
apt-get autoremove -y $DPKG_OPTS 2>/dev/null || true

echo "[CHROOT] 7/10 GRUB kuruluyor..."
if [ -n "$ESP_DEV" ] && [ "$ESP_DEV" != "none" ]; then
    mkdir -p /boot/efi
    mount "$ESP_DEV" /boot/efi 2>/dev/null || true

    # Eski NextOS GRUB artıklarını temizle
    rm -rf /boot/efi/EFI/NextOS 2>/dev/null || true

    # os-prober'ı devre dışı bırak (disk taramasında hang olmasın)
    if [ -x /usr/bin/os-prober ]; then
        chmod -x /usr/bin/os-prober 2>/dev/null || true
    fi

    grub-install --target=x86_64-efi \
                 --efi-directory=/boot/efi \
                 --bootloader-id=pardus \
                 --recheck 2>/dev/null || true

    echo "[CHROOT] 7b/10 update-grub çalıştırılıyor (os-prober devre dışı)..."
    update-grub 2>/dev/null || true

    # os-prober'ı geri aç (kalıcı sistem için)
    if [ -f /usr/bin/os-prober ]; then
        chmod +x /usr/bin/os-prober 2>/dev/null || true
    fi

    umount /boot/efi 2>/dev/null || true
fi

echo "[CHROOT] 8/10 Initramfs güncelleniyor..."
timeout 120 update-initramfs -u -k all 2>/dev/null || true

echo "[CHROOT] 9/10 Sudoers ayarlanıyor..."
if [ -d /etc/sudoers.d ]; then
    echo "${USERNAME} ALL=(ALL:ALL) ALL" > "/etc/sudoers.d/90-${USERNAME}"
    chmod 440 "/etc/sudoers.d/90-${USERNAME}"
fi

echo "[CHROOT] 10/10 Chroot yapılandırması tamamlandı."
CHROOTEOF

chmod +x "$MNT_TARGET/tmp/parkur-chroot.sh"

log "  Chroot script çalıştırılıyor..."
chroot "$MNT_TARGET" /tmp/parkur-chroot.sh \
    "$USERNAME" "$PASSWORD_HASH" "$HOSTNAME_VAL" \
    "$LOCALE_VAL" "$TIMEZONE_VAL" "$KEYBOARD_VAL" \
    "${ESP_DEV:-none}"

log "  Chroot yapılandırması tamamlandı"

# ── policy-rc.d'yi temizle (kalıcı sistemde servisler normal çalışsın) ──
rm -f "$MNT_TARGET/usr/sbin/policy-rc.d"

# ────────────────────────────────────────────
# 5. Temizlik
# ────────────────────────────────────────────
log "Adım 5: Temizlik..."

rm -f "$MNT_TARGET/tmp/parkur-chroot.sh"
rm -f "$MNT_TARGET/opt/parkur/parkur.conf" 2>/dev/null || true

# Bind unmount (ters sıra)
for mp in run sys proc dev/pts dev; do
    umount "$MNT_TARGET/$mp" 2>/dev/null || umount -l "$MNT_TARGET/$mp" 2>/dev/null || true
done

umount "$MNT_TARGET" 2>/dev/null || umount -l "$MNT_TARGET" 2>/dev/null || true
sync

# Başarı işareti
mkdir -p /opt/parkur
touch "$STAMP"

log "=== ParKur Kurulum Başarıyla Tamamlandı ==="

# ────────────────────────────────────────────
# 6. Pürüzsüz Reboot
# ────────────────────────────────────────────
tiered_reboot
