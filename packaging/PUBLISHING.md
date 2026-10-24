<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Package Publishing Guide

Instructions for submitting SSG to platform-specific package repositories.

## Arch Linux (AUR)

The PKGBUILD is at `packaging/arch/PKGBUILD`.

### First-time submission

1. Create an AUR account at https://aur.archlinux.org
2. Clone the AUR package base:
   ```sh
   git clone ssh://aur@aur.archlinux.org/ssg.git aur-ssg
   cd aur-ssg
   ```
3. Copy `packaging/arch/PKGBUILD` into the AUR repo
4. Generate `.SRCINFO`:
   ```sh
   makepkg --printsrcinfo > .SRCINFO
   ```
5. Commit and push:
   ```sh
   git add PKGBUILD .SRCINFO
   git commit -m "Initial upload: ssg 0.0.37"
   git push
   ```

### Updating

1. Update `pkgver` and checksums in `PKGBUILD`
2. Regenerate `.SRCINFO`:
   ```sh
   makepkg --printsrcinfo > .SRCINFO
   ```
3. Commit and push

### Testing

```sh
cd packaging/arch
makepkg -si    # builds and installs locally
```

## Homebrew, Scoop and winget

The files under `packaging/homebrew`, `packaging/scoop` and
`packaging/winget` are templates. At release time,
`scripts/render-packaging.sh` fills in the version and the SHA-256 of
each archive from the `.sha256` files the build wrote, and
`release.yml` attaches the results to the release:

| Asset | Channel |
| :--- | :--- |
| `ssg.rb` | Homebrew formula |
| `ssg.json` | Scoop manifest |
| `ssg.winget.yaml` | winget manifest |

To render them by hand for a published release, put its `.sha256`
files in a directory and run:

```sh
scripts/render-packaging.sh <version> <dir-with-sha256-files> <out-dir>
```

### Homebrew

The tap at https://github.com/sebastienrousseau/homebrew-tap copies
`ssg.rb` from the latest release into `Formula/ssg.rb`. Users install
with:

```sh
brew install sebastienrousseau/tap/ssg
```

Homebrew no longer installs a formula from a URL or a local file
outside a tap, so the tap is the supported route.

### Scoop

Users install straight from the release's manifest:

```powershell
scoop install https://github.com/sebastienrousseau/static-site-generator/releases/latest/download/ssg.json
```

The manifest's `checkver` and `autoupdate` sections (hash from the
archive's `.sha256` file) let a Scoop bucket keep it current.

### winget

SSG is not in `microsoft/winget-pkgs` yet. To submit a release:

1. Fork https://github.com/microsoft/winget-pkgs
2. Copy the release's `ssg.winget.yaml` to
   `manifests/s/sebastienrousseau/ssg/<version>/sebastienrousseau.ssg.yaml`
3. Validate: `winget validate manifests/s/sebastienrousseau/ssg/<version>/`
4. Submit a PR to `microsoft/winget-pkgs`

## Debian (.deb)

The build script is at `packaging/deb/build.sh`.

### Building

```sh
./packaging/deb/build.sh
```

This produces `ssg_<version>_amd64.deb`.

### Distribution

Attach the `.deb` file to the GitHub Release. Users install with:

```sh
sudo dpkg -i ssg_0.0.37_amd64.deb
```

## Release Checklist

1. Update version in `Cargo.toml`
2. Update `packaging/arch/PKGBUILD` (pkgver, checksums)
3. Build `.deb` with `packaging/deb/build.sh`
4. Tag the release and push; `release.yml` renders `ssg.rb`, `ssg.json`
   and `ssg.winget.yaml` and attaches them with the binaries
5. Submit PRs to AUR and winget-pkgs
