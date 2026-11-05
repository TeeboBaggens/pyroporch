# Security

## How credentials are stored

Hashing, as with SHA, is one-way. That's right for checking a password, but an API key has to be sent
to a service later, so it must be recoverable. Fast hashes like SHA are also wrong for passwords,
because they make guessing cheap. So:

| What | How |
|---|---|
| API keys, passwords, tokens | Encrypted with XChaCha20-Poly1305, each with its own random nonce |
| The vault key | 256 random bits, stored only in encrypted form |
| Your passphrase | Never stored. Argon2id (64 MiB, 3 passes) turns it into the key that opens the vault key |
| A secret's name and allowed sites | Sealed into its encryption: changing them in the file breaks the secret |

The cryptography comes from the RustCrypto crates rather than OpenSSL, which keeps the build simple on
every platform. Secret names and allowed sites are visible in `vault.json`; values aren't.

Optionally, "Remember on this device" keeps the vault key in the system keychain (macOS Keychain,
Windows Credential Manager or Linux Secret Service). Anyone signed in as you can then unlock it. It's
off unless you choose it.

## Where credentials go

- Plugins and assistants only ever handle a placeholder such as `{{secret:name}}`. The app fills it in
  as the request leaves, and blanks any copy, plain or base64, out of the answer.
- Each secret names the sites it may go to, with no wildcards. Requests elsewhere are refused.
- A placeholder can't appear in the part of an address that decides the site, so it can't change where
  a request goes.
- Redirects aren't followed, so an allowed site can't pass your credential on.
- Unencrypted http is only allowed to this computer, unless you add `http://name` to `grants.toml`
  yourself.

## What can't be saved

Config is checked when a key runs, when you edit a key, and on anything an assistant asks to save.
These are refused:

- Anything in a secret field that isn't a vault reference.
- Headers and settings with sensitive names (such as `Authorization`, `password`, `token`, `api_key`)
  holding literal values.
- Addresses with a username, a password or a sensitive query value.
- Text in the published format of common tokens or private keys.

## Plugins

Plugins, including the two samples the app comes with, run in a WebAssembly sandbox with no access
to files, the network, programs or other plugins. Their one way out is a call to the app, which checks it against what you've allowed.
They stay off until you approve them, the approval is tied to the file's SHA-256 fingerprint, and
limits on memory (64 MiB) and instructions stop a plugin that misbehaves. The app approves its own
samples when it installs them, because it carries them. Editing one means approving it again. A plugin
can't mark itself as anything else: only what its approval and the person's choices allow gets through. See
[the plugin guide](../plugins/README.md) for the full list of what a plugin can and can't reach.

## Keys that act on your computer

Running commands, opening apps, hotkeys, media keys and typed text can't be sandboxed, so they're the Desktop actions,
built as part of the app, and no plugin can ask for them. A key using them needs your approval, tied to a fingerprint of
its settings, so a later change by a file edit or an assistant voids it. Presets come from a file, so they arrive unapproved too. A key's approval
covers every action on it and how long each one waits.

## Assistants

The built-in MCP server only hands an assistant placeholders, and it can't approve anything, or change security
or assistant settings. [MCP.md](MCP.md) lists every tool and limit.

## Files from elsewhere

An imported file is treated as untrusted. Any approval in it is thrown away, a key holding a credential
in plain text is left out, and pictures are renamed from their contents, so the file can't choose where
they land. Exports contain no secrets, approvals or allowed sites.

## Known limits

- **Other programs running as you.** A program with your user rights can edit config files, including
  an approval fingerprint or `grants.toml`, and could read this app's memory while the vault is open.
  What it can't do from the files alone is read a secret, or change which sites one may go to.
  Treat assistants that can run commands accordingly.
- **Secrets in memory.** The app wipes passphrases and secrets when done, but once a secret is filled
  into a request, the HTTP and TLS libraries hold copies that can't be wiped. Core dumps are switched
  off on macOS and Linux.
- **The plain-text check is a safety net, not a guarantee.** It recognises common names and formats. A
  password typed into an ordinary text field under an unremarkable name won't be caught, and neither
  will a credential that's part of an address's path, like some webhook links.
- **The Desktop actions are trusted.** They're compiled into the app and aren't sandboxed.
- **A forgotten passphrase can't be recovered,** unless this device remembers the vault.

This has had no independent security review.
