#!/bin/sh

set -ex

MSRV="1.23.0"

# If we're building on 1.23, then lazy_static 1.2 will fail to build since it
# updated its MSRV to 1.24.1. In this case, we force the use of lazy_static 1.1
# to build on Rust 1.23.0.
if [ "$TRAVIS_RUST_VERSION" = "$MSRV" ]; then
    cargo update -p lazy_static --precise 1.1.0
    # On older versions of Cargo, this apparently needs to be run twice
    # if Cargo.lock didn't previously exist. Since this command should be
    # idempotent, we run it again unconditionally.
    cargo update -p lazy_static --precise 1.1.0
fi

cargo doc --verbose
cargo build --verbose
cargo test --verbose

if [ "$TRAVIS_RUST_VERSION" = "nightly" ]; then
  cargo +nightly generate-lockfile -Z minimal-versions
  cargo build --verbose
  cargo test --verbose
fi
