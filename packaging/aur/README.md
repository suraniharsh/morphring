# AUR packages

| Package | Contents | Status |
|---|---|---|
| `morphring-bin` | The `morphring` CLI from the GitHub release tarball | PKGBUILD ready, not yet published |
| `morphring-app-bin` | The ring app (Tauri) | Planned; a separate package later |

`morphring-bin` installs:

- `/usr/bin/morphring`
- `/usr/share/doc/morphring/` with `README.md`, `CHANGELOG.md` and `THIRD-PARTY-NOTICES.md`
- `/usr/share/licenses/morphring/` with `LICENSE` and `THIRD-PARTY-NOTICES.md`

The conversion engines (ImageMagick, FFmpeg, LibreOffice, Poppler, Ghostscript,
Pandoc) are `optdepends`. `morphring doctor` reports which ones are installed.

## Bumping the version

Do this only after the release tag is published and its assets are attached.

1. Set `pkgver` in `morphring-bin/PKGBUILD` to the new version and reset
   `pkgrel=1`.
2. Take the checksum of `morphring-v<version>-x86_64-linux.tar.gz` from the
   release's `SHA256SUMS.txt` and put it in `sha256sums`. Never use a checksum
   from a local build: the package must match what the release serves.

   ```sh
   gh release download v<version> -R suraniharsh/morphring -p SHA256SUMS.txt -O - \
     | grep x86_64-linux.tar.gz
   ```

3. Regenerate the metadata and check the package builds:

   ```sh
   cd packaging/aur/morphring-bin
   makepkg --printsrcinfo > .SRCINFO
   makepkg -f                      # in a scratch copy, to keep build output out of the repo
   pacman -Qlp morphring-bin-*.pkg.tar.zst
   ```

4. Commit `PKGBUILD` and `.SRCINFO` together.

## Publishing

Not done yet. Publishing needs the owner's AUR account: clone
`ssh://aur@aur.archlinux.org/morphring-bin.git`, copy in `PKGBUILD` and
`.SRCINFO`, commit and push. The source URL only works for anonymous users
once the repository and the release are public.
