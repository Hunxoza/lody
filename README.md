# Lody

Let your programs talk to you, in your own language. Lody reads aloud what a program says,
translated if you like, so you can keep your eyes on your work. It isn't only for AI: each
program it reads is one module, and [adding yours](CONTRIBUTING.md#add-a-program) is the best
way to help. Thai first; a language is one TOML file in `crates/lody-core/locales/`.

It starts in the terminal, with Claude Code, where it's almost automatic: talk to the AI with
[Handy](https://github.com/cjpais/Handy) (optional; Lody can install and check it for you),
listen to its answer with Lody.

Windows, Linux and macOS (on macOS the Windows-only and Linux-only parts, such as muting other
programs or pausing while Handy records, do nothing yet).

## The app

A window and a tray menu. It reads replies in the background. Its settings cover Lody
(language, translation, voice, speed, how much to read); for Handy it installs it if missing,
checks that its model, language and translation suit your language, and opens Handy to change
them. Lody never writes Handy's settings, so the two can't overwrite each other.

The **Voice** tab picks the voice from a list of free ones, each with a note on its speed, how
natural it sounds and whether it needs the internet: Microsoft Edge's Thai and multilingual
voices, Google Translate's, the computer's own, or your own program or OpenAI-compatible service
(`crates/lody-core/voices.toml`; Edge's list is fetched live). A sentence a voice fails on is
read by Google's instead of being skipped.

The **Translator** (Replies tab) is picked the same way, from `crates/lody-core/translators.toml`:
Google Translate, its second address (the same translations, limited separately), or Claude
Haiku or Sonnet through the `claude` command and your Claude Code sign-in (slower, more natural
Thai). When it fails, Lody says so in your language and reads the English.

The **Read** tab shows each reply translated in full. **Corrections** keeps what Handy heard
next to what you sent to Claude Code (with the recording) in `~/.local/state/lody/corrections/`,
and suggests the English words you keep fixing for Handy's Custom Words.

### Install

Download the installer for your computer from
[Releases](https://github.com/Hunxoza/lody/releases) (`<version>` below is the one you download):

```sh
sudo dnf install ./Lody-<version>-1.x86_64.rpm     # Fedora
sudo apt install ./Lody_<version>_amd64.deb        # Ubuntu, Debian
chmod +x Lody_<version>_amd64.AppImage && ./Lody_<version>_amd64.AppImage   # any Linux, no install
```

On Windows, run `Lody_<version>_x64-setup.exe` (installs for you only, no administrator) or
`Lody_<version>_x64_en-US.msi`. Audio plays through Windows itself: nothing else to install.

On macOS, open `Lody_<version>_universal.dmg` and drag Lody to Applications. The app is not
signed yet, so the first time right-click it and choose **Open**. Audio needs `brew install mpv`.

### Build the installers

```sh
cargo install tauri-cli --version "^2" --locked   # once
cd crates/lody-app
NO_STRIP=true cargo tauri build                    # NO_STRIP: the AppImage tool fails on Fedora without it
# -> target/release/bundle/{rpm,deb,appimage}/
```

On Windows, first install the C++ build tools and Rust (WebView2 comes with Windows 11), then
build in PowerShell:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install Rustlang.Rustup
cargo install tauri-cli --version "^2" --locked
cd crates/lody-app
cargo tauri build --bundles nsis,msi                # -> target/release/bundle/{nsis,msi}/
```

Or let GitHub build them: **Actions → Build → Run workflow** builds Windows, macOS and Linux
side by side. A release merged into `main` is built and published by itself (see
[Releasing](CONTRIBUTING.md#releasing-maintainer)). This works in a fork too, once Actions is
turned on in the fork's Actions tab.

### Debug build

When something goes wrong (a voice switching, a reply not read), a debug build shows why:

```sh
cargo run -p lody-app    # debug build; its log prints in the terminal (a console window on Windows)
RUST_LOG=debug cargo run -p lody-app   # more detail
```

Without building anything, **Actions → Build → Run workflow** with **debug** ticked makes debug
installers.

### Run from source

Needs [Rust](https://rustup.rs), and for audio on Linux and macOS `mpv` or `ffplay`. Then, for
your system:

```sh
# Fedora
sudo dnf install webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel gtk3-devel
# Ubuntu, Debian
sudo apt install build-essential libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libxdo-dev libssl-dev
# macOS
xcode-select --install
# Windows: the C++ build tools, as in "Build the installers" above
```

```sh
cargo build --release -p lody-app
./target/release/lody-app            # opens the settings window; closing it keeps Lody running
./target/release/lody-app --hidden   # start in the background
```

On Linux, run from source, **App → Show Lody in the app menu** adds it to GNOME's app list
(installed packages are there already), and **Start Lody when I log in** starts it in the
background. On GNOME the tray icon needs the AppIndicator extension; without it, open Lody
again from the app menu.

## The command

The same reading without a window:

```sh
cargo build --release -p lody
./target/release/lody say -t "The tests pass now."   # translate into Thai and read aloud
./target/release/lody watch                          # read Claude Code's replies aloud
```

`lody watch` reads the session logs Claude Code already writes (`~/.claude/projects`), so
nothing is installed into Claude Code. Only replies finished after it starts are read, a new
prompt stops that session's reply, and while Claude works Lody says what it's doing in your
language without translating: running a command, what it searches the web for, which file it
opens (*While the AI works* in the settings); never the command itself. Only the final answer
is translated, so the translator isn't asked too often (Google refuses for a while after too
many requests). Code, tables, paths, URLs, commands and anything that looks like a secret stay
on your computer: only prose is sent to the translator and Microsoft's Edge voices.

Settings live in `config.toml`, every key optional (see `crates/lody-core/src/settings.rs`):

| System | Settings | Logs and state |
|---|---|---|
| Linux | `~/.config/lody/` | `~/.local/state/lody/` |
| macOS | `~/Library/Application Support/lody/` | the same folder |
| Windows | `%APPDATA%\lody\` | `%LOCALAPPDATA%\lody\` |

## Layout

| Path | What |
|---|---|
| `crates/lody-core` | filter, translation, voices, speaking queue, Handy setup: no window, tested |
| `crates/lody-core/src/sources` | one module per program Lody reads (`claude_code.rs`) |
| `crates/lody-app` | the app: tray, settings window (`ui/`, plain HTML, no build step) |
| `crates/lody-cli` | the `lody` command |

```sh
cargo test                  # offline
cargo test -- --ignored     # also talks to Microsoft's voice servers and Hugging Face
```

## Contributing

Fork the repository, make your change on a `feat/…` or `fix/…` branch, and open a pull request
into `staging`. [CONTRIBUTING.md](CONTRIBUTING.md) shows how, and how to add a program for Lody
to read, a language, a voice or a translator.

## License

Lody is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Unless you say otherwise, anything you contribute is licensed the same way, with no additional
terms.

`crates/lody-core/handy/models.json` is trimmed from [Handy](https://github.com/cjpais/Handy)'s
model catalog, © 2025 CJ Pais, under the [MIT License](crates/lody-core/handy/LICENSE).
