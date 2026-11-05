# Writing a plugin

A plugin adds new actions that keys can use. It's one small file of WebAssembly that runs in a sandbox,
so the same file works on macOS, Windows and Linux, and it can only do what the person using it has
allowed.

This guide starts with a working plugin, then covers everything in full. Two sample plugins come with
the app, and each has its source code in its folder: **API Calls** shows calling a site with a secret,
and **Screen Saver** shows drawing on the keys. They use nothing but what's described here.

**Contents:** [Quick start](#quick-start-a-dice-key) · [The plugins folder](#the-plugins-folder) ·
[How a plugin runs](#how-a-plugin-runs) ·
[Describing actions](#describing-actions) · [Running an action](#running-an-action) ·
[Calling a site](#calling-a-site) · [Secrets](#secrets) · [Recipes](#recipes) ·
[`plugin.toml`](#plugintoml) · [Limits](#limits) · [Testing](#testing-without-the-app) ·
[Troubleshooting](#troubleshooting) · [Sharing](#sharing-a-plugin) ·
[Other languages](#writing-a-plugin-in-another-language) ·
[Keys that act on the computer](#keys-that-act-on-the-computer) · [Changing the samples](#changing-the-samples)

## Quick start: a dice key

This takes about ten minutes, and needs no network or secrets. You'll need [Rust](https://rustup.rs),
current stable (this project is built with 1.99.1), its WebAssembly target, and a copy of this project,
which holds the plugin kit in `crates/sdk`. The kit isn't published on crates.io yet.

```sh
rustup target add wasm32-unknown-unknown
cargo new --lib dice
```

**1. Set up the crate.** Replace `dice/Cargo.toml` with this, pointing `path` at your copy of the kit:

<!-- guide-example: Cargo.toml -->
```toml
[package]
name = "dice"
version = "1.0.0"
edition = "2024"

[lib]
# cdylib builds the .wasm file. rlib lets `cargo test` run on your own computer.
crate-type = ["cdylib", "rlib"]

[dependencies]
pyroporch-sdk = { path = "../pyroporch/crates/sdk" }

# A smaller file, which also loads faster.
[profile.release]
opt-level = "s"
lto = true
strip = true
```

**2. Write the plugin.** Replace `dice/src/lib.rs` with this:

<!-- guide-example: lib.rs -->
```rust
use pyroporch_sdk::*;

pub struct Dice;

impl Plugin for Dice {
    // Describes the action. The settings form, the config checker and the AI assistant tools are
    // all built from this, so there's no UI code to write.
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: "com.example.dice".into(),
            name: "Dice".into(),
            version: "1.0.0".into(),
            description: "Rolls a die and shows the result on the key.".into(),
            actions: vec![ActionSpec {
                id: "roll".into(),
                name: "Roll a Die".into(),
                description: "Shows a random number on the key.".into(),
                icon: "🎲".into(),
                fields: vec![Field::new("sides", "Sides", FieldKind::Number).default(6)],
                ..ActionSpec::default()
            }],
        }
    }

    // Runs when the key is pressed.
    fn run(&self, request: &RunRequest, host: &HostHandle) -> Result<Outcome, String> {
        let sides = request.number("sides").unwrap_or(6.0).max(2.0) as u64;
        // The sandbox has no source of randomness, so the clock stands in for one.
        let roll = host.now_ms() % sides + 1;
        Ok(Outcome {
            title: Some(roll.to_string()),
            message: format!("rolled {roll}"),
            ..Outcome::default()
        })
    }
}

// Makes the crate loadable as a sandboxed plugin.
export_plugin!(Dice);
```

**3. Build it.** From the `dice` folder:

```sh
cargo build --release --target wasm32-unknown-unknown
```

The result is `target/wasm32-unknown-unknown/release/dice.wasm`. A dash in a crate's name becomes an
underscore in the file's name.

**4. Say what it is and what it asks for.** Make a file called `plugin.toml`:

<!-- guide-example: plugin.toml -->
```toml
id = "com.example.dice"
name = "Dice"
version = "1.0.0"
description = "Rolls a die and shows the result on the key."
wasm = "dice.wasm"

[permissions]
http_hosts = []
secrets = []
deck = false
```

**5. Install it.** In the app, go to the Plugins screen and choose **Open the plugins folder**. Make a
folder called `dice` in there, and put `plugin.toml` and `dice.wasm` in it. Back on the Plugins screen,
choose **Refresh**.

**6. Approve it and use it.** Dice appears, with what it asks for listed. Choose **Approve and turn on**.
*Roll a Die* is now in the action list, so drag it onto a key and press the key.

To change the plugin later, build it, copy the new `.wasm` over the old one, choose **Refresh**, and
approve it again. The approval is tied to the file's fingerprint, so every new build needs one.

## The plugins folder

The app looks in its own plugins folder, which the Plugins screen can open. Each plugin is one folder in there:

```text
plugins/
  README.md         this guide, which the app keeps up to date
  api-call/
    plugin.toml     what it is, and what it asks for
    plugin.wasm     the built plugin
    source/lib.rs   its source code, to learn from
    README.txt
  screensaver/
    ...
```

The app puts the samples there the first time it starts, already approved. They're ordinary plugins from then on:
remove one and it stays removed, until **Restore the sample plugins**. Only `plugin.toml` and the `.wasm` file are
needed to run a plugin. Anything else in a plugin's folder is left alone, and a folder whose name starts with `.` or `_`
is ignored, which is a handy way to set one aside.

This project's source code is laid out the same way, with the source of its plugins set aside:

```text
plugins/
  README.md
  api-call/         plugin.toml and plugin.wasm, built
  screensaver/      plugin.toml and plugin.wasm, built
  _examples/
    api-call/       Cargo.toml and src/
    screensaver/    Cargo.toml and src/
    build-example-plugins.sh
```

## How a plugin runs

A plugin is one type that implements the `Plugin` trait, with up to three functions:

| Function | When it's called | Can reach the app? |
|---|---|---|
| `info()` | Once, when the plugin is loaded | No |
| `run()` | Each time a key using one of its actions is pressed, or refreshed by its timer | Yes, through `host` |
| `paint()` | Many times a second while it's drawing on the keys. Optional | No |

- **Nothing runs until it's approved.** A new plugin is listed, but none of its code runs until the person
  approves it.
- **One call at a time.** Calls to one plugin wait for each other, so a slow `run()` holds up its `paint()`.
  Other plugins aren't affected.
- **Memory lasts between calls, but not for ever.** Keep state in a `Mutex` if you need it, as the Screen
  Saver sample does. It's gone when the plugin is reloaded, which also happens when the app starts or the
  person chooses Refresh, and an assistant's connection has a copy of its own. There's no way to save
  files, so anything that has to last belongs in the key's settings.
- **The type is built once,** by the expression you give `export_plugin!`. For a type holding state, write
  `export_plugin!(MyPlugin::default());`.

## Describing actions

### `PluginInfo`

| Field | What it's for |
|---|---|
| `id` | Your plugin's unique id, in reverse-domain form, like `com.example.dice`. It must match `plugin.toml`, and can't contain a slash or a space. Ids of built-in modules and of other plugins are taken, and `core.` and `pyroporch.` are the app's own |
| `name` | Shown in the app, and as the group's heading in the action list |
| `version` | Your version, as text |
| `description` | One sentence, shown on the Plugins screen |
| `actions` | What keys can do with it |

### `ActionSpec`

Only `id` and `name` are essential. `..ActionSpec::default()` fills in the rest.

| Field | Default | What it's for |
|---|---|---|
| `id` | | Unique inside the plugin, with no slash. Keys store `plugin_id/action_id`, so changing it breaks keys already using it |
| `name` | | Shown in the action list |
| `description` | empty | Shown in the action list, and read by assistants deciding what to use |
| `icon` | empty | One character or emoji, used on the key at first |
| `fields` | none | The settings form |
| `refreshable` | `false` | The key can re-run on a timer. See [Showing a live value](#showing-a-live-value) |
| `hidden` | `false` | Leaves it off the action list while keys already using it keep working. Use it to retire an action |
| `previews` | `false` | Adds a preview button to the settings form, for an action that draws |
| `risk` | `Low` | Has no effect in a plugin. The sandbox treats every plugin as low risk, because it can only do what it's been allowed |

### Fields

Each field is one setting in the form. `Field::new(key, label, kind)` makes one, and these add to it:

| Method | What it does |
|---|---|
| `.help("...")` | A line of explanation under the field |
| `.default(value)` | What the form starts with |
| `.required()` | The key won't run, and an assistant can't save it, until this has a value |
| `.visible_when("other_key", "value")` | Only shows the field when another field has this value |
| `.hidden_when("other_key", "value")` | Hides the field when another field has this value, or starts out with it |

| `FieldKind` | The form shows | The setting holds | Read it with |
|---|---|---|---|
| `Text` | One line | Text | `request.text("key")` |
| `MultiLine` | Several lines | Text | `request.text("key")` |
| `Number` | A number | A number | `request.number("key")` |
| `Bool` | A check box | True or false | `request.flag("key")` |
| `Url` | An address | Text. For a link to open, rather than an address the plugin calls | `request.text("key")` |
| `RequestUrl` | An address | Text. For an address the plugin calls: the app asks the person about its site as soon as the key is set up | `request.text("key")` |
| `Select { options }` | A list | The chosen option, as text | `request.text("key")` |
| `Secret` | A masked field that stores into the vault | Only ever a `{{secret:name}}` reference | `request.text("key")` |
| `Hotkey` | A key recorder | Text, like `ctrl+shift+m` | `request.text("key")` |
| `KeyValues` | Rows of names and values | Name and value pairs. Values can hold references | `request.pairs("key")` |
| `LocalPath { folders }` | A text box with browse buttons, one for folders only if `folders` is true | Text, the path | `request.text("key")` |
| `Rules` | Rows of a test, a value, an icon and two colours | A list of `Rule`s. Use `rule.matches(value)` to test one | `request.rules("key")?` |

Three things to allow for:

- **A setting can be missing.** Defaults fill the form, but a key written by hand or by an assistant might not
  have them, so give every read a fallback, as `unwrap_or(6.0)` does in the quick start.
- **Don't mark a field `.required()` if it's also `.visible_when(...)`.** It would stop the key running even
  while the form is hiding it.
- **Credentials are refused in plain text.** The app won't save or run a key holding one, in a `Secret` field
  or in a setting or header with a sensitive name such as `password`, `token` or `api_key`. So use a
  `Secret` field for them.

## Running an action

### What `run()` gets

| `RunRequest` | What it holds |
|---|---|
| `action` | Your action's `id`, so one `run()` can serve several actions |
| `settings` | The key's settings |
| `trigger` | `Trigger::Press`, or `Trigger::Refresh` when a timer re-ran it |

### What it gives back

Return `Err("reason")` for a failure: the reason shows on the key and in the activity list, and if the key
has several steps, the rest don't run. Write it for the person reading it. A successful run returns an
`Outcome`, where everything is optional:

| `Outcome` | What it does |
|---|---|
| `message` | A short line for the activity list. `Outcome::message("done")` makes one with only this |
| `title` | New text for the key, for live values. If several steps set one, the last wins |
| `icon` | A character or emoji for the key to show instead of its own, until its next run. Anything like a file path is ignored |
| `icon_color` | A colour like `#FFFFFF` for the key's icon, until its next run |
| `color` | A colour like `#2E8B57` for the key to show instead of its own, until its next run |
| `navigate` | Moves the deck: `Navigate::NextPage`, `PreviousPage`, `Page("name")` or `Profile("id")` |
| `deck` | Changes the deck: `DeckControl::Sleep`, `SetBrightness(percent)`, `Brighter(step)`, `Dimmer(step)` or `ScreenSaver(timing)`. Ignored unless the plugin was approved with `deck = true` |

### What the host offers

`host` is the plugin's one way out, and each call is checked against what the person allowed:

| Call | What it does |
|---|---|
| `host.http(&request)` | Sends a web request. See below |
| `host.log(level, "text")` | `LogLevel::Warn` and `Error` go to the app's `audit.log`, under your plugin's id. `Info` isn't kept |
| `host.now_ms()` | The time, in milliseconds since 1970 |

## Calling a site

```rust
use pyroporch_sdk::*;

fn call(host: &HostHandle) -> Result<String, String> {
    let response = host.http(&HttpRequest {
        method: "POST".into(),
        url: "https://api.example.com/v1/lights".into(),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: Some(json!({"on": true}).to_string()),
        timeout_ms: Some(5_000),
    })?;
    if !(200..300).contains(&response.status) {
        return Err(format!("the server answered {}", response.status));
    }
    Ok(response.body)
}
```

The host applies these rules, whatever the plugin asks:

| Rule | Detail |
|---|---|
| Sites must be allowed | Each site needs the person's OK. Sites listed in `plugin.toml` are allowed when the plugin is approved. For any other, the call fails with "needs your OK", and a request waits for the person in the app, so the same call works once they've said yes |
| A port is part of the site | `example.com:8443` is a different site from `example.com` |
| https only | Plain `http` only reaches this computer, unless the person adds `http://name` to `grants.toml` themselves |
| Methods | `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD` |
| Time limit | 15 seconds unless `timeout_ms` says otherwise, and between 100 milliseconds and 60 seconds |
| Answers | Up to 1 MiB, as text. Beyond that, `truncated` is true and `body` holds the first 1 MiB. Binary answers don't survive being turned into text |
| Error statuses | A 404 or 500 is a normal answer, so check `status` yourself |
| Redirects | Not followed, because they could carry a secret somewhere nobody approved. You get the 3xx status as the answer |

## Secrets

A plugin never sees a credential. It sends a placeholder, and the host swaps in the value as the request
leaves and blanks any copy out of the answer.

| Placeholder | Becomes | From code |
|---|---|---|
| `{{secret:name}}` | The secret | `secret_ref("name")` |
| `{{basic:user:name}}` | `user:secret` in base64, for HTTP Basic sign-in | `basic_ref("user", "name")` |

- **Declare a `FieldKind::Secret` field,** and the person picks or enters the secret in the form. The
  setting then holds the reference, ready to use: `format!("Bearer {}", request.text("token"))`.
- **Placeholders work in** headers, the body, and the part of an address after the `?`. Before the `?` they're
  refused, so a secret can't change where a request goes.
- **Your plugin has to be allowed the secret,** by name in `plugin.toml`, or with `secrets = ["*"]` for any the
  person chooses.
- **The secret has to allow the site.** Each one lists where it may go, and the person sets that. Asking
  to send it anywhere else fails, whatever the plugin was allowed.
- **The vault can be locked.** The call then fails with "the vault is locked", and the person only has to
  unlock it and press the key again.
- **Keys pressed by an assistant can't use secrets,** unless the person has turned that on.

## Recipes

Every Rust example in this guide is compiled as part of the project's tests, and the quick start is built
and run in the real sandbox, so what's here works.

### Showing a live value

Set `refreshable: true`, and add a number field called exactly `refresh_seconds`. At 1 or more, the app
re-runs the key on that timer, with `Trigger::Refresh`, while its page is on screen. This one also uses a
secret, and leaves the site for the app to ask about because the address is a `RequestUrl`, so its
`plugin.toml` needs only `secrets = ["*"]`:

```rust
use pyroporch_sdk::*;

pub struct Status;

impl Plugin for Status {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: "com.example.status".into(),
            name: "Status".into(),
            version: "1.0.0".into(),
            description: "Shows a value from a web API on a key.".into(),
            actions: vec![ActionSpec {
                id: "show".into(),
                name: "Show Status".into(),
                description: "Reads a value from a web API.".into(),
                icon: "📡".into(),
                refreshable: true,
                fields: vec![
                    Field::new("url", "Address", FieldKind::RequestUrl).required(),
                    Field::new("token", "API token", FieldKind::Secret),
                    Field::new("value", "Value to show", FieldKind::Text).help("For example /data/state"),
                    Field::new("refresh_seconds", "Refresh every (seconds)", FieldKind::Number).default(0),
                ],
                ..ActionSpec::default()
            }],
        }
    }

    fn run(&self, request: &RunRequest, host: &HostHandle) -> Result<Outcome, String> {
        let mut headers = Vec::new();
        let token = request.text("token");
        if !token.is_empty() {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        let response = host.http(&HttpRequest {
            method: "GET".into(),
            url: request.text("url").into(),
            headers,
            ..HttpRequest::default()
        })?;
        if response.status != 200 {
            return Err(format!("the server answered {}", response.status));
        }
        let answer: Value = serde_json::from_str(&response.body).map_err(|e| e.to_string())?;
        let shown = answer.pointer(request.text("value")).ok_or("that value isn't in the answer")?;
        let text = shown.as_str().map_or_else(|| shown.to_string(), str::to_string);
        Ok(Outcome { title: Some(text), ..Outcome::default() })
    }
}

export_plugin!(Status);
```

An action that changes something shouldn't repeat by itself. Refuse the timer in its `run()`:

```rust
use pyroporch_sdk::*;

fn check(request: &RunRequest) -> Result<(), String> {
    if request.trigger == Trigger::Refresh {
        return Err("this action only runs when pressed".into());
    }
    Ok(())
}
```

### Several actions, and a form that adapts

```rust
use pyroporch_sdk::*;

fn actions() -> Vec<ActionSpec> {
    let lights = FieldKind::Select { options: vec!["all".into(), "desk".into()] };
    vec![
        ActionSpec {
            id: "on".into(),
            name: "Lights On".into(),
            fields: vec![
                Field::new("which", "Which", lights).default("all"),
                Field::new("dim", "Dim them", FieldKind::Bool).default(false),
                Field::new("level", "Level", FieldKind::Select { options: vec!["low".into(), "mid".into()] })
                    .visible_when("which", "desk"),
            ],
            ..ActionSpec::default()
        },
        ActionSpec { id: "off".into(), name: "Lights Off".into(), ..ActionSpec::default() },
    ]
}

fn run(request: &RunRequest) -> Result<Outcome, String> {
    match request.action.as_str() {
        "on" if request.flag("dim") => Ok(Outcome::message("on, dimmed")),
        "on" => Ok(Outcome::message("on")),
        "off" => Ok(Outcome::message("off")),
        other => Err(format!("unknown action: {other}")),
    }
}
```

### Drawing on the keys

A plugin can play an animation across the whole deck, as the Screen Saver sample does. It needs `deck = true`
in `plugin.toml`, and two parts. Its action returns `DeckControl::ScreenSaver`, saying when to play, and
`paint()` draws each frame:

```rust
use pyroporch_sdk::*;

pub struct Sweep;

impl Plugin for Sweep {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: "com.example.sweep".into(),
            name: "Sweep".into(),
            version: "1.0.0".into(),
            description: "A band of colour across the deck.".into(),
            actions: vec![ActionSpec {
                id: "play".into(),
                name: "Sweep".into(),
                description: "Press to turn it on, and press again to turn it off.".into(),
                icon: "🌊".into(),
                previews: true,
                ..ActionSpec::default()
            }],
        }
    }

    fn run(&self, _request: &RunRequest, _host: &HostHandle) -> Result<Outcome, String> {
        let timing = SaverTiming { start_now: true, wait_minutes: 0 };
        Ok(Outcome { deck: Some(DeckControl::ScreenSaver(timing)), ..Outcome::default() })
    }

    fn paint(&self, request: &PaintRequest) -> Result<Vec<u8>, String> {
        let across = request.cols * request.blocks_per_key;
        let down = request.rows * request.blocks_per_key;
        let band = (request.millis / 40) as u32 % across;
        // One byte for brightness, then three for each square, row by row.
        let mut frame = Vec::with_capacity(1 + (across * down * 3) as usize);
        frame.push(100);
        for _y in 0..down {
            for x in 0..across {
                frame.extend_from_slice(if x == band { &[255, 140, 40] } else { &[20, 20, 30] });
            }
        }
        Ok(frame)
    }
}

export_plugin!(Sweep);
```

| `PaintRequest` | What it holds |
|---|---|
| `settings` | The settings of the action that started it |
| `rows`, `cols` | The size of the deck, in keys. Don't assume them: they change from one deck to the next |
| `blocks_per_key` | How many squares to draw across and down each key |
| `millis` | How long it's been playing. Base the animation on this, not on counting frames |
| `restart` | True for the first frame, so you can start fresh |

- **The answer must be exactly** `1 + (rows × blocks) × (cols × blocks) × 3` bytes. Anything else stops the
  animation. The first byte is brightness, as a percent of the person's setting.
- **Frames are asked for about 15 times a second,** and each one has the full [instruction limit](#limits).
- **`paint()` can't reach anything.** It has no host, so there's no clock, network or log. Use `millis`.
- **`SaverTiming` decides when it plays.** `start_now` starts it right away. `wait_minutes` starts it after
  that long with no key presses, and again each time it's stopped. With `wait_minutes` at 0, it only
  plays when started right away, then turns itself off. While it plays, any key stops it, and that press does
  nothing else. Pressing the key that turned it on turns it off.
- **`previews: true`** lets the person try it from the settings form.

## `plugin.toml`

This is what the person is shown before approving, and what the app holds the plugin to.

| Setting | Needed | What it's for |
|---|---|---|
| `id` | Yes | Must match `PluginInfo::id` |
| `wasm` | Yes | The plugin file's name. It has to sit next to `plugin.toml` |
| `name` | No | Shown in the app. Falls back to the id |
| `version` | No | Shown in the app |
| `description` | No | Shown in the app |

| `[permissions]` | Default | What it asks for |
|---|---|---|
| `http_hosts` | `[]` | Sites to allow on approval, like `"api.example.com"`, or with a port, `"example.com:8443"`. Ask only for what the plugin always needs: it can ask for others as it goes |
| `secrets` | `[]` | Vault secrets it may refer to, by name, or `["*"]` for any the person picks for a key. Each still only goes to its own sites |
| `deck` | `false` | May change the deck's brightness, put it to sleep and play on its keys |

Asking for less is better for everyone: the person sees this list when deciding whether to approve it.

## Limits

| Limit | Value |
|---|---|
| Plugin file | 32 MiB |
| Memory | 64 MiB |
| Work per call to `run()` or `paint()` | About 5 billion instructions. Going over stops the call |
| Time for one `run()` to go on calling the app | 5 minutes. After that its next call stops it |
| One message between the plugin and the app | 8 MiB |
| One answer from a site | 1 MiB |
| One web request | 60 seconds |

**Not in the sandbox:** files, sockets, threads, pressing keys, starting programs, the clock in `std::time`, random numbers from the system, and
environment variables. Those parts of the standard library compile, and then fail or panic when called. So
use `host.now_ms()` for the time and `host.http()` for the network. Crates that need any of these from the
system, or that use `wasm-bindgen` or WASI, won't load.

## Testing without the app

Your plugin is ordinary Rust, so `cargo test` runs it on your own computer. Give it a stand-in for the host,
and add this to the bottom of `lib.rs`:

<!-- guide-example: tests -->
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct FakeHost;

    impl Host for FakeHost {
        fn http(&self, _request: &HttpRequest) -> Result<HttpResponse, String> {
            Err("no network in this test".into())
        }
        fn log(&self, _level: LogLevel, _message: &str) {}
        fn now_ms(&self) -> u64 {
            41
        }
    }

    #[test]
    fn a_roll_lands_on_the_die() {
        let host: HostHandle = Arc::new(FakeHost);
        let request = RunRequest {
            action: "roll".into(),
            settings: Map::new(),
            trigger: Trigger::Press,
        };
        let outcome = Dice.run(&request, &host).unwrap();
        assert_eq!(outcome.title.as_deref(), Some("6"));
    }
}
```

A fake host can also record what it's asked to send, and answer with whatever you want to test against.

## Troubleshooting

The Plugins screen says what's wrong with anything in the folder.

| What you see | What it means |
|---|---|
| This folder has no plugin.toml, so it isn't a plugin | Each plugin needs its own folder, holding `plugin.toml` and the `.wasm` file |
| This is source code, and it has to be built first | The folder holds a `Cargo.toml`. Build it, then install only `plugin.toml` and the `.wasm` file |
| This plugin file needs its own folder here | A `.wasm` file was put straight in the plugins folder |
| Can't read its plugin.toml | The file has a mistake, which the message names. It needs an `id` and a `wasm`, and the id can't contain a slash or space |
| Can't read `name.wasm` | The `wasm` setting doesn't match the file's name |
| Its id is already taken | A built-in module or another plugin in the folder has this id |
| Waiting for your approval | Choose **Approve and turn on** |
| The plugin file changed since you approved it | Normal after a new build. Approve it again |
| The file's id doesn't match its plugin.toml | `id` in `plugin.toml` and `PluginInfo::id` differ |
| the plugin asks for something the sandbox doesn't offer | A dependency needs WASI or `wasm-bindgen`. Often it's a crate reaching for the clock or random numbers. `cargo tree` shows where it comes from |
| this file isn't a Pyro Porch plugin | `export_plugin!` is missing, or `crate-type` doesn't include `cdylib` |
| the plugin needs plugin system version | Built with a newer or older kit than this app. Rebuild it with this app's kit |

When pressing a key:

| What you see | What it means |
|---|---|
| this key uses `plugin/action`, which isn't installed | The plugin is off or removed, or the action's id changed |
| this key still needs: *label* | A `.required()` field has no value |
| needs your OK to contact *site* | Allow the site in the app, then press again |
| *site* must use https | See the rules in [Calling a site](#calling-a-site) |
| a secret can only go in the part of an address after the ? | Move it to a header, the body or the query |
| this plugin wasn't given access to the secret | Add it to `secrets` in `plugin.toml`, and approve the plugin again |
| the secret *name* isn't allowed to go to *site* | The person sets where each secret may go, in the vault |
| the vault is locked | Unlock it in the app |
| an assistant can't use vault secrets | Press it in the app, or turn this on under AI Assistant |
| the plugin stopped: ... | It panicked, or used up its instructions or memory. Return `Err` rather than panicking, because a panic's message doesn't reach the app |
| that plugin wasn't given access to the deck | It needs `deck = true`, and approving again |
| the plugin drew the wrong amount | `paint()` returned an error, or the wrong number of bytes |

**To see what's happening,** call `host.log(LogLevel::Warn, "...")` and read `audit.log` in the app's config
folder. Never log anything from a site's answer that could be private.

## Sharing a plugin

A plugin is its folder: `plugin.toml` and the `.wasm` file. Add a `README.txt` and your source code if you'd
like people to be able to read them, as the samples do. Zip the folder, and anyone can drop it into their
plugins folder and approve it. The same file works on every platform.

**Keep your folder names out of it.** A built file records where its source was, and that usually includes your user
name, even with `strip` on. This rewrites those paths, and the samples are built the same way:

```sh
RUSTFLAGS="--remap-path-prefix=$HOME=/home" cargo build --release --target wasm32-unknown-unknown
```

Before sharing:

- [ ] `id` is your own, and matches in both places.
- [ ] `plugin.toml` asks for as little as it can.
- [ ] Each failure returns an `Err` that a person can act on, and nothing panics.
- [ ] Every setting has a fallback if it's missing.
- [ ] Credentials go in `Secret` fields, and nothing logs them.
- [ ] An action that changes something refuses `Trigger::Refresh`.
- [ ] The `.wasm` file was built with your folder names kept out, then installed and tried in the app.

**Updating.** A new file needs approving again, but sites the person allowed stay allowed. Keep action
ids and setting keys the same, because keys in people's configs store them. To retire an action, set
`hidden: true` and keep it working.

## Writing a plugin in another language

The kit does all of this for Rust, but any language that compiles to WebAssembly can follow it. A plugin is a
module that imports one function and exports these, with `memory` also exported:

| Export | Signature | What it does |
|---|---|---|
| `ld_abi` | `() -> i32` | Returns the plugin system version, currently `1` |
| `ld_alloc` | `(len: i32) -> i32` | Sets aside `len` bytes, and returns where |
| `ld_free` | `(ptr: i32, len: i32)` | Frees what `ld_alloc` set aside |
| `ld_info` | `() -> i64` | Returns the `PluginInfo` as JSON |
| `ld_run` | `(ptr: i32, len: i32) -> i64` | Takes a `RunRequest` as JSON, and returns the result as JSON |
| `ld_paint` | `(ptr: i32, len: i32) -> i64` | Optional. Takes a `PaintRequest` as JSON, and returns a frame as raw bytes |

| Import | Signature | What it does |
|---|---|---|
| `pyroporch.host_call` | `(ptr: i32, len: i32) -> i64` | Sends a call to the app as JSON, and returns its reply as JSON |

**Passing data.** An `i64` answer packs a location and a length: `(ptr << 32) | len`. The app calls `ld_free`
on whatever a function returns, so that memory must come from `ld_alloc`. Input to `ld_run` and `ld_paint` is
put in memory from `ld_alloc`, and is the plugin's to free. So is a reply from `host_call`.

**The JSON.** `RunRequest`, `PaintRequest`, `PluginInfo` and `Outcome` are the types in `crates/sdk/src/lib.rs`, in
their `serde` form, and `trigger` is `"press"` or `"refresh"`. `ld_run` answers `{"Ok": outcome}` or
`{"Err": "reason"}`. `host_call` takes one of these, and replies in the same `Ok` or `Err` form:

```json
{"call": "http", "request": {"method": "GET", "url": "https://api.example.com/", "headers": [["Accept", "text/plain"]]}}
{"call": "log", "level": "warn", "message": "something to look at"}
{"call": "now_ms"}
```

Calls to `host_call` only work during `ld_run`. Anything else the module imports stops it loading.

## Keys that act on the computer

A plugin can't press keys, type, open apps or start programs, because the sandbox has no way to, and no permission
gives it one. Those are the Desktop actions, which are built as part of the app and aren't a plugin. To add to them,
change `crates/desktop` in this project and build the app. Mark each action that acts on the computer `Risk::High`, so the
person has to approve every key using it.

## Changing the samples

Edit a sample's source under `plugins/_examples/`, then run `plugins/_examples/build-example-plugins.sh`, which rebuilds
its `plugin.wasm` in its folder under `plugins/`. The app carries that file and the `plugin.toml` next to it. With
`--check`, the script only says whether those files are up to date, and a test runs that. The script only covers the
samples: build your own plugins as the quick start shows.
