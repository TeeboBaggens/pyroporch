#!/bin/sh
# Rebuilds the example plugins from their source here, and puts each built plugin.wasm in its folder
# under plugins/. Run this after changing one of them. With --check, it only says whether those files
# are up to date. The guide in plugins/README.md covers building your own.
set -e
cd "$(dirname "$0")/../.."

# A built file records where its source was. These keep anyone's folder names out of it, and make the
# same source build the same file on any computer.
sysroot="$(rustc --print sysroot)"
registry="${CARGO_HOME:-$HOME/.cargo}/registry/src"
CARGO_ENCODED_RUSTFLAGS="$(printf '%s\037%s' "--remap-path-prefix=$sysroot/lib/rustlib/src/rust=/rust" "--remap-path-prefix=$registry=/registry")"
export CARGO_ENCODED_RUSTFLAGS

cargo build --release --target wasm32-unknown-unknown --target-dir target/example-plugins \
    -p pyroporch-plugin-api-call -p pyroporch-plugin-screensaver

for plugin in api-call screensaver; do
    built="target/example-plugins/wasm32-unknown-unknown/release/pyroporch_plugin_$(echo "$plugin" | tr - _).wasm"
    if [ "$1" = "--check" ]; then
        cmp -s "$built" "plugins/$plugin/plugin.wasm" || {
            echo "$plugin is out of date: run plugins/_examples/build-example-plugins.sh" >&2
            exit 1
        }
    else
        cp "$built" "plugins/$plugin/plugin.wasm"
    fi
done
ls -l plugins/*/plugin.wasm
