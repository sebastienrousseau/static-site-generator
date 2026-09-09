<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Installation

SSG supports multiple installation methods across macOS, Linux, and Windows.

## Requirements

- **Rust 1.88.0+** (only for building from source or `cargo install`)
- No runtime dependencies for prebuilt binaries

## Quick Install (Prebuilt Binary)

The install script auto-detects your OS and architecture, downloads the correct binary from GitHub Releases, verifies the SHA256 checksum, and installs to `~/.local/bin`.

```sh
curl -fsSL https://raw.githubusercontent.com/sebastienrousseau/static-site-generator/main/scripts/install.sh | sh
```

Add `~/.local/bin` to your `PATH` if it is not already there:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

## Homebrew (macOS / Linux)

```sh
brew install --formula https://raw.githubusercontent.com/sebastienrousseau/static-site-generator/main/packaging/homebrew/ssg.rb
```

## Cargo

```sh
cargo install ssg
```

This builds from source and installs to `~/.cargo/bin`.

## Debian / Ubuntu (.deb)

Download the `.deb` package from the [latest release](https://github.com/sebastienrousseau/static-site-generator/releases/latest):

```sh
sudo dpkg -i ssg_0.0.37_amd64.deb
```

Or build the `.deb` yourself:

```sh
./packaging/deb/build.sh
```

## Arch Linux (AUR / PKGBUILD)

Using an AUR helper:

```sh
yay -S ssg
```

Or build manually with the PKGBUILD:

```sh
cd packaging/arch
makepkg -si
```

## Windows (Scoop)

```powershell
scoop bucket add ssg https://github.com/sebastienrousseau/static-site-generator
scoop install ssg
```

The Scoop manifest lives at `packaging/scoop/ssg.json`.

## Windows (winget)

```powershell
winget install sebastienrousseau.ssg
```

The winget manifest lives at `packaging/winget/ssg.yaml`.

## Windows (Manual)

Download the `.zip` from the [latest release](https://github.com/sebastienrousseau/static-site-generator/releases/latest), extract `ssg.exe`, and add its directory to your `PATH`.

## Build from Source

```sh
git clone https://github.com/sebastienrousseau/static-site-generator.git
cd static-site-generator
make init    # installs toolchain, hooks, builds
cargo test --lib
```

The `make init` target installs rustfmt, clippy, cargo-deny, sets up git hooks, and runs a full build.

## Use as a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
ssg = "0.0.60"
```

## WSL2 / GitHub Codespaces

The dev server binds to `127.0.0.1` by default. In WSL2 or Codespaces, set `SSG_HOST` to bind to all interfaces so the forwarded port is reachable:

```sh
export SSG_HOST=0.0.0.0
ssg -c content -o public -t templates -s public
```

## Verify Installation

```sh
ssg --version
```

## Next Steps

- [Quick Start](quick-start.md) — scaffold and build your first site
- [CLI Reference](cli.md) — all flags and options
