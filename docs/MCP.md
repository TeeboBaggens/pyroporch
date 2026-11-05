# The built-in MCP server

`pyroporch mcp` serves the [Model Context Protocol](https://modelcontextprotocol.io) over stdin and
stdout. An assistant starts it as a child process, so there's no background service and no network
port. The AI Assistant screen shows the exact settings to copy:

```json
{"mcpServers": {"pyroporch": {"command": "/path/to/pyroporch", "args": ["mcp"]}}}
```

It speaks protocol revision 2026-07-28, and the older revisions back to 2024-11-05 for clients that
open with `initialize`.

It edits the same config files as the app, so the running app shows changes within a second.

## How an assistant uses it

1. `get_overview` says what's set up, what's waiting for the person, and what the assistant may do.
2. `list_actions` says what a key can do, and which settings each action takes.
3. `set_key` adds keys. Any `needs_person` in its reply is what to tell the person.
4. For a credential, `request_secret` asks the person to enter it in the app, and returns a
   `{{secret:name}}` reference to put in the key's settings.
5. `check_config` finds anything that still won't run, and `press_key` tries a key.

To add an ability no action offers, `get_guide` with the topic `plugins` explains how to write a plugin.

## Tools

These can always be used, including when the person has chosen "Look only":

| Tool | What it does |
|---|---|
| `get_overview` | Profiles, plugins, the vault's state, anything waiting for the person, and what the assistant may do |
| `list_actions` | Every action and its settings, grouped by plugin with each plugin's id |
| `get_profile` | All pages and keys in a profile, including inside folders |
| `get_settings` | How the app looks, the deck and background settings, and what the assistant may do |
| `list_secrets` | Names and allowed sites, never values |
| `list_images` | Pictures a key's background can use |
| `get_guide` | This app's guides, by topic: `plugins`, `actions`, `config`, `assistant` or `security` |
| `check_config` | Every key that wouldn't run and why, with where it is, in the form the other tools take |

These make changes, and aren't offered under "Look only":

| Tool | What it does |
|---|---|
| `set_key` | Create a key, or replace everything about one. Several steps run in order, each waiting its `delay_ms` first. `is_folder` makes a folder. Won't replace a folder that has keys in it |
| `move_key` | Move a key to an empty slot on the same screen, with its approval and anything inside it |
| `clear_key` | Remove a key, and if it's a folder, what's in it |
| `create_profile`, `set_active_profile` | Add a profile, and switch to one |
| `add_page`, `rename_page` | Add a page, and rename one |
| `add_preset`, `remove_preset` | Add a ready-made action to the action list, and take one out |
| `add_image` | Copy a PNG file from this computer into the images folder, and get its name for a key's `background` |
| `update_settings` | Change the theme, accent, scale, menu bar icon, deck brightness, whether the deck is used, and whether the app runs in the background |
| `request_secret` | Ask the person to enter a credential; returns only a placeholder. Doesn't ask again if the vault has it for those sites |
| `request_site_access` | Ask the person to allow a site for a plugin |
| `press_key` | Try a key |

**Reaching a key.** Keys are found by `row` and `col` from 0, with optional `profile` and `page`, which default to the
active profile and its first page. For a key inside a folder, `folder` lists the `[row, col]` of each folder to open,
outermost first. Row 0, column 0 of a folder is its back key, which can't be replaced.

**Answers.** Each answer is JSON, as text and as `structuredContent`. A failure sets `isError` and says what's wrong,
so the assistant can correct itself, and that includes a misspelled argument. An unknown tool is a protocol error.
Tools carry `title` and annotations saying whether they read, add or can replace something.

## The limits on an assistant

- It can't read or store a secret. `request_secret` hands the typing to the person.
- It can't allow a site, approve a plugin, or approve a key that acts on the computer. Those wait in
  the app.
- It can't change security or assistant settings: `update_settings` leaves them out, including its own access.
- It can't delete a profile or a page.
- A request to save a credential in plain text is refused before anything is written.
- A credential saved in plain text by hand is hidden from what it reads.
- It sees a profile as last saved in the app. Changes waiting in an autosave file aren't visible to it, and are put together with its own when the person saves.
- A key it saves can't both refresh on a timer and use a vault secret, unless the person lets assistants use secrets.
- Keys it presses can't use vault secrets, unless the person turns that on in the app and this device
  remembers the vault.
- A key it presses can change the deck's brightness, but can't change pages, put the deck to sleep or start the
  screen saver, because those need the deck or the app.
- It can press keys the person has already approved, including ones that act on the computer. Turn on
  "Look only" in the app to prevent all changes and presses.
- `add_image` only takes PNG files, and never gives what it read back to the assistant.
- Changes it makes are recorded, and the AI Assistant screen shows the recent ones.

These limits are enforced by the MCP server. An assistant that can also run its own commands on your
computer isn't held back by them, except for what the vault's encryption protects. See
[SECURITY.md](SECURITY.md).
