#!/system/bin/sh
set -eu

base=/data/local/archlinux
root=/data/local/archlinux-suid
user=${1:-Eatgrapes}
term=${TERM:-xterm-256color}
chroot=/data/data/com.termux/files/usr/bin/chroot

mkdir -p "$root"
if ! grep -q " $root " /proc/mounts; then
    mount --rbind "$base" "$root"
    mount -o remount,bind,suid "$root"
fi

mkdir -p "$root/run/user/1001"
chmod 700 "$root/run/user/1001"
chown 1001:1001 "$root/run/user/1001"

if [ "$user" = root ]; then
    exec "$chroot" "$root" /usr/bin/env -i \
        HOME=/root TERM="$term" PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
        /bin/bash -l
fi

exec "$chroot" "$root" /usr/bin/env -i \
    HOME=/home/Eatgrapes USER=Eatgrapes LOGNAME=Eatgrapes TERM="$term" \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /usr/bin/su - Eatgrapes
