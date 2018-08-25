#!/bin/sh

set -ex

cargo doc --verbose
cargo build --verbose
cargo test --verbose

if [ "$TRAVIS_RUST_VERSION" = "nightly" ]; then
  cargo +nightly generate-lockfile -Z minimal-versions
  cargo build --verbose
  cargo test --verbose
fi
