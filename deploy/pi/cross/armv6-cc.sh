#!/bin/sh
# gcc front-end for ARMv6 (Pi Zero W / Pi 1): Debian's cross compiler, the
# Pi's own sysroot and gcc libdir so no ARMv7 object ever reaches the link.
SYSROOT=${SYSROOT:-/work/target/pi-sysroot}
GCCDIR=$(ls -d "$SYSROOT"/usr/lib/gcc/arm-linux-gnueabihf/[0-9]* | sort -V | tail -1)
LIBDIR="$SYSROOT/usr/lib/arm-linux-gnueabihf"
# Both -B prefixes go ahead of the compiler's own (ARMv7) /usr/arm-linux-gnueabihf/lib
# for start files AND -l lookups, so crt1.o, libc_nonshared.a, libgcc.a are the Pi's.
exec arm-linux-gnueabihf-gcc --sysroot="$SYSROOT" -B"$GCCDIR/" -B"$LIBDIR/" \
  -march=armv6+fp -mfpu=vfp -mfloat-abi=hard -marm \
  -L"$GCCDIR" -L"$LIBDIR" \
  -Wl,-rpath-link,"$LIBDIR" "$@"
