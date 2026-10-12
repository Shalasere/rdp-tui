# Release binaries and Arch packaging

The Rust frontend is distributed separately from FreeRDP. Release binaries
target Linux x86_64 and ARM64 using static musl builds; FreeRDP 3 and its display
dependencies must be installed through the user's distribution.

## GitHub releases

The `Release binaries` workflow builds and tests both targets on native GitHub
runners. Pushes to `main`, relevant pull requests, and manual runs produce
downloadable Actions artifacts. Version tag pushes additionally run the full
Python/Rust CI gate and publish a GitHub release only after every check and both
builds succeed.

Each `.tar.gz` contains the executable, license, README, installation instructions,
and `BUILD-INFO.json` with the source commit, compiler, target, and binary hash.
The release includes `SHA256SUMS` for both archives. Packaging rejects executables
with the wrong architecture or a dynamic loader. The packaging helper requires
Python 3.11 or newer; CI uses Python 3.13.

To make a release:

1. Set the Rust version in `Cargo.toml` and the root package entry in `Cargo.lock`.
2. Run `make test lint format-check` and commit the changes.
3. Push the commit and wait for CI and both portable builds to pass.
4. Create a matching tag, for example:

   ```sh
   git tag -a v0.2.0-alpha.2 -m 'Release v0.2.0-alpha.2'
   git push origin v0.2.0-alpha.2
   ```

The tag must exactly match `v` plus the Cargo version. Prerelease tags containing
`-` are marked as GitHub prereleases. A draft is used while assets are uploaded,
then published once complete. Published binaries are never replaced on a rerun;
use a new version if the contents change. Upload failures leave a draft that the
same workflow can retry.

For a local x86_64 packaging check:

```sh
rustup target add x86_64-unknown-linux-musl
cargo test --locked --all-targets --target x86_64-unknown-linux-musl
cargo build --locked --release --target x86_64-unknown-linux-musl
python scripts/release.py bundle --target x86_64-unknown-linux-musl
python scripts/release.py checksums
(cd dist && sha256sum -c SHA256SUMS)
```

## AUR

`packaging/aur/rdp-tui-git/` contains a source-building development package recipe.
It installs the Rust executable under `/usr/bin/rdp-tui`; run `rdp-tui tui` to
open the interface. FreeRDP is required; OpenSSH, libsecret's `secret-tool`, and
Hyprland integration are optional. The recipe builds and tests the locked Rust
dependencies before packaging, and includes the license and README.

This repository's recipe is preparation for an AUR submission, not a declaration
that an AUR package is published. Before submitting it:

1. Add the maintainer's AUR username/contact to the `PKGBUILD` comment.
2. Check again that the package name is available in the AUR and is not in the
   official repositories.
3. Copy the recipe to a separate packaging directory, build with `makepkg`, and
   regenerate `.SRCINFO` with `makepkg --printsrcinfo > .SRCINFO`.
4. Commit `PKGBUILD` and `.SRCINFO` to the maintainer's AUR Git repository and push
   using the SSH key registered to that AUR account.

An AUR release package (`rdp-tui`) or prebuilt package (`rdp-tui-bin`) can be added
later. The `-git` recipe deliberately follows upstream development and uses the
VCS source mechanism rather than pretending a development snapshot is a fixed
release.
