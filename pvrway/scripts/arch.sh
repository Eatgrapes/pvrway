#!/data/data/com.termux/files/usr/bin/sh
set -eu

launcher=/data/local/archlinux/launch
if [ "${1:-}" = root ]; then
    exec su -c "$launcher root"
fi
exec su -c "$launcher Eatgrapes"
