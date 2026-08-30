#!/system/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    exec su -c "$0 $*"
fi

root=/data/local/archlinux
app_files=/data/user/0/io.eatgrapes.pvrway/files
proxy=$root/tmp/pvrway-build/release/pvrway_proxy

am start -n io.eatgrapes.pvrway/android.app.NativeActivity >/dev/null
until [ -S "$app_files/pvrway-frame.sock" ]; do sleep 1; done

chmod 777 "$app_files"
chmod 666 "$app_files/pvrway-frame.sock"
mkdir -p "$root/run/pvrway-app" "$root/run/user/1001"
mountpoint -q "$root/run/pvrway-app" 2>/dev/null || mount --bind "$app_files" "$root/run/pvrway-app"
chmod 777 "$root/run/user/1001"

for pid in $(pidof pvrway_proxy 2>/dev/null || true); do kill "$pid" 2>/dev/null || true; done
rm -f "$root/run/user/1001/pvrway-proxy.sock" "$root/run/user/1001/pvrway-proxy.sock.lock"
/data/data/com.termux/files/usr/bin/chroot "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c "XDG_RUNTIME_DIR=/run/user/1001 nohup $proxy >/tmp/pvrway-proxy.log 2>&1 &"

export XDG_RUNTIME_DIR=/run/user/1001
export WAYLAND_DISPLAY=pvrway-proxy.sock
exec /data/data/com.termux/files/usr/bin/chroot "$root" /usr/bin/su - Eatgrapes -s /bin/bash -c "XDG_RUNTIME_DIR=/run/user/1001 WAYLAND_DISPLAY=pvrway-proxy.sock weston-terminal"
