#!/usr/bin/env bash
# Copy the parts of a running Pi's root filesystem that a cross link needs
# (libc/libm/pthread/libgcc/libstdc++/libasound, the crt objects, gcc's own
# static libs, and /usr/include) into a local sysroot.  Used for the ARMv6
# Pi Zero W, whose libraries Debian's armhf (ARMv7) cross packages cannot
# stand in for.  Usage: sysroot-from-pi.sh <ssh-host> [dest]
set -euo pipefail
host=${1:?ssh host}; dest=${2:-target/pi-sysroot}
cd "$(dirname "$0")/../../.."
gccdir=$(ssh "$host" 'ls -d /usr/lib/gcc/arm-linux-gnueabihf/[0-9]* | sort -V | tail -1')
mkdir -p "$dest/usr/lib/arm-linux-gnueabihf" "$dest$gccdir" "$dest/usr/include"
rsync -aL --info=progress2 \
  --include='*/' \
  --include='libc.so*' --include='libc_nonshared.a' --include='libm.so*' --include='libm-*.a' \
  --include='libpthread*' --include='libdl.so*' --include='librt.so*' --include='libutil.so*' \
  --include='libgcc_s.so*' --include='libstdc++.so*' --include='libasound.so*' --include='libatomic.so*' \
  --include='ld-linux-armhf.so*' --include='*crt*.o' --include='libmvec.so*' --include='libresolv.so*' \
  --exclude='*' \
  "$host:/usr/lib/arm-linux-gnueabihf/" "$dest/usr/lib/arm-linux-gnueabihf/"
# Only objects and libraries from gcc's own dir: it also holds the (ARM) LTO
# plugin and headers, which must not be seen by the host compiler via -B.
rsync -aL --info=progress2 --exclude='*/' --exclude='liblto_plugin*' --exclude='libcc1*' \
  --include='*.o' --include='*.a' --include='*.so' --include='*.so.*' --exclude='*' \
  "$host:$gccdir/" "$dest$gccdir/"
rsync -aL --info=progress2 --exclude='python*' --exclude='node*' "$host:/usr/include/" "$dest/usr/include/"
# SDL2 (renderer-sdl) and everything it pulls in: the linker resolves a shared
# library's own dependencies at link time, so they must be in the sysroot too.
sdl_libs=$(ssh "$host" 'l=/usr/lib/arm-linux-gnueabihf/libSDL2-2.0.so.0; echo $l; ldd $l | awk "/=> \//{print \$3}"')
rsync -aL --info=progress2 --files-from=<(echo "$sdl_libs") "$host:/" "$dest/"
ln -sfn libSDL2-2.0.so.0 "$dest/usr/lib/arm-linux-gnueabihf/libSDL2.so"
mkdir -p "$dest/lib" && ln -sfn ../usr/lib/arm-linux-gnueabihf "$dest/lib/arm-linux-gnueabihf"
ln -sfn arm-linux-gnueabihf/ld-linux-armhf.so.3 "$dest/lib/ld-linux-armhf.so.3"
# libasound2-dev is not on the Pi: give the linker a dev symlink + a .pc stub.
lib="$dest/usr/lib/arm-linux-gnueabihf"
ln -sfn libasound.so.2 "$lib/libasound.so"
mkdir -p "$lib/pkgconfig"
cat > "$lib/pkgconfig/alsa.pc" <<PC
prefix=/usr
libdir=\${prefix}/lib/arm-linux-gnueabihf
Name: alsa
Description: ALSA (runtime library from the Pi, no headers)
Version: 1.2.14
Libs: -L\${libdir} -lasound
PC
du -sh "$dest"; echo "sysroot at $dest (gcc libs: $gccdir)"
