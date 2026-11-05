# Pyro Porch

A lean button deck: a grid of keys that open things, press hotkeys, call APIs and more. It works
on screen and with a USB deck, and keeps running in the background. Written in Rust, for macOS,
Windows and Linux. The name is a play on a deck by a stream: a porch, with a fire.

## Status

Built and tested on macOS. It has loaded on a real USB deck, but not every deck feature has been tried on one. The Windows and Linux builds haven't been run yet: the
automatic checks in `.github/workflows` will try them once this is on GitHub. Nobody has reviewed its security
independently.

## Run it

You'll need [Rust](https://rustup.rs). The right version is pinned in `rust-toolchain.toml`, and rustup fetches it
by itself.

```sh
cargo run                   # or cargo build --release
```

On a Mac the first start assembles `Pyro Porch.app` next to the program and runs that, so the system
shows the right name and icon when asking for permissions.

On Linux, building also needs the system's development packages for USB and the window, such as
`libudev-dev`, `libxkbcommon-dev`, `libx11-dev`, `libwayland-dev` and `libgl1-mesa-dev`.

## Commands

| Command | What it does |
|---|---|
| `pyroporch` | Opens the app |
| `pyroporch mcp` | Runs the MCP server for an AI assistant, over stdin and stdout |
| `pyroporch --screenshot <file.png>` | Draws 120 frames, saves a picture of the window and exits |
| `pyroporch --view <screen>` | Starts on another screen: `plugins`, `vault`, `assistant` or `settings` |
| `pyroporch --quit` | Closes a copy running in the background |
| `pyroporch --list-decks` | Lists connected decks and their key sizes |
| `pyroporch --version` | Shows the version |
| `pyroporch --help` | Shows this list |

The config lives in the platform's config folder, or in `PYROPORCH_HOME` if that's set.

## Layout

```
crates/sdk/        The plugin kit: everything a plugin can see
crates/core/       Everything without a window: config, vault, policy, sandbox, engine, MCP server
crates/desktop/    Part of the app: open apps and links, hotkeys, media keys, typed text, commands
plugins/           The built plugins the app ships with, and the guide to writing one
  README.md        The guide
  api-call/        Built: make API calls
  screensaver/     Built: animations on the keys, started by a press or a quiet spell
  _examples/       The source code
    api-call/      The worked example for plugin authors
    screensaver/   The worked example for drawing on the keys
    build-example-plugins.sh  Rebuilds the built plugins from their source
src/               The window and the USB deck module
docs/              Guides: ACTIONS, CONFIG, MCP, SECURITY
tests/             Tests of the finished program, including that the guides match it
```

Each built plugin is a `plugin.toml` and a `plugin.wasm`, just as in any plugins folder. The app carries them, with their
source code, and puts them in its own plugins folder along with the guide, where it calls them samples, and anyone can
remove them.

Each optional part is a feature, so a smaller build is one flag away:

```sh
cargo build --release --no-default-features --features desktop-actions
```

## Guides

- [Actions](docs/ACTIONS.md): everything a key can do, and each setting
- [Config files](docs/CONFIG.md): everything the app does is described in editable files
- [AI assistant](docs/MCP.md): connecting the built-in MCP server, and every tool it offers
- [Writing a plugin](plugins/README.md): a quick start, recipes and the full reference for extending the app
- [Security](docs/SECURITY.md): how credentials are handled, and the limits of that

## Testing

```sh
cargo test --workspace
```

The tests drive the real window, talk to the real MCP server over its connection, and run real
sandboxed plugins, including hostile ones. They also compile every example in the plugin guide, build and run
its quick start in the sandbox, and check the guides against what the program really offers. The sandbox tests need
`rustup target add wasm32-unknown-unknown`, and skip themselves without it.

## USB decks

Close the deck maker's own app first, because only one program can hold the device. On Linux you
may need a udev rule to reach the device without root; the `elgato-streamdeck` crate's README has one.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option. Pyro Porch is an
independent project, and isn't affiliated with or endorsed by any maker of decks.
