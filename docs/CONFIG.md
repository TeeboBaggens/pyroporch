# Config files

Everything lives in plain TOML files in one folder, which the Settings screen can open:

| Platform | Folder |
|---|---|
| macOS | `~/Library/Application Support/pyroporch` |
| Windows | `%APPDATA%\pyroporch` |
| Linux | `~/.config/pyroporch` |

Set `PYROPORCH_HOME` to use another folder. On macOS and Linux, files are readable only by your user account.

**Edits are picked up within a second, while the app is running.** A file with a mistake is ignored, the
last good copy stays in use, and the app shows what's wrong. The file itself is left alone: nothing is saved
over it until it's fixed. If `config.toml` can't be read when the app starts, there is no good copy to use, so
it runs on the default settings with assistant access off. If `grants.toml` has a mistake, plugins from the
plugins folder stay off until it's fixed.

## `config.toml`

```toml
active_profile = "default"
run_in_background = true   # closing the window leaves the deck working, from the menu bar or tray

[ui]
theme = "dark"           # or "light"
accent = "#E8743B"
scale = 1.0
tray_icon = "color"      # or "white" or "black": the lantern in the menu bar or tray
collapsed_groups = []    # groups folded away in the action list: a plugin id, "organize" or "presets"
# colours kept for choosing again, on any key
favorite_colors = ["#2F6FDB", "#0F8B8D", "#2E8B57", "#7A4FD0", "#A83279", "#B4532A", "#C0392B", "#B8860B", "#3A4252", "#14171C"]

[security]
auto_lock_minutes = 15   # 0 means never
mcp_may_use_secrets = false   # keys an assistant presses may use vault secrets

[mcp]
enabled = true           # assistants may connect
read_only = false        # "Look only": they can read, but not change or press anything

[hardware]
enabled = true           # use a connected USB deck
brightness = 70          # 0 to 100

# Ready-made actions, with no code. They appear at the top of the action list.
[[presets]]
name = "Lights Off"
icon = "💡"
color = "#B8860B"
action = "pyroporch.api-call/request"

[presets.settings]
url = "https://home.example.com/api/lights/off"
method = "POST"
auth = "bearer token"
secret = "{{secret:home_token}}"
```

The app also keeps `version`, for the file's format, and `sample_hashes`, for which samples it has installed.
Leave those alone. Anything left out of the file takes the value shown above.

## Saving

The Save button at the top of the window, or Cmd/Ctrl+S, puts the current profile in its file. Until then,
changes are kept in the `autosave` folder a moment after each edit, so nothing a program or an assistant wrote to
the profile's file gets overwritten, and keys on the deck already use your changes:

| The profile | Its autosave file |
|---|---|
| Has been saved | `autosave/<name>_autosave.toml` |
| Has never been saved | `autosave/<date>_<time>_autosave.toml` |

Saving puts your changes together with anything else that changed in the file, and removes the autosave. If the
app closes first, the changes come back the next time it starts, still unsaved. An assistant sees a profile as
it was last saved.

Settings have no Save button. They go to `config.toml` straight away, because switches such as assistant access
have to count at once, and each save keeps anything another program changed in the file.

## `profiles/<name>.toml`

```toml
name = "Studio"
rows = 3
cols = 5

[[pages]]
name = "Main"

[[pages.keys]]
row = 0
col = 0
title = "Status"
icon = "🌐"
color = "#0F8B8D"

# A key can do several things: each step runs in order, after waiting its delay_ms.
[[pages.keys.steps]]
action = "pyroporch.api-call/request"
delay_ms = 0

[pages.keys.steps.settings]
url = "https://api.example.com/v1/status"
show_value = "/data/state"
refresh_seconds = 30
```

A grid is 1 to 8 rows by 1 to 16 columns, and each key's `row` and `col` count from 0. A key can have these:

| Setting | What it does |
|---|---|
| `row`, `col` | Where it sits. A key outside the grid is kept, and comes back if the grid grows |
| `title` | The text on it |
| `icon` | One character or emoji, or `builtin:lantern-out` for the lantern that's gone out |
| `background` | A picture filling the key: see [Folders and pictures](#folders-and-pictures) |
| `color`, `text_color` | Its background and title colours, like `#2F6FDB` |
| `icon_color` | The colour of an icon that's a character. The same as `text_color` if left out |
| `toggle` | Pressing flips between two looks |
| `alt_title`, `alt_color` | The second look |
| `steps` | What it does, with an `action`, its `settings` and a `delay_ms` for each |
| `folder` | Makes it a folder instead: see [Folders and pictures](#folders-and-pictures) |
| `approved` | The app's record of your approval. Leave this alone |

Every action and its settings are in [ACTIONS.md](ACTIONS.md), and the MCP tool `list_actions` lists what's installed
right now. Keys that act on your computer need approving in the app after you edit them by hand: the
approval covers exactly the settings you approved.

**Secrets never go in these files.** Refer to the vault with `{{secret:name}}`. A key holding a credential
in plain text won't run, and the app says which setting is the problem.

Brightness keys change `[hardware] brightness`, so the setting and the keys always agree.

## Folders and pictures

A key can be a folder, which is a separate screen of keys, and folders can go 8 deep. Its first slot, row 0,
column 0, is always the way back:

```toml
[[pages.keys]]
row = 1
col = 4
title = "Media"
icon = "📁"

[pages.keys.folder]
name = "Media"

[[pages.keys.folder.keys]]
row = 0
col = 1
title = "Mute"
```

`background = "<name>.png"` on a key fills it with a picture from the `images` folder. Only the file
name counts, so a config can't point anywhere else. The app's picture window fits a picture to the key and
adds it there, and pictures are named after their contents.

## Export and import

The Settings screen writes everything, or one profile, to a single `.pyroporch` file, pictures included.
No secrets, approvals or allowed sites are in it, so it's safe to share. Importing only ever adds:
keys holding a plain credential are left out, and everything else needs the person's approval as usual.

## Other files

| File | What it is |
|---|---|
| `vault.json` | The encrypted vault |
| `grants.toml` | What you've allowed: sites and plugins |
| `pending.toml` | Requests waiting for your answer |
| `audit.log` | What ran and what changed. It names sites only, never full addresses or values |
| `autosave/` | Changes not saved yet. See [Saving](#saving). Safe to delete, which throws those changes away |
| `plugins/<name>/` | Plugins, including the two samples, API Calls and Screen Saver. Remove what you don't want. A folder whose name starts with `.` or `_` is ignored |
| `plugins/README.md` | How to write a plugin. The app keeps it up to date |
| `images/` | Key pictures, named after their contents |
| `instance.lock` and `instance.build` | Keep one copy of the app running per config folder |

`grants.toml` is what the Plugins screen writes. Adding `http://name` to a plugin's `http_hosts` there is the one
reason to edit it by hand: it allows unencrypted traffic to that site.
