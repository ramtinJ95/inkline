#!/usr/bin/env bash
# Compare line-editor cost: plain bash vs bash+inkline vs zsh with
# zsh-autosuggestions + zsh-syntax-highlighting (the setup inkline replicates).
#
# zsh and the plugins come from the official Arch packages, verified with
# pacman-key and unpacked into a cache directory; nothing is installed.
#
#   bench/compare.sh [BASHRC]      # default BASHRC: ~/.bashrc (must load inkline)
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
cache=${XDG_CACHE_HOME:-$HOME/.cache}/inkline-bench
bashrc=${1:-$HOME/.bashrc}
mirror=${INKLINE_BENCH_MIRROR:-$(grep -m1 -oP '^Server = \K.*' /etc/pacman.d/mirrorlist | sed 's|\$repo|extra|; s|\$arch|x86_64|')}
pkgs=(zsh-5.9.2-1-x86_64 zsh-autosuggestions-0.7.1-1-any zsh-syntax-highlighting-0.8.0-2-any)
mkdir -p "$cache/pkgs" "$cache/root" "$cache/zdot"

for p in "${pkgs[@]}"; do
  for ext in pkg.tar.zst pkg.tar.zst.sig; do
    [[ -f $cache/pkgs/$p.$ext ]] || curl -sfL --max-time 120 -o "$cache/pkgs/$p.$ext" "$mirror/$p.$ext"
  done
  pacman-key --verify "$cache/pkgs/$p.pkg.tar.zst.sig" "$cache/pkgs/$p.pkg.tar.zst" 2>&1 | grep -q 'Good signature' \
    || { echo "signature check failed for $p" >&2; exit 1; }
  tar --zstd -xf "$cache/pkgs/$p.pkg.tar.zst" -C "$cache/root"
done

root=$cache/root
history=$cache/history
cp "${HISTFILE:-$HOME/.bash_history}" "$history" 2>/dev/null || : >"$history"
cat "$here/lines.txt" >>"$history"
cp "$history" "$history.zsh"

cat >"$cache/zdot/.zshenv" <<EOF
module_path=($root/usr/lib/zsh/5.9.2)
fpath=($root/usr/share/zsh/functions/*(/) \$fpath)
EOF
# Mirror of a typical macOS ~/.zshrc line-editor setup.
cat >"$cache/zdot/.zshrc" <<EOF
HISTFILE=$history.zsh; HISTSIZE=100000; SAVEHIST=100000
setopt SHARE_HISTORY
bindkey -e
source $root/usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh
bindkey '^y' autosuggest-accept
command -v starship >/dev/null && eval "\$(starship init zsh)"
command -v zoxide >/dev/null && eval "\$(zoxide init zsh)"
source $root/usr/share/zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh
[[ -f /usr/share/fzf/key-bindings.zsh ]] && source /usr/share/fzf/key-bindings.zsh
EOF

sed 's|^\( *\)enable -f .*inkline.*|\1:  # inkline disabled for the baseline|' "$bashrc" >"$cache/plain.bashrc"
grep -q 'inkline disabled' "$cache/plain.bashrc" || { echo "$bashrc does not load inkline with enable -f" >&2; exit 1; }

results=$cache/results.jsonl
: >"$results"
run() { python3 -I "$here/latency.py" --lines "$here/lines.txt" --json "$results" "$@" >/dev/null; }
run --name bash-plain --env HISTFILE="$history" -- bash --rcfile "$cache/plain.bashrc" -i
run --name bash+inkline --env HISTFILE="$history" -- bash --rcfile "$bashrc" -i
run --name zsh+plugins --env ZDOTDIR="$cache/zdot" -- "$root/usr/bin/zsh" -i

echo "CPU profile: $(cat /sys/firmware/acpi/platform_profile 2>/dev/null || echo unknown)"
python3 - "$results" <<'EOF'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
print(f"{'shell':14} | {'redraw done ms med/p90/max':>27} | {'CPU µs/key med/p90/max':>26} | {'burst µs/char':>13} | {'Enter→prompt ms':>15} | startup ms")
for r in rows:
    d, c, e = r["done_ms"], r["cpu_us_per_key"], r["enter_to_prompt_ms"]
    print(f"{r['name']:14} | {d['median']:>7.2f} /{d['p90']:>7.2f} /{d['max']:>7.2f} | {c['median']:>7.0f} /{c['p90']:>7.0f} /{c['max']:>7.0f} | "
          f"{r['cpu_us_per_key_burst']:>13.1f} | {e['median']:>15.0f} | {r['startup_ms']:>10.0f}")
EOF
