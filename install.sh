#!/usr/bin/env bash
# Install inkline from its latest GitHub release:
#
#   curl -fsSL https://raw.githubusercontent.com/ramtinJ95/inkline/main/install.sh | bash
#
# Downloads the prebuilt library for this CPU, checks its checksum and that
# this bash can load it, installs it to ~/.local/lib/bash/inkline.so and adds
# the lines that load it to ~/.bashrc (unless they are already there).
#
#   INKLINE_VERSION=v0.1.0   install that release instead of the latest
#   INKLINE_NO_BASHRC=1      leave ~/.bashrc alone
set -euo pipefail

repo=ramtinJ95/inkline
version=${INKLINE_VERSION:-latest}
dest_dir=$HOME/.local/lib/bash
dest=$dest_dir/inkline.so
rc=$HOME/.bashrc
snippet='# inkline: zsh-style suggestions and highlighting (https://github.com/ramtinJ95/inkline)
if [[ $- == *i* ]] && [[ -f ~/.local/lib/bash/inkline.so ]]; then
  enable -f ~/.local/lib/bash/inkline.so inkline
fi'

die() {
  printf 'inkline: %s\n' "$*" >&2
  exit 1
}

[[ $(uname -s) == Linux ]] || die "only Linux is supported"
case $(uname -m) in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "no prebuilt library for $(uname -m); see https://github.com/$repo#build-from-source" ;;
esac
for tool in curl sha256sum bash; do
  command -v "$tool" >/dev/null || die "$tool is required"
done

if [[ $version == latest ]]; then
  base=https://github.com/$repo/releases/latest/download
else
  base=https://github.com/$repo/releases/download/$version
fi
asset=inkline-$arch-linux.so

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset ($version)"
curl -fsSL -o "$tmp/$asset" "$base/$asset" || die "download failed: $base/$asset"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
(cd "$tmp" && grep -F "  $asset" SHA256SUMS | sha256sum --check --status) || die "checksum mismatch for $asset"

if ! bash --norc --noprofile -c 'enable -f "$1" inkline' _ "$tmp/$asset" 2>"$tmp/err"; then
  die "your bash cannot load inkline: $(cat "$tmp/err")"
fi

mkdir -p "$dest_dir"
# Rename into place: running shells keep the library they already loaded.
install -m755 "$tmp/$asset" "$dest_dir/.inkline.so.new"
mv -f "$dest_dir/.inkline.so.new" "$dest"
echo "Installed $dest"

if [[ ${INKLINE_NO_BASHRC:-} == 1 ]]; then
  printf 'Left ~/.bashrc alone. To load inkline, add:\n\n%s\n\n' "$snippet"
elif grep -qsF 'inkline.so' "$rc"; then
  # shellcheck disable=SC2088 # a path shown to the user, not expanded
  echo "~/.bashrc already loads inkline"
else
  printf '\n%s\n' "$snippet" >>"$rc"
  echo "Added inkline to ~/.bashrc"
fi
if grep -qsE '^[^#]*ble\.sh' "$rc"; then
  echo "Note: ~/.bashrc also loads ble.sh. Remove it; it replaces readline, so inkline can't run alongside it." >&2
fi

echo "Open a new terminal to start using inkline."
