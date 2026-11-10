# inkline

**zsh-style autosuggestions and syntax highlighting for bash.**

If you like how zsh feels with zsh-autosuggestions and
zsh-syntax-highlighting but live in bash, inkline gives you the same
experience without switching shells, without a heavyweight line editor, and
without slowing your prompt down.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="demo/demo-dark.svg">
  <img alt="bash with inkline: commands turn green or red as you type, history suggestions appear in grey and are accepted with Right, Alt-F or Ctrl-Y" src="demo/demo-light.svg">
</picture>

## Features

- **Suggestions as you type.** The most recent matching command from your
  history appears in grey after the cursor. Accept all of it, or one word at a
  time.
- **Syntax highlighting.** Valid commands turn green and typos turn red before
  you press Enter. Strings, variables, paths, globs, redirections and comments
  each get their own colour.
- **Matches your theme.** inkline only uses your terminal's palette colours,
  so it looks right in any theme and changes when you switch themes.
- **Keeps bash as it is.** GNU readline still does the editing, so your
  inputrc, `bind` settings, Tab completion and fzf key bindings keep working.
- **Fast.** About 0.1 ms per keystroke, roughly 40–50× less work than the zsh
  plugins it imitates. See [Performance](#performance).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/ramtinJ95/inkline/main/install.sh | bash
```

Then open a new terminal and start typing.

The [script](install.sh) downloads the prebuilt library for your CPU (x86_64
or aarch64) from the latest release, checks it, installs it to
`~/.local/lib/bash/inkline.so` and adds the lines that load it to the end of
`~/.bashrc`. If you use ble.sh or a similar plugin, remove it from
`~/.bashrc`.

inkline is tested on Arch Linux and Omarchy. Other Linux distributions with
bash and GNU readline 8 may work, but haven't been tested yet.

To update, run the install command again. To uninstall, delete the inkline
lines from `~/.bashrc` and the file `~/.local/lib/bash/inkline.so`.

### Build from source

You need Rust 1.85 or newer.

```bash
git clone https://github.com/ramtinJ95/inkline ~/src/inkline
cd ~/src/inkline
cargo build --release
install -Dm755 target/release/libinkline.so ~/.local/lib/bash/inkline.so
```

Then add this to the end of `~/.bashrc`:

```bash
if [[ $- == *i* ]]; then
  enable -f ~/.local/lib/bash/inkline.so inkline
fi
```

## Usage

| Key | What it does |
| --- | --- |
| Right, End, Ctrl-F, Ctrl-E or Ctrl-Y at the end of the line | Accept the suggestion |
| Alt-F (or another forward-word motion) | Accept the next word |
| Up/Down, history search | Hide the suggestion until you edit the line |

`inkline off` and `inkline on` turn it off and on in the current shell.
`inkline status` shows whether it's active.

## Performance

CPU time per keystroke, typing real commands into each shell:

| | Median | 90th percentile |
| --- | ---: | ---: |
| bash | 96 µs | 145 µs |
| **bash + inkline** | **181 µs** | **266 µs** |
| zsh + zsh-autosuggestions + zsh-syntax-highlighting | 7,672 µs | 14,002 µs |

A command returns to the prompt in 34 ms with inkline (32 ms in plain bash,
71 ms in zsh). Loading inkline adds about 16 ms to shell startup.

<sub>838 keystrokes per shell on an AMD Ryzen 7 PRO 6850U (low-power mode),
bash 5.3 and zsh 5.9 with the same prompt and history. Run
[`bench/compare.sh`](bench/compare.sh) to measure on your machine.</sub>

## How it works

inkline is a bash loadable builtin written in Rust. Each time readline redraws
the line, inkline briefly appends the suggestion to it, so readline wraps and
erases the suggestion exactly like text you typed. inkline then colours the
characters in place. Before painting, it checks its idea of where each
character sits against readline's own cursor; if they disagree, it shows the
line without decorations instead of risking a garbled screen. If anything goes
wrong internally, inkline turns itself off and bash carries on as normal.

## Compared to zsh

inkline follows the defaults of zsh-autosuggestions 0.7.1 and
zsh-syntax-highlighting 0.8.0, with a few differences:

- Comments use the theme's muted grey instead of zsh's bold black, which many themes
  (Omarchy's included) render invisibly.
- After a paste from the kill ring (`yank`), inkline shows a suggestion for
  the new line instead of keeping the old one.
- vi `f`/`t` jumps don't accept suggestion text.
- zsh-only syntax such as `noglob` or global aliases doesn't exist in bash.

## Contributing

Bug reports and pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md)
covers building, the test suites, the benchmark and releases.

## License

[MIT](LICENSE)
