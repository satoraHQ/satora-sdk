#!/bin/sh
# Check production dependencies for the rustls profile. Pass a Cargo target
# triple as the first argument to check a different native platform.
set -eu
cd "$(dirname "$0")/.."

if [ "$#" -gt 1 ]; then
    echo "usage: $0 [target-triple]" >&2
    exit 2
fi
if [ "$#" -eq 1 ]; then
    set -- --target "$1"
fi

graph=$(cargo tree --locked --no-default-features --features rustls-tls,gasless,websocket \
    --edges normal,build --prefix none "$@")
if printf '%s\n' "$graph" | grep -E '^(native-tls|tokio-native-tls|openssl|openssl-sys|openssl-src|openssl-macros) v'; then
    echo "The rustls profile resolved a native TLS or OpenSSL dependency." >&2
    exit 1
fi
