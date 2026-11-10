# Contributing

## Build

```bash
cargo build --release
install -Dm755 target/release/libinkline.so ~/.local/lib/bash/inkline.so
```

New shells load the new build; `inkline status` shows its counters, and
"layout mismatches" and "internal errors" should stay at 0.

## Tests

```bash
cargo test --release
```

Unit tests plus property fuzzing in `tests/fuzz.rs`: random and hostile input
(invalid UTF-8, control, zero-width and wide characters, narrow screens) must
not panic, layouts must stay inside the screen, and repaints replayed through a
small terminal model must put every cell in place. For a longer run:
`INKLINE_FUZZ_ITERS=200000 cargo test --release --test fuzz`, with
`INKLINE_FUZZ_SEED=n` to vary it.

The screen tests drive a real bash through a pty and check the result in the
[pyte](https://github.com/selectel/pyte) terminal emulator:

```bash
python3 -m venv .venv && .venv/bin/pip install pyte
.venv/bin/python tests/screen_test.py
.venv/bin/python tests/fuzz_screen.py --seeds 8 --steps 300
```

`fuzz_screen.py` sends the same random keys (typing, editing, history, Tab,
Ctrl-C, Ctrl-L, resizes) to bash with and without inkline; after every key the
screens must match, suggestion aside. Both scripts take `--rc FILE` to also run
with your own prompt and inputrc (a bashrc that doesn't load inkline or
another line editor), and `INKLINE_SO=path` tests a library other than
`target/release/libinkline.so`, such as a release download.

## Demo and benchmark

- `.venv/bin/python demo/record.py` re-records the README animation from a
  scripted session in a throwaway HOME.
- `bench/compare.sh [BASHRC]` measures plain bash, bash + inkline and zsh with
  zsh-autosuggestions and zsh-syntax-highlighting (from the Arch packages,
  unpacked into `~/.cache/inkline-bench`, not installed). The bashrc must load
  inkline; it is run once with and once without it.

## Releasing

1. Bump `version` in `Cargo.toml`, run `cargo build` to update `Cargo.lock`,
   and commit.
2. Tag and push: `git tag -a v0.2.0 -m "inkline 0.2.0" && git push origin v0.2.0`.

The release workflow builds the library for x86_64 and aarch64 on AlmaLinux 8
(glibc 2.28), runs the tests, and attaches `inkline-<arch>-linux.so` and
`SHA256SUMS` to the GitHub release, creating it if needed. `install.sh`
installs from the latest release.
