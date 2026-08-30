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

hypr_config="$root/home/Eatgrapes/.config/hypr/hyprland.conf"
mkdir -p "$(dirname "$hypr_config")"
if [ ! -f "$hypr_config" ]; then
    printf '%s\n' \
        'monitor=WAYLAND-1,1600x720@60,0x0,1' \
        'env = WLR_BACKENDS,wayland' \
        'env = WLR_RENDERER,gles2' \
        'env = WLR_NO_HARDWARE_CURSORS,1' \
        'env = WLR_LIBINPUT_NO_DEVICES,1' \
        'env = XDG_CURRENT_DESKTOP,Hyprland' \
        'general { gaps_in = 4; gaps_out = 8; border_size = 2; col.active_border = rgba(9b6cffee); col.inactive_border = rgba(3b2b55aa) }' \
        'decoration { rounding = 8 }' \
        'misc { disable_hyprland_logo = true; disable_splash_rendering = true; vfr = false }' \
        'input { kb_layout = us }' \
        'bind = SUPER, RETURN, exec, foot' \
        'bind = SUPER, Q, killactive' \
        'bind = SUPER, M, exit' > "$hypr_config"
    chown -R 1001:1001 "$root/home/Eatgrapes/.config"
fi

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

if [ ! -x "$root/usr/bin/Hyprland" ]; then
    "$chroot" "$root" /usr/bin/pacman -Sy --needed --noconfirm \
        hyprland waybar foot wofi xdg-desktop-portal-hyprland wl-clipboard \
        grim slurp noto-fonts ttf-dejavu gcc libdrm
fi

hook_source=/data/data/com.termux/files/home/pvrway-prime-hook.c
if [ -f "$hook_source" ]; then
    cp "$hook_source" "$root/tmp/pvrway-prime-hook.c"
fi
if [ -f "$root/tmp/pvrway-prime-hook.c" ]; then
    "$chroot" "$root" /usr/bin/cc -shared -fPIC -O2 -I/usr/include/libdrm \
        /tmp/pvrway-prime-hook.c -ldl -ldrm -o /tmp/pvrway-prime-hook.so
    chown 1001:1001 "$root/tmp/pvrway-prime-hook.so"
else
    echo "pvrway-prime-hook.c is missing" >&2
    exit 1
fi
"$chroot" "$root" /usr/bin/dbus-uuidgen --ensure 2>/dev/null || true

am start -n io.eatgrapes.pvrway/android.app.NativeActivity >/dev/null
until [ -S "$app_files/pvrway-frame.sock" ]; do sleep 1; done

chmod 777 "$app_files"
chmod 666 "$app_files/pvrway-frame.sock"
chmod 666 /dev/ion /dev/dri/card0 /dev/dri/renderD128
mkdir -p "$root/run/pvrway-app" "$root/run/user/1001"
grep -q " $root/run/pvrway-app " /proc/mounts || mount --bind "$app_files" "$root/run/pvrway-app"
chown 1001:1001 "$root/run/user/1001"
chmod 700 "$root/run/user/1001"

while grep -q " $root/run/user/1001 " /proc/mounts; do
    umount "$root/run/user/1001" 2>/dev/null || break
done
chown 1001:1001 "$root/run/user/1001"
chmod 700 "$root/run/user/1001"

mkdir -p "$root/apex/com.android.runtime"
grep -q " $root/apex/com.android.runtime " /proc/mounts || \
    mount --bind /apex/com.android.runtime "$root/apex/com.android.runtime"

for pid in $(pidof pvrway_proxy 2>/dev/null || true); do kill "$pid" 2>/dev/null || true; done
rm -f "$root/run/user/1001/pvrway-proxy.sock" "$root/run/user/1001/pvrway-proxy.sock.lock"
"$chroot" "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c "XDG_RUNTIME_DIR=/run/user/1001 nohup $proxy >/tmp/pvrway-proxy.log 2>&1 &"

export XDG_RUNTIME_DIR=/run/user/1001
export WAYLAND_DISPLAY=pvrway-proxy.sock
for pid in $(pidof Hyprland 2>/dev/null || true); do kill "$pid" 2>/dev/null || true; done
rm -f "$root/run/user/1001/wayland-1" "$root/run/user/1001/wayland-1.lock"
"$chroot" "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c \
    "XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=pvrway-proxy.sock LD_PRELOAD=/tmp/pvrway-prime-hook.so LIBGL_ALWAYS_SOFTWARE=1 MESA_LOADER_DRIVER_OVERRIDE=llvmpipe GALLIUM_DRIVER=llvmpipe WLR_BACKENDS=wayland WLR_RENDERER=gles2 WLR_NO_HARDWARE_CURSORS=1 WLR_LIBINPUT_NO_DEVICES=1 nohup Hyprland >/tmp/hyprland.log 2>&1 &"

while [ ! -S "$root/run/user/1001/wayland-1" ]; do sleep 1; done
while :; do
    hypr_socket=$(ls -t "$root/run/user/1001/hypr"/*/.socket.sock 2>/dev/null | head -n 1 || true)
    [ -n "$hypr_socket" ] && break
    sleep 1
done
hypr_socket=${hypr_socket#"$root"}
sleep 3
printf '%s' 'output create headless PVR' | "$chroot" "$root" /system/bin/nc -U "$hypr_socket"
printf '%s' 'keyword monitor PVR,1600x720@60,0x0,1' | "$chroot" "$root" /system/bin/nc -U "$hypr_socket"
printf '%s' 'keyword monitor WAYLAND-1,1600x720@60,0x0,1,mirror,PVR' | "$chroot" "$root" /system/bin/nc -U "$hypr_socket"

"$chroot" "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c \
    "XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=wayland-1 nohup foot >/tmp/foot.log 2>&1 &"
"$chroot" "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c \
    "XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=wayland-1 nohup dbus-run-session waybar >/tmp/waybar.log 2>&1 &"
