#!/bin/sh

set -ex

MSRV="1.23.0"

cargo build --verbose

# Give up testing on MSRV since our dev-dependencies no longer support it.
if [ "$TRAVIS_RUST_VERSION" = "$MSRV" ]; then
    exit
fi

cargo build --verbose --all
cargo doc --verbose
cargo test --verbose
