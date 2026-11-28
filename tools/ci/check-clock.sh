#!/bin/sh
# The clock comes from opendeck_types::time, never std::time: std's Instant
# and SystemTime panic in a browser. crates/types/src/time.rs is the one
# file allowed to name them. (Clippy can't police this: natively our
# Instant IS std's, so it flags the shim as well.)
set -e
cd "$(git rev-parse --show-toplevel)"
hits=$(git grep -nE \
    -e 'std::time::(Instant|SystemTime|UNIX_EPOCH|\*)' \
    -e 'std::time::\{[^}]*(Instant|SystemTime|UNIX_EPOCH)' \
    -e '^[[:space:]]*time::\{?[^};]*(Instant|SystemTime|UNIX_EPOCH)' \
    -e 'use std::\{[^;]*time::\{?[^};]*(Instant|SystemTime|UNIX_EPOCH)' \
    -- 'crates/**/*.rs' ':!crates/types/src/time.rs' || true)
if [ -n "$hits" ]; then
    echo "std's clock used outside crates/types/src/time.rs; use opendeck_types::time instead:"
    echo "$hits"
    exit 1
fi
echo "clock: ok"
