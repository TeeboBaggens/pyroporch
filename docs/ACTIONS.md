# Actions

Everything a key can do out of the box. In a config file or through an assistant, an action's name is
`plugin_id/action_id`, and its settings are the `key` values in each table. A plugin can add more: the MCP
tool `list_actions` always lists what's installed right now.

Settings without a *required* mark can be left out. "Needs approval" means the person has to approve each key
using that action, because it acts on the computer directly.

## Navigation

Built in. Moves between screens, so these only work from the deck or the app.

| Action | What it does | Settings |
|---|---|---|
| `core.navigation/next_page` | Goes to the next page, wrapping round to the first | None |
| `core.navigation/previous_page` | Goes to the previous page, wrapping round to the last | None |
| `core.navigation/go_to_page` | Goes to a page by name | `page`, *required* |
| `core.navigation/switch_profile` | Switches to a profile by id | `profile`, *required* |

Folders aren't an action. A key is made into a folder in the config file, in the app, or with `is_folder` in the
MCP tool `set_key`.

## Deck

Built in. Controls the USB deck itself.

| Action | What it does | Settings |
|---|---|---|
| `core.deck/sleep` | Turns every key and the backlight off. Any key wakes it, and that press does nothing else | None |
| `core.deck/brighter` | Turns the brightness up | `step`: how many percent, 10 if left out |
| `core.deck/dimmer` | Turns the brightness down, stopping at 5% so the deck can't look switched off | `step`: how many percent, 10 if left out |
| `core.deck/set_brightness` | Goes straight to one brightness | `percent`: 0 to 100, 50 if left out |

## Desktop

Built in. Acts on the computer, so all but one need approval. Change anything about an approved key, and it needs
approving again.

| Action | What it does | Settings | Needs approval |
|---|---|---|---|
| `pyroporch.desktop/open_url` | Opens an http or https address in the browser | `url`, *required* | No |
| `pyroporch.desktop/open_path` | Opens an application, file or folder | `path`, *required* | Yes |
| `pyroporch.desktop/press_key` | Presses one key | `keys`, *required* | Yes |
| `pyroporch.desktop/hotkey` | Presses a key combination | `keys`, *required* | Yes |
| `pyroporch.desktop/media` | Sends a media key | `key`, *required*: `play/pause`, `next`, `previous`, `volume up`, `volume down` or `mute` | Yes |
| `pyroporch.desktop/type_text` | Types a piece of text | `text`, *required* | Yes |
| `pyroporch.desktop/run_command` | Starts a program, with no shell in between | `program`, *required*. `args`: separated by spaces | Yes |

On a Mac, pressing and typing need Pyro Porch allowed under Privacy & Security > Accessibility.

**Writing keys.** `keys` is names joined with `+`, like `ctrl+shift+m`, and letter case doesn't matter:

| Kind | Names |
|---|---|
| Modifiers | `ctrl` or `control`, `shift`, `alt` or `option`, `cmd`, `command`, `win`, `windows`, `super` or `meta` |
| Function keys | `f1` to `f24`, as far as the system has them |
| Moving | `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown` |
| Editing | `enter` or `return`, `tab`, `esc` or `escape`, `space`, `backspace`, `delete`, `capslock` |
| Media | `playpause`, `medianext`, `mediaprevious`, `volumeup`, `volumedown`, `volumemute` |
| Symbols | `plus` and `minus`, since `+` joins names. Any other single character is itself |
| Not on a Mac | `insert`, `printscreen`, `pause`, `numlock` |
| Linux only | `scrolllock` |

## API Calls

A sample plugin. Sends a web request. See [SECURITY.md](SECURITY.md) for how its credentials are kept.

**`pyroporch.api-call/request`**

| Setting | What it does |
|---|---|
| `url` | *Required.* The address to call. The person allows each site the first time |
| `method` | `GET`, `POST`, `PUT`, `PATCH` or `DELETE`. `GET` if left out |
| `auth` | `none`, `bearer token`, `basic` or `api key header`. `none` if left out, which sends no credentials, for a public API |
| `secret` | The credential for `auth`, as a `{{secret:name}}` reference. Not needed for `none` |
| `username` | For `basic` |
| `key_header` | For `api key header`. `X-Api-Key` if left out |
| `headers` | Extra headers, as names and values |
| `body` | What to send. Goes as JSON unless `headers` sets a `Content-Type` |
| `show_value` | Which part of a JSON answer to show on the key, like `/data/temperature` |
| `watch_value` | Which part of a JSON answer `rules` looks at, like `/status/indicator`. The same as `show_value` if left out |
| `rules` | Changes the key's icon and colour by that value: see [Changing the look by the answer](#changing-the-look-by-the-answer) |
| `refresh_seconds` | Re-runs on this timer while its page is on screen, for `GET` only. 300 is every 5 minutes. 0 if left out, which means only when pressed |
| `timeout_seconds` | Gives up after this long. 15 if left out |

A status outside the 200s counts as a failure.

### Changing the look by the answer

This key checks GitHub's public status every 5 minutes, and needs no credentials:

```toml
[pages.keys.steps.settings]
url = "https://www.githubstatus.com/api/v2/status.json"
watch_value = "/status/indicator"
show_value = "/status/description"
refresh_seconds = 300

[[pages.keys.steps.settings.rules]]
when = "is"
value = "none"
icon = "✅"
color = "#2E8B57"

[[pages.keys.steps.settings.rules]]
when = "is"
value = "minor"
icon = "⚠"
color = "#B8860B"

[[pages.keys.steps.settings.rules]]
when = "is"
value = "major"
icon = "⚠"
color = "#B4532A"

[[pages.keys.steps.settings.rules]]
when = "is"
value = "critical"
icon = "🔥"
color = "#C0392B"
```

Each rule has these, and the first one that fits wins:

| Part | What it does |
|---|---|
| `when` | `is`, `is not`, `contains`, `above`, `below` or `anything else`. `above` and `below` compare numbers. `anything else` belongs last, to catch what the others didn't |
| `value` | What to compare with. Capitals don't count |
| `icon` | One character or emoji to show, not a file. The key's own if left out |
| `color` | The colour to show, like `#2E8B57`. The key's own if left out |

The key goes back to its own look when no rule fits, and when a call fails, so an old answer never passes for a
current one. A picture on the key covers its icon and colour, so a key with rules is best without one.

## Screen Saver

A sample plugin. Plays an animation across the keys.

**`pyroporch.screensaver/toggle`** turns the screen saver on, and pressing the same key turns it off. While it plays,
any key stops it, and that press does nothing else.

| Setting | What it does |
|---|---|
| `animation` | `Breathing`, `Rainbow: side to side`, `Rainbow: up and down`, `Snake`, `Campfire`, `Digital rain`, `Twinkling stars` or `Game of Life`. `Rainbow: side to side` if left out |
| `start_now` | Starts as soon as the key is pressed |
| `wait_minutes` | Starts after this long with no key presses, and again each time it's stopped. 0 means it only plays when started right away, then turns itself off |
| `speed` | `slow`, `normal` or `fast`. `normal` if left out |

It can't be started by an assistant, because that needs the deck or the app.

## Keys that do several things

A key's `steps` run in order, and stop at the first failure. `delay_ms` on a step is how long it waits before
that step, up to 10 minutes, and each action takes effect as it runs. In the app, drop one action onto a key
that already has one to add a step.
