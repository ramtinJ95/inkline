# inkline

zsh-style autosuggestions and syntax highlighting for bash, and nothing else.

It is a bash loadable builtin written in Rust. GNU readline stays the line
editor (your inputrc, `bind`, Tab completion, fzf widgets keep working);
inkline decorates readline's output and follows the zsh plugins' rules:

- **Autosuggestions** as in zsh-autosuggestions 0.7.1 (default `history`
  strategy, `fg=8`): the newest history entry starting with the buffer, shown
  after the buffer wherever the cursor is, multi-line entries included.
  Right / End / Ctrl-F / Ctrl-E accept it when the cursor is at the end,
  Ctrl-Y (`autosuggest-accept`) too; forward-word motions (Alt-F, vi `w`/`e`/…)
  accept as far as they move; history navigation (Up/Down, history-search)
  clears it until you edit.
- **Syntax highlighting** with zsh-syntax-highlighting 0.8.0's `main`
  highlighter defaults applied to bash syntax: commands/aliases/functions/
  builtins green, unknown commands bold red, reserved words, strings and
  redirections yellow, `$var` and escapes inside quotes cyan, existing paths and
  path prefixes underlined, `*`/`?` blue, `$(…)`/backquote delimiters magenta,
  precommands (`sudo`, `command`, `exec`, `nice`, …) green underlined with their
  option arguments understood, comments `fg=black,bold`.

All colours are the terminal's 16 palette colours, so they follow the active
terminal theme (Omarchy writes it into the terminal's palette).

## Install

```bash
cargo build --release
install -Dm755 target/release/libinkline.so ~/.local/lib/bash/inkline.so
```

In `~/.bashrc` (instead of ble.sh), after any `bind`/inputrc setup:

```bash
if [[ $- == *i* ]]; then
  enable -f ~/.local/lib/bash/inkline.so inkline
fi
```

`inkline status` shows state, counters and per-keystroke cost; `inkline off` /
`inkline on` toggle it for the current shell. `inkline-accept`
(zsh's `autosuggest-accept`, bound to Ctrl-Y) is available to `bind`.

## How it works

- While readline redraws, the suggestion is temporarily part of the line
  buffer, so readline itself wraps, scrolls and erases it like typed text.
- inkline then recolours the cells. Their positions come from a
  reimplementation of readline 8.3's line layout (wrapping, wide characters,
  tabs, `^X` control characters, newlines), checked against readline's real
  cursor and bottom row before anything is painted; if they disagree the frame
  is redrawn without the suggestion, so it can never look like typed text.
  `inkline status` counts such "layout mismatches" (expected: 0).
- Accept-line, completion listings and other printing commands first redraw
  without the suggestion; Ctrl-C clears it, keeping readline's `^C`. Nothing
  grey ends up in scrollback.
- Every entry point runs under `catch_unwind`; an internal error disables
  inkline for that shell and readline carries on. The library pins itself so
  `enable -d inkline` cannot leave dangling pointers in readline. Requires
  readline 8.x (checked at load).

## Tests

```bash
cargo test                                  # highlighter, suggestions, painting
python3 -m venv .venv && .venv/bin/pip install pyte
.venv/bin/python tests/screen_test.py       # real bash in a pty, checked through a terminal emulator
.venv/bin/python tests/screen_test.py --rc ~/some-rc-without-ble   # plus your own prompt/inputrc
cargo run --release --example bench -- ~/.bash_history
```

The screen tests include a randomized edit sequence compared frame by frame
against plain readline (text and cursor must match exactly).

## Differences from zsh that remain

- `yank`/`yank-pop`: zsh-autosuggestions leaves the previous suggestion
  displayed (they are "ignore" widgets); inkline fetches one for the new buffer.
- vi `f`/`t` character search is not a partial-accept motion.
- Highlighting applies zsh-syntax-highlighting's rules to bash syntax; zsh-only
  constructs (`noglob`, `=cmd`, global/suffix aliases) do not exist in bash.
