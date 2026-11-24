<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="Reacher CM" width="80" />
</p>

<h1 align="center">Reacher CM</h1>

<p align="center">
  A personal fork of <a href="https://github.com/alexandrosnt/Reach">Reach</a>, the modern, cross-platform SSH client and remote management tool, with additional features.
</p>

<p align="center">
  <a href="https://rsipka.github.io/ReacherCM/">Website</a> · <a href="https://github.com/rsipka/ReacherCM/releases">Download</a> · <a href="https://github.com/rsipka/ReacherCM/issues">Report a Bug</a> · <a href="https://github.com/alexandrosnt/Reach">Upstream Reach</a>
</p>

> **Reacher CM is built on Reach** by [Alexander Nt.](https://github.com/alexandrosnt) and the
> [Reach contributors](https://github.com/alexandrosnt/Reach/graphs/contributors).
> Reacher CM is a personal fork that adds features of its own on top. If you don't
> need what Reacher CM adds, use [Reach](https://github.com/alexandrosnt/Reach) itself. Some features
> are based on the [ReachQ](https://github.com/CPTProgrammer/ReachQ) fork by CPTProgrammer.
>
> **AI-assisted:** most of the code Reacher CM adds is written with AI coding tools
> ([Claude Code](https://claude.com/claude-code)), reviewed and tested by the maintainer.
>
> Reacher CM has its own app id, data folder and keychain entries, so it installs and runs next to
> Reach. It starts empty: restore a Reach backup or point it at your sync location to bring data over.

---

## Why Reacher CM?

PuTTY hasn't changed in decades, MobaXterm is Windows-only, and Termius charges a subscription for the basics. Reach, which Reacher CM is built on, is an SSH client built from scratch: native Rust networking, a fast UI, encrypted storage, and the same app on Windows, macOS, Linux and Android. No Electron, no subscription, no account.

Reacher CM adds what people moving from Remote Desktop Manager and similar tools miss most: importing from RDM and other tools, saved credentials shared by many sessions, password managers, session notes, Telnet and serial sessions, broadcast input, and sync through a folder, SFTP, Git, an SMB share or an S3 bucket.

## Added in Reacher CM

- **Imports from other tools**: Remote Desktop Manager (RDP, SSH, VNC and Telnet entries, folders, logins and notes), mRemoteNG, MobaXterm, PuTTY, WinSCP and Tabby.
- **Saved credentials**: one login kept once in a vault and used by many sessions, by every session in a folder, or by a jump host.
- **Password managers**: take a password from Passbolt, 1Password, Bitwarden or KeePass instead of keeping a copy.
- **Session notes and custom fields**, like RDM's Documentation tab, encrypted and synced with the session.
- **Edit many sessions at once**, and favorites on the start screen.
- **Telnet and serial sessions** for switches, routers and console servers, and **macOS login** for Macs over VNC.
- **tmux on the server**: sessions that survive a dropped connection, with tmux windows as tabs, and the tmux session and window in the terminal bar with a dropdown to switch.
- **Broadcast input**: type into several terminals at once.
- **RDP clipboard with pictures and file drops**: copy pictures both ways, drop files onto the desktop, see transfer progress, choose per session what the clipboard carries, and send and save files from a phone.
- **Searchable session history**, encrypted, with secrets masked.
- **Terminal your way**: hundreds of color schemes and your own themes, cursor style, copy and paste behaviour, and your installed fonts.
- **Local-first sync**: every vault works offline and syncs in the background, through Turso, a folder, SFTP, Git, an SMB share without mounting it, or an S3 bucket (Amazon, MinIO, Backblaze B2, Wasabi, R2). Conflicting edits are kept as copies, not lost.
- **Settings regrouped and searchable**, on desktop and phone.
- **Update checks you can switch off**, signed **apt and dnf repositories**, a **Czech translation**, and early **iOS** builds.

See the [changelog](CHANGELOG.md) for details.

## Features

### Connect

- **SSH** with tabs, split panes and tab groups, rendered with WebGL.
- **Your `~/.ssh/config`, all of it.** ProxyJump, ProxyCommand, ControlMaster, Kerberos (GSSAPI), X11 forwarding, PKCS#11 and FIDO2 keys, and the rest of the 104 keywords.
- **Remote Desktop (RDP), VNC and SPICE** in a tab. RDP shares the clipboard, files and a local folder as a drive. SPICE opens KVM/QEMU virtual machines, including Proxmox VE's and virt-manager's .vv files.
- **WSL distros and local shells** next to your servers, plus a **serial console** for routers and switches.
- **Host keys checked properly.** Trust on first use, a loud stop when a key changes, and never a silent accept.

### Work

- **Command palette** (Ctrl+Shift+P): any session, tab, page, snippet or action in a few keystrokes.
- **SFTP explorer**: drag and drop, edit in place, compress and extract on the server, drag files straight to your desktop.
- **Port forwarding**: local and SOCKS proxies from the Tunnels panel, plus everything your ssh_config forwards.
- **Snippets** with Tab completion, and **Recipes**: parameterised scripts with a declared risk level, checked before they run.
- **Live server stats** (CPU, memory, disk, network, who's logged in) without installing an agent.
- **Session logging** to plain text files.

### Stay safe

- **Encrypted vault** for sessions, passwords and keys (XChaCha20-Poly1305, Argon2id), with optional encrypted cloud sync.
- **Master password**, plus unlock with Windows Hello, Touch ID, Android biometrics or a FIDO2 security key. Auto-lock when idle or when your computer locks.
- **Password managers**: point a session at a Passbolt, 1Password, Bitwarden or KeePass entry instead of keeping a copy.
- **Backups** of your sessions and settings, optionally password-protected.
- **Zero-trust defaults**: anything that opens a door, like session sharing or the MCP server, stays off until you switch it on.

### DevOps (opt-in, in Settings → DevOps)

- **Databases**: PostgreSQL, MySQL, MariaDB, SQL Server, SQLite and Redis. A data grid, a schema-aware SQL editor, a table designer that shows the `ALTER` before it runs, and confirmations scaled to the risk. Connect directly or through an SSH session.
- **Containers and Kubernetes**: Docker, Podman, Compose, Kubernetes and Helm on the servers you already reach.
- **Ansible** and **OpenTofu**: run playbooks and plans with streaming output. On Windows, Ansible runs through WSL.

Each tool is off on a fresh install. Switching one off hides it without deleting anything it saved.

### Collaborate

- **Session sharing**: two terminals side by side, peer to peer and end-to-end encrypted. The owner decides whether the other side can type.
- **MCP server**: hand a session to Claude Code, Cursor or any MCP client. Loopback only, with guards against writing blind or reading secrets.
- **AI assistant** with your own API key.

### Make it yours

- **Themes** that colour the whole app, not just the terminal, from a registry pinned by SHA-256.
- **Interface size** (Ctrl+= / Ctrl+-) for the whole window, separate from the terminal's text size.
- **Lua plugins** in a sandbox.
- **Ten languages**: English, Czech, German, Spanish, French, Italian, Greek, Bulgarian, Russian and Chinese.
- **A real phone app**: Esc, Ctrl, Alt and arrow keys above the keyboard, pinch to zoom, and a layout that respects notches and large text.

## Download

Get Reacher CM from its [releases page](https://github.com/rsipka/ReacherCM/releases):

- Windows: `.exe`, `.msi`
- macOS: `.dmg`, Apple silicon and Intel
- Linux: `.AppImage`, `.deb`, `.rpm`
- Android: `.apk`
- iPhone and iPad: unsigned `.ipa` for [LiveContainer](https://github.com/LiveContainer/LiveContainer) or sideloading. Add the source `https://raw.githubusercontent.com/rsipka/ReacherCM/update-feed/apps.json` in LiveContainer's Sources tab and each release shows up as an update.

Reacher CM updates itself from those releases. On Ubuntu, Debian, Fedora and RHEL it can also come from its signed apt and dnf repositories (stable releases only); setup is on the [website](https://rsipka.github.io/ReacherCM/#linux).

## Built with

[Tauri 2](https://v2.tauri.app), Rust and Svelte 5. The SSH stack is pure Rust ([russh](https://github.com/warp-tech/russh)), with no OpenSSH dependency, and the UI uses the system webview instead of a bundled Chromium, so Reacher CM stays small and light.

| | |
|---|---|
| **Backend** | Rust, Tokio, russh |
| **Frontend** | Svelte 5, SvelteKit, TypeScript, Tailwind CSS 4 |
| **Terminal** | xterm.js with WebGL |
| **Crypto** | XChaCha20-Poly1305, Argon2id, X25519 |

### Build from source

You'll need [Rust](https://rustup.rs), [Node.js 22+](https://nodejs.org) and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```bash
git clone https://github.com/rsipka/ReacherCM.git
cd ReacherCM
npm install
npm run tauri dev     # run it
npm run tauri build   # package it
```

## Community

- **[Reach Discord](https://discord.gg/CSbEybvDVV)**: the upstream Reach community.
- **[Reacher CM issues](https://github.com/rsipka/ReacherCM/issues)**: bugs in Reacher CM and in the features it adds.
- **[Reach issues](https://github.com/alexandrosnt/Reach/issues)**: bugs that Reach has too belong upstream. For a larger change, open an issue there first.
- **[Changelog](CHANGELOG.md)**: what changed in each release.

Pull requests, translations and recipes for the [registry](https://github.com/alexandrosnt/reach-recipes-registry) are all welcome.

## Support Reach

Reacher CM exists because of Reach. Reach is free and MIT licensed, and it stays that way: nothing is or ever will be paywalled. It's built in spare time with no budget, which is why the installers aren't code-signed yet. If Reach saves you time, **[GitHub Sponsors](https://github.com/sponsors/alexandrosnt)** helps pay for a signing certificate, test hardware, and more evenings spent on Reach. A good bug report or telling a colleague helps just as much.

## Contributors

Thank you to everyone who has sent code to Reach, which Reacher CM is built on. The picture is drawn from the repository's own contributor list, so a merged pull request puts you in it (it can take a few days to show).

<a href="https://github.com/alexandrosnt/Reach/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=alexandrosnt/Reach&max=100" alt="Contributors to Reach" />
</a>

## License

Reacher CM is free software under the [GNU General Public License, version 3 or later](LICENSE) (GPL-3.0-or-later). You may use, change and share it; if you distribute it or a changed version, you must pass on the source under the same licence.

It is based on [Reach](https://github.com/alexandrosnt/Reach), which is MIT licensed. The code taken from Reach and from the ReachQ fork keeps its MIT copyright and permission notice in [LICENSE-MIT](LICENSE-MIT). The sync server, [ReacherCM-Sync](https://github.com/rsipka/ReacherCM-Sync), is under the AGPL-3.0.
