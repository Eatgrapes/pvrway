#!/system/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    exec su -c "$0 $*"
fi

base=/data/local/archlinux
root=/data/local/archlinux-suid
app_files=/data/user/0/io.eatgrapes.pvrway/files
proxy=/tmp/pvrway-build/release/pvrway_proxy
chroot=/data/data/com.termux/files/usr/bin/chroot

mkdir -p "$root"
if ! grep -q " $root " /proc/mounts; then
    mount --rbind "$base" "$root"
    mount -o remount,bind,suid "$root"
fi

printf '%s\n' 'root:123456' | "$chroot" "$root" /usr/bin/chpasswd
mkdir -p "$root/etc/sudoers.d"
printf '%s\n' 'Eatgrapes ALL=(ALL:ALL) ALL' > "$root/etc/sudoers.d/eatgrapes"
chmod 440 "$root/etc/sudoers.d/eatgrapes"

if [ ! -f "$root/var/lib/pvrway/base-tools" ]; then
    mkdir -p "$root/var/lib/pvrway"
    if "$chroot" "$root" /usr/bin/pacman -Sy --needed --noconfirm \
        sudo fish coreutils findutils grep sed gawk procps-ng iproute2 \
        less nano bash-completion util-linux psmisc; then
        "$chroot" "$root" /usr/bin/usermod -aG wheel Eatgrapes 2>/dev/null || true
        touch "$root/var/lib/pvrway/base-tools"
    else
        log_file="$root/tmp/pvrway-pacman.log"
        printf '%s\n' 'Arch base tools installation failed; run pacman manually.' > "$log_file"
    fi
fi

am start -n io.eatgrapes.pvrway/android.app.NativeActivity >/dev/null
until [ -S "$app_files/pvrway-frame.sock" ]; do sleep 1; done

chmod 777 "$app_files"
chmod 666 "$app_files/pvrway-frame.sock"
mkdir -p "$root/run/pvrway-app" "$root/run/user/1001"
grep -q " $root/run/pvrway-app " /proc/mounts || mount --bind "$app_files" "$root/run/pvrway-app"
chmod 777 "$root/run/user/1001"

while grep -q " $root/run/user/1001 " /proc/mounts; do
    umount "$root/run/user/1001" 2>/dev/null || break
done
chmod 777 "$root/run/user/1001"

mkdir -p "$root/apex/com.android.runtime"
grep -q " $root/apex/com.android.runtime " /proc/mounts || \
    mount --bind /apex/com.android.runtime "$root/apex/com.android.runtime"

for pid in $(pidof pvrway_proxy 2>/dev/null || true); do kill "$pid" 2>/dev/null || true; done
rm -f "$root/run/user/1001/pvrway-proxy.sock" "$root/run/user/1001/pvrway-proxy.sock.lock"
"$chroot" "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c "XDG_RUNTIME_DIR=/run/user/1001 nohup $proxy >/tmp/pvrway-proxy.log 2>&1 &"

export XDG_RUNTIME_DIR=/run/user/1001
export WAYLAND_DISPLAY=pvrway-proxy.sock
exec "$chroot" "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c "XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=pvrway-proxy.sock weston-terminal"
