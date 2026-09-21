# tacoshell

A generic use for anything taco wrap for TUI apps. 

**TLDR:**

It puts "shell" apps in a "wrapper" so they get their own window with an icon and a desktop entry, so it feels like a normal app instead of "open a terminal and type this" (which on most phones and tablets you can't even do).

It can wrap either:

- **a local program**, run in a pty
- **an app on a server over SSH**, with a login screen, server key checks and auto reconnect. 
  - That's what I use it for mainly as of now until taskologic gets a real API that is exposable

Everything it puts on screen (logo, text, colors, keyboard layout) comes from one TOML file that gets baked into the binary at build time. Made a new TUI app? = Just make a new TOML file and aslong as your TUI app isnt overly keyboard driven any Normie should be able to use it.

## Building yout apps

```bash
make CONFIG=path/to/yourapps.toml            # release build
make CONFIG=path/to/yourapps.toml install    # install it rawdog style
make CONFIG=path/to/yourapps.toml deb        # Make a .deb (for debian/ubuntu)
make CONFIG=path/to/yourapps.toml pkg        # Make a .pkg (for mac)
```
Without any config you get `tacoshell.toml`, which just wraps `sh` and lists all possible configs available to you.

Plain cargo works too with `TACOSHELL_CONFIG=path/to/yourapp.toml` if you dont want to use the Makefile.

**Good to know:**

- Make sure to change `app.binary`, otherwise you get a build warning and a warning in the app (i dont want conflicting tacoshell binaries becoming an issue!)
- A broken config fails the build and tells you whats wrong
- The binary needs at least the glibc of the machine you built it on. 
  - For older systems or arm64 add `GLIBC=2.31` and/or `TARGET=aarch64-unknown-linux-gnu`. 
  - Thist however needs [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) (`cargo install cargo-zigbuild --locked`) and [zig](https://ziglang.org/download/) in your PATH, without them it just builds for your own machine. zig 0.16 complains about a "deprecated linker optimization setting" every build, ignore it
- `--print-config` shows the baked config with all defaults, `--kiosk` starts kiosk mode

## Config

`tacoshell.toml` has every option in it, commented out, with its default and what it does, so look there. `examples/ssh.toml` is a working SSH setup to start from. 

Technically only `[app]` and either `[local]` or `[ssh]` are required (not both at the same time though!)

## What it can do

- **SSH mode**: login form with your ASCII logo, asks before trusting a new server key (and yells in red if it ever changes), reconnects by itself when the session dies or the network drops. Password login only for now
- **Passwords**: `credentials = "memory"` keeps them while the app runs. `"unsafe"` saves them to disk base64'd, which is **not** encryption, so don't ship that unless you know what you're doing
  - Secure Storage of em creds coming someday TM
- **Menu**: right click for restart / reinitialize / logout
- **Kiosk mode**: fullscreen for touchscreens, with a built in on screen keyboard and an optional PIN so randos walking by can't log it out
- **Local mode**: if the program isn't installed the app says so (put install hints in `text.program_missing`), and `local.depends` ends up in the .deb's dependencies

Linux is what it's built and used on. macOS should work (that's what `make pkg` is for) but isn't really tested, Windows has no build or installer yet, and Android/iOS will definetly for now only compile with SSH mode (if at all), havent tested that yet though.

## Hacking on it

```bash
cargo test                       # includes SSH tests against a server that runs inside the test
cargo run --example dev_server   # fake SSH server on 127.0.0.1:2222, password "taco"
make CONFIG=examples/ssh.toml && ./target/release/tacoshell
```

## Known gaps

- no scrollback, no selecting or copying text
- wide characters (CJK, most emoji) take one cell instead of two
- no bracketed paste, no cursor blink
- SSH: no key or agent auth, no host certificates
- Untested on anything but Linux sofar

## Credits

The terminal emulator is the same one hoardom's app wrapper had (partly from my scraped eguiemo software suite that will come out one day maybe and partially Stack Overflow, Any AI fixes have been marked in the codes comment, comments themselfs were briefly checked by an llm and expanded where it concidered them to be to vague).
