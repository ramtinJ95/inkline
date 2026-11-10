"""Differential screen fuzzer: the same random key stream goes to bash with
inkline and to plain bash; after every key the visible text (suggestion cells
blanked) and the cursor must be identical.

When inkline accepts a suggestion (Right/End/C-e/C-f/C-y at the end of the
line, M-f partially) the accepted text is read off the screen and typed into
the plain shell, so both buffers stay equal.

    .venv/bin/python tests/fuzz_screen.py [--seeds 8] [--steps 300] [--rc FILE]
"""

import argparse
import fcntl
import os
import random
import re
import struct
import sys
import termios
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from screen_test import ESCAPED_RC, SIMPLE_RC, Term, is_ghost  # noqa: E402

HISTORY = [
    "git status --short",
    "git stash list",
    "ls -la /etc",
    "echo 'a b' | cat",
    "cat aa1-long-name.txt",
    "echo the quick brown fox jumps over the lazy dog and keeps running",
    "lsblk -o NAME,SIZE",
    "echo 日本語 🦀 done",
    "ls -la /etc/ssh",
]

TYPED = list("gitlsech /-.'\"$|;()=*a") + ["日", "🦀", "é", "é", "\x16\t", "\x16\x01"]
EDIT = ["\x7f", "\x02", "\x06", "\x01", "\x05", "\x1b[D", "\x1b[C", "\x1bb", "\x17", "\x0b", "\x15"]
OTHER = ["\x19", "\x1bf", "\x1b[A", "\x1b[B", "\t", "\x03", "\x0c", "RESIZE"]
ACCEPT_AT_END = {"\x1b[C", "\x06", "\x05", "\x19"}


POSSIBILITIES = re.compile(r"Display all \d+")


def comparable(t):
    """Screen text without suggestion cells. The completion count differs by
    one between the shells: the inkline shell has the `inkline` builtin."""
    return [POSSIBILITIES.sub("Display all N", row) for row in t.text_without_ghost()]


def ghost_cells(t):
    return t.ghost_cells()


def ghost_text(t):
    """Suggestion text in reading order, spaces included (pyte keeps a wide
    char in one cell and leaves the next one empty)."""
    return "".join(
        t.cell(y, x).data
        for y in range(t.screen.lines)
        for x in range(t.screen.columns)
        if is_ghost(t.cell(y, x))
    )


def at_suggestion_start(t):
    cells = ghost_cells(t)
    return bool(cells) and cells[0] == t.cur


def forward_word_prefix(text):
    """readline rl_forward_word over `text`: skip non-word, then word chars."""
    i = 0
    while i < len(text) and not text[i].isalnum():
        i += 1
    while i < len(text) and text[i].isalnum():
        i += 1
    return text[:i]


def resize(t, rows, cols):
    t.screen.resize(rows, cols)
    fcntl.ioctl(t.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def run(seed, steps, rc, log):
    rnd = random.Random(seed)
    d = None
    a = Term(rc, cols=40, rows=40, history=HISTORY)
    b = Term(rc, cols=40, rows=40, history=HISTORY, plugin=False, cwd=a.cwd)
    for name in ("aa1-long-name.txt", "aa2.txt", "abc.md"):
        open(os.path.join(a.cwd, name), "w").close()
    trail = []
    try:
        for step in range(steps):
            r = rnd.random()
            key = rnd.choice(TYPED) if r < 0.5 else rnd.choice(EDIT) if r < 0.85 else rnd.choice(OTHER)
            trail.append(key)
            if key == "RESIZE":
                cols = rnd.choice([20, 33, 40, 57, 80])
                for t in (a, b):
                    resize(t, 40, cols)
                    t.settle(0.15)
                continue
            if key == "\x0c":
                for t in (a, b):
                    t.send(key, quiet=0.08)
                continue
            mirror = key
            if ghost_cells(a):
                start = at_suggestion_start(a)
                if key in ACCEPT_AT_END and start:
                    mirror = ghost_text(a)  # inkline accepts: type the same into plain
                elif key == "\x19":
                    mirror = None  # C-y mid-line: inkline does nothing, plain would yank
                elif key == "\x1bf":
                    if not start:
                        trail.pop()
                        continue  # may run into the suggestion; skip
                    mirror = forward_word_prefix(ghost_text(a))
            elif key == "\x19":
                mirror = None
            # completion over thousands of command names takes a while to draw
            quiet = 0.4 if key == "\t" else 0.05
            a.send(key, quiet=quiet)
            if mirror:
                b.send(mirror, quiet=quiet)
            a.settle(0.03)
            b.settle(0.03)
            if comparable(a) != comparable(b) or a.cur != b.cur:
                raise AssertionError(
                    f"seed {seed}: diverged at step {step} after {key!r} (mirror {mirror!r})\n"
                    f"last keys: {trail[-12:]!r}\ninkline:\n{a.dump()}\nplain:\n{b.dump()}"
                )
        s = a.status()
        if "layout mismatches 0" not in s or "internal errors 0" not in s:
            raise AssertionError(f"seed {seed}: bad status\n{s}")
        log(f"seed {seed}: {steps} steps ok")
    finally:
        a.close()
        b.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", type=int, default=8)
    ap.add_argument("--first-seed", type=int, default=1)
    ap.add_argument("--steps", type=int, default=300)
    ap.add_argument("--rc", action="append", default=[])
    args = ap.parse_args()
    rcs = [("simple", SIMPLE_RC), ("escaped-multiline-prompt", ESCAPED_RC)] + [(os.path.basename(p), open(p).read()) for p in args.rc]
    failed = 0
    t0 = time.time()
    for name, rc in rcs:
        for seed in range(args.first_seed, args.first_seed + args.seeds):
            try:
                run(seed, args.steps, rc, lambda m: print(f"PASS {name} {m}", flush=True))
            except AssertionError as e:
                failed += 1
                print(f"FAIL {name}\n{e}", flush=True)
    print(f"{failed} failed in {time.time() - t0:.0f}s")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
