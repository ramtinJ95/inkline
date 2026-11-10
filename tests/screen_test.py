"""Screen-level tests: drive a real interactive bash through a pty into the
pyte terminal emulator and check what a user would see.

    python3 -m venv .venv && .venv/bin/pip install pyte
    cargo build --release && .venv/bin/python tests/screen_test.py [-k name] [--rc FILE]

--rc FILE adds a run of the realistic tests on top of that rc file (e.g. your
own bashrc minus the line editor plugin) to check multi-line/escaped prompts.
"""

import argparse
import fcntl
import os
import pty
import random
import select
import shutil
import struct
import sys
import tempfile
import termios
import time
import traceback

import pyte

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SO = os.environ.get("INKLINE_SO") or os.path.join(ROOT, "target", "release", "libinkline.so")
GHOST_FG = "brightblack"


def is_ghost(cell):
    """Suggestion cells: bright black, not bold (comments are bold bright black)."""
    return cell.fg == GHOST_FG and not cell.bold

HISTORY = [
    "ls -la /etc",
    "echo hello-world-123",
    "git status --short",
    "cat aa1-long-name.txt",
    "echo the quick brown fox jumps over the lazy dog and keeps running far away",
]


class Term:
    def __init__(self, rc_body, cols=60, rows=24, history=HISTORY, plugin=True, cwd=None, extra_history=()):
        self.dir = tempfile.mkdtemp(prefix="inkline-test-")
        self.cwd = cwd or self.dir
        hist = os.path.join(self.dir, "hist")
        with open(hist, "w") as f:
            f.write("\n".join(history) + "\n")
        rc = os.path.join(self.dir, "rc")
        with open(rc, "w") as f:
            f.write(rc_body)
            f.write(f"\nHISTFILE={hist}; history -c; history -r\n")
            for entry in extra_history:
                f.write(f"history -s {entry}\n")
            if plugin:
                f.write(f"enable -f {SO} inkline\n")
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ.update(TERM="xterm-256color", LANG="C.UTF-8", LC_ALL="C.UTF-8", INPUTRC="/dev/null")
            os.chdir(self.cwd)
            os.execvp("bash", ["bash", "--noprofile", "--rcfile", rc, "-i"])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.settle(1.0)

    def settle(self, quiet=0.12, limit=5.0):
        end = time.time() + limit
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], quiet)
            if not r:
                return
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return
            if not data:
                return
            if b"\x1b[6n" in data:
                os.write(self.fd, f"\x1b[{self.screen.cursor.y + 1};{self.screen.cursor.x + 1}R".encode())
            self.stream.feed(data)

    def send(self, keys, quiet=0.12):
        os.write(self.fd, keys.encode() if isinstance(keys, str) else keys)
        self.settle(quiet)

    def type(self, text):
        for ch in text:
            self.send(ch, quiet=0.04)
        self.settle()

    def row(self, y):
        return self.screen.display[y]

    def cell(self, y, x):
        return self.screen.buffer[y][x]

    @property
    def cur(self):
        return self.screen.cursor.y, self.screen.cursor.x

    def find(self, text):
        """Bottom-most occurrence (the live line), as (row, screen column).
        Wide characters occupy two cells in pyte, the second one empty."""
        for y in reversed(range(self.screen.lines)):
            cols = [x for x in range(self.screen.columns) if self.cell(y, x).data != ""]
            line = "".join(self.cell(y, x).data for x in cols)
            i = line.find(text)
            if i >= 0:
                return y, cols[i]
        raise AssertionError(f"{text!r} not on screen:\n{self.dump()}")

    def ghost_cells(self):
        return [(y, x) for y in range(self.screen.lines) for x in range(self.screen.columns)
                if is_ghost(self.cell(y, x)) and self.cell(y, x).data != " "]

    def text_without_ghost(self):
        rows = []
        for y in range(self.screen.lines):
            rows.append("".join(" " if is_ghost(self.cell(y, x)) else self.cell(y, x).data
                                for x in range(self.screen.columns)).rstrip())
        return rows

    def status(self):
        """`inkline status` output, unwrapped (rows are joined without separators)."""
        self.send("\x05\x15inkline status\r", quiet=0.3)  # End, kill line, run
        return "".join(self.screen.display)

    def dump(self):
        return "\n".join(f"{y:2}|{line}|" for y, line in enumerate(self.screen.display))

    def close(self):
        try:
            os.write(self.fd, b"\x15exit\r")
            self.settle(0.2, 1.0)
        except OSError:
            pass
        try:
            os.kill(self.pid, 9)
            os.waitpid(self.pid, 0)
        except (OSError, ChildProcessError):
            pass
        shutil.rmtree(self.dir, ignore_errors=True)


SIMPLE_RC = "PS1='$ '\nPROMPT_COMMAND=\n"
ESCAPED_RC = r"""PS1='\[\e[34m\]dir \[\e[0m\]\n\[\e[35m\]❯\[\e[0m\] '
PROMPT_COMMAND=
"""

TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn


def styled(t, y, x, n):
    return [t.cell(y, x + i) for i in range(n)]


def assert_eq(a, b, t, msg=""):
    if a != b:
        raise AssertionError(f"{msg}\n  got:  {a!r}\n  want: {b!r}\n{t.dump()}")


@test
def highlight_command_options_path(rc):
    t = Term(rc)
    try:
        t.type("ls -la /etc")
        y, x = t.find("ls -la /etc")
        assert all(c.fg == "green" for c in styled(t, y, x, 2)), t.dump()
        assert all(c.fg == "default" and not c.underscore for c in styled(t, y, x + 3, 3)), t.dump()
        assert all(c.underscore for c in styled(t, y, x + 7, 4)), t.dump()
        assert_eq(t.cur, (y, x + 11), t, "cursor at end")
    finally:
        t.close()


@test
def unknown_command_is_bold_red(rc):
    t = Term(rc)
    try:
        t.type("lsqqq")
        y, x = t.find("lsqqq")
        assert all(c.fg == "red" and c.bold for c in styled(t, y, x, 5)), t.dump()
    finally:
        t.close()


@test
def quotes_and_substitution(rc):
    t = Term(rc)
    try:
        t.type('echo "hi $HOME" $(ls)')
        y, x = t.find('echo "hi $HOME"')
        assert t.cell(y, x + 5).fg == "brown", "double quote is yellow"  # pyte names SGR 33 'brown'
        assert t.cell(y, x + 9).fg == "cyan", "$HOME inside quotes"
        dx = t.row(y).find("$(ls)")
        assert t.cell(y, dx).fg == "magenta" and t.cell(y, dx + 2).fg == "green", t.dump()
        t.type(" # a note")
        cx = t.row(y).find("# a note")
        assert all(c.fg == GHOST_FG and c.bold for c in styled(t, y, cx, 8)), "comment: bold bright black"
        assert not t.ghost_cells(), t.dump()
    finally:
        t.close()


@test
def ghost_suggestion_and_right_arrow_accept(rc):
    t = Term(rc)
    try:
        t.type("echo hel")
        y, x = t.find("echo hello-world-123")
        assert_eq(t.cur, (y, x + 8), t, "cursor stays after typed text")
        assert all(t.cell(y, x + 8 + i).fg == GHOST_FG for i in range(12)), t.dump()
        t.send("\x1b[C")
        assert not t.ghost_cells(), "accepted: no ghost left\n" + t.dump()
        assert_eq(t.cur, (y, x + 20), t, "cursor at end after accept")
        t.send("\r", quiet=0.3)
        assert any(line.strip() == "hello-world-123" for line in t.screen.display), t.dump()
    finally:
        t.close()


@test
def ctrl_y_and_partial_accept(rc):
    t = Term(rc)
    try:
        t.type("git s")
        t.send("\x1bf")  # M-f: accept next word only
        y, x = t.find("git status")
        assert_eq(t.cur, (y, x + 10), t, "partial accept up to end of word")
        assert t.ghost_cells(), "rest still suggested\n" + t.dump()
        t.send("\x19")  # C-y
        assert not t.ghost_cells(), t.dump()
        assert_eq(t.cur, (y, x + len("git status --short")), t)
    finally:
        t.close()


@test
def ghost_never_reaches_scrollback(rc):
    t = Term(rc)
    try:
        t.type("echo hel")
        t.send("\r", quiet=0.3)
        t.type("echo hel")
        t.send("\x03", quiet=0.3)  # Ctrl-C with a ghost showing
        text = "\n".join(t.screen.display)
        assert "hello-world" not in text.replace("echo hello-world-123", ""), t.dump()
        assert len(t.ghost_cells()) == 0, "no ghost anywhere after Enter and Ctrl-C\n" + t.dump()
    finally:
        t.close()


@test
def suggestion_stays_while_cursor_moves(rc):
    """zsh-autosuggestions: moving the cursor keeps the suggestion; accept
    widgets only accept with the cursor at the end."""
    t = Term(rc)
    try:
        t.type("echo hel")
        y, x = t.find("echo hello-world-123")
        t.send("\x02\x02")  # C-b C-b
        assert_eq(t.cur, (y, x + 6), t)
        assert_eq(len(t.ghost_cells()), 12, t, "suggestion still shown after the buffer")
        t.send("\x1b[C")  # Right in the middle: plain cursor motion
        assert_eq(t.cur, (y, x + 7), t)
        assert_eq(len(t.ghost_cells()), 12, t)
        t.send("\x05")  # End: moves to the end, does not accept yet
        assert_eq(t.cur, (y, x + 8), t)
        assert_eq(len(t.ghost_cells()), 12, t)
        t.send("\x05")  # End at the end: accepts
        assert not t.ghost_cells(), t.dump()
        assert_eq(t.cur, (y, x + 20), t)
    finally:
        t.close()


@test
def history_navigation_clears_suggestion(rc):
    t = Term(rc)
    try:
        t.type("git")
        assert t.ghost_cells(), t.dump()
        t.send("\x1b[A", quiet=0.2)  # Up: previous-history or history-search-backward
        assert not t.ghost_cells(), "CLEAR widget\n" + t.dump()
        t.send("x\x7f")  # edit (and undo the edit): suggestion is fetched again
        y, _ = t.cur
        assert t.row(y).strip(), t.dump()
    finally:
        t.close()


@test
def newest_match_equal_to_buffer_means_no_suggestion(rc):
    t = Term(rc, history=["ls -la /etc --color", "ls -la /etc"])
    try:
        t.type("ls -la /etc")
        assert not t.ghost_cells(), "newest entry equals the buffer\n" + t.dump()
        t.send("\x7f")
        assert t.ghost_cells(), t.dump()
    finally:
        t.close()


@test
def multiline_history_suggestion(rc):
    t = Term(rc, extra_history=["$'for i in 1 2\\ndo echo item-$i\\ndone'"])
    try:
        t.type("for i")
        ty, tx = t.find("for i in 1 2")
        t.find("do echo item-$i")
        assert t.cell(ty, tx + 6).fg == GHOST_FG, t.dump()
        dy, dx = t.find("do echo item-$i")
        assert dy == ty + 1 and t.cell(dy, dx).fg == GHOST_FG, t.dump()
        t.send("\x19")  # C-y accepts all three lines
        assert not t.ghost_cells(), t.dump()
        t.send("\r", quiet=0.4)
        assert any(line.strip() == "item-2" for line in t.screen.display), t.dump()
    finally:
        t.close()


@test
def backspace_updates_ghost(rc):
    t = Term(rc)
    try:
        t.type("git sx")
        assert not t.ghost_cells(), t.dump()
        t.send("\x7f")
        assert t.ghost_cells(), t.dump()
        y, x = t.find("git status --short")
    finally:
        t.close()


@test
def wrapped_line_highlight_positions(rc):
    t = Term(rc, cols=30)
    cmd = "echo aaaaaaaa bbbbbbbbbb cccccccccc ls"
    try:
        t.type(cmd)
        want = Term(rc, cols=30, plugin=False, cwd=t.cwd)
        try:
            want.type(cmd)
            assert_eq(t.text_without_ghost(), want.text_without_ghost(), t, "same text as plain readline")
            assert_eq(t.cur, want.cur, t, "same cursor as plain readline")
        finally:
            want.close()
        y, x = t.find("echo")
        assert all(c.fg == "green" for c in styled(t, y, x, 4)), t.dump()
    finally:
        t.close()


@test
def long_suggestion_wraps_and_accepts_like_typed_text(rc):
    full = "echo the quick brown fox jumps over the lazy dog and keeps running far away"
    t = Term(rc, cols=30)
    try:
        t.type("echo the quick")
        rows = {y for y, _ in t.ghost_cells()}
        assert len(rows) >= 2, "suggestion wraps onto following rows\n" + t.dump()
        assert "…" not in "".join(t.screen.display), t.dump()
        t.send("\x1b[C")
        assert not t.ghost_cells(), t.dump()
        plain = Term(rc, cols=30, plugin=False, cwd=t.cwd)
        try:
            plain.type(full)
            assert_eq(t.text_without_ghost(), plain.text_without_ghost(), t, "accepting == typing it")
            assert_eq(t.cur, plain.cur, t)
        finally:
            plain.close()
    finally:
        t.close()


@test
def wrapped_suggestion_cleared_on_ctrl_c_and_enter(rc):
    t = Term(rc, cols=30)
    try:
        t.type("echo the quick")
        t.send("\x03", quiet=0.3)
        assert not t.ghost_cells(), "Ctrl-C\n" + t.dump()
        t.type("echo the quick")
        t.send("\r", quiet=0.3)
        assert not t.ghost_cells(), "Enter\n" + t.dump()
        assert "jumps" not in "\n".join(t.screen.display), t.dump()
    finally:
        t.close()


@test
def suggestion_at_bottom_of_screen_scrolls_cleanly(rc):
    full = "echo the quick brown fox jumps over the lazy dog and keeps running far away"
    t = Term(rc, cols=30, rows=8)
    plain = Term(rc, cols=30, rows=8, plugin=False, cwd=t.cwd)
    try:
        for term in (t, plain):
            term.send("seq 1 30\r", quiet=0.4)
        t.type("echo the quick")
        assert len({y for y, _ in t.ghost_cells()}) >= 2, t.dump()
        t.send("\x1b[C")
        plain.type(full)
        assert_eq(t.text_without_ghost(), plain.text_without_ghost(), t, "after accept at the bottom")
        assert_eq(t.cur, plain.cur, t)
        # Ctrl-C at the bottom with a wrapped suggestion: ^C stays, the
        # suggestion goes, the prompt follows on the next row. (The terminal
        # has scrolled for the suggestion, as it does in zsh.)
        t.send("\x03", quiet=0.3)
        t.type("echo the quick")
        t.send("\x03", quiet=0.3)
        assert not t.ghost_cells(), t.dump()
        y, _ = t.find("echo the quick^C")
        assert y < t.cur[0] <= y + 3, "new prompt right below (prompts may start with blank/info rows)\n" + t.dump()
    finally:
        t.close()
        plain.close()


@test
def completion_listing_leaves_no_ghost(rc):
    d = tempfile.mkdtemp(prefix="inkline-cwd-")
    for name in ("aa1-long-name.txt", "aa2.txt", "aa3.txt"):
        open(os.path.join(d, name), "w").close()
    t = Term(rc, cwd=d)
    try:
        t.type("cat aa")
        assert t.ghost_cells(), t.dump()
        t.send("\t")
        t.send("\t", quiet=0.3)
        assert not t.ghost_cells() or all(
            t.row(y).find("cat aa") >= 0 for y, _ in t.ghost_cells()), "only the live line may show a ghost\n" + t.dump()
        assert "aa2.txt" in "\n".join(t.screen.display), "listing shown\n" + t.dump()
    finally:
        t.close()
        shutil.rmtree(d, ignore_errors=True)


@test
def tab_sequence_matches_plain(rc):
    """Tab behaviour (complete / menu-complete cycling) is unchanged by inkline."""
    d = tempfile.mkdtemp(prefix="inkline-cwd-")
    for name in ("aa1-long-name.txt", "aa2.txt", "aa3.txt"):
        open(os.path.join(d, name), "w").close()
    a = Term(rc, cwd=d)
    b = Term(rc, cwd=d, plugin=False)
    try:
        for keys in ("cat aa", "\t", "\t", "\t", "\x1b[Z", "\t"):
            for t in (a, b):
                t.type(keys) if len(keys) > 1 else t.send(keys, quiet=0.25)
            assert_eq(a.text_without_ghost(), b.text_without_ghost(), a, f"after {keys!r}")
            assert_eq(a.cur, b.cur, a, f"cursor after {keys!r}")
    finally:
        a.close()
        b.close()
        shutil.rmtree(d, ignore_errors=True)


@test
def clear_screen_redraws_decorated(rc):
    t = Term(rc)
    try:
        t.type("echo hel")
        t.send("\x0c", quiet=0.3)
        y, x = t.find("echo hel")
        assert y <= 2, "line redrawn at the top\n" + t.dump()
        assert t.cell(y, x).fg == "green", t.dump()
        assert all(gy == y for gy, _ in t.ghost_cells()) and t.ghost_cells(), t.dump()
    finally:
        t.close()


@test
def reverse_search_is_left_alone(rc):
    t = Term(rc)
    try:
        # readline's own search on a spare key (Ctrl-R may belong to fzf)
        t.send("bind '\"\\C-xr\": reverse-search-history'\r", quiet=0.3)
        t.send("\x18r")
        t.type("hello")
        assert "reverse-i-search" in "\n".join(t.screen.display), t.dump()
        assert not t.ghost_cells(), t.dump()
        t.send("\x07")  # C-g
        t.type("ls")
        y, x = t.find("ls")
        assert t.cell(y, x).fg == "green", t.dump()
    finally:
        t.close()


@test
def bind_x_widgets(rc):
    """The bash path fzf's Ctrl-R/Ctrl-T use: a `bind -x` command that rewrites
    READLINE_LINE, and one that prints output. (fzf itself cannot run under
    this emulator, with or without inkline.)"""
    t = Term(rc)
    try:
        t.send("bind -x '\"\\C-xf\": READLINE_LINE=\"echo hello-world-123\"; READLINE_POINT=${#READLINE_LINE}'\r", quiet=0.3)
        t.send("bind -x '\"\\C-xp\": echo printed-by-widget'\r", quiet=0.3)
        t.type("echo hel")
        assert t.ghost_cells(), t.dump()
        t.send("\x18f", quiet=0.3)
        y, x = t.find("echo hello-world-123")
        assert not t.ghost_cells(), "line equals the history entry: nothing to suggest\n" + t.dump()
        assert t.cell(y, x).fg == "green", t.dump()
        assert_eq(t.cur, (y, x + len("echo hello-world-123")), t)
        t.send("\x15")
        t.type("echo hel")
        t.send("\x18p", quiet=0.3)
        t.find("printed-by-widget")
        ghosts = t.ghost_cells()
        assert ghosts and all(gy == t.cur[0] for gy, _ in ghosts), "ghost only on the live line\n" + t.dump()
        s = t.status()
        assert "layout mismatches 0" in s and "internal errors 0" in s, s
    finally:
        t.close()


@test
def wide_and_control_chars_are_highlighted(rc):
    t = Term(rc, cols=24)
    try:
        t.type("echo 日本語🦀 ok && ls")
        t.send("\x16\t")  # literal tab via quoted-insert
        t.send("\x16\x01")  # literal ^A
        t.type("x")
        plain = Term(rc, cols=24, plugin=False, cwd=t.cwd)
        try:
            plain.type("echo 日本語🦀 ok && ls")
            plain.send("\x16\t")
            plain.send("\x16\x01")
            plain.type("x")
            assert_eq(t.text_without_ghost(), plain.text_without_ghost(), t, "same text as plain readline")
            assert_eq(t.cur, plain.cur, t)
        finally:
            plain.close()
        y, x = t.find("ls")
        assert t.cell(y, x).fg == "green", t.dump()
        s = t.status()
        assert "layout mismatches 0" in s, s
    finally:
        t.close()


@test
def random_edits_match_plain_readline(rc):
    rnd = random.Random(1234)
    keys = list("abc xyz'\"$/-") + ["日", "🦀", "é", "\x16\t", "\x16\x01"] + ["\x7f", "\x02", "\x06", "\x01", "\x05", "\x1b[D", "\x1b[C"] * 2
    a = Term(rc, cols=24)
    b = Term(rc, cols=24, plugin=False, cwd=a.cwd)
    try:
        for step in range(160):
            k = rnd.choice(keys)
            if k in ("\x1b[C", "\x06", "\x05"):
                # these may accept a suggestion in inkline; send the same to both only when no ghost
                if a.ghost_cells():
                    continue
            a.send(k, quiet=0.05)
            b.send(k, quiet=0.05)
            if a.text_without_ghost() != b.text_without_ghost() or a.cur != b.cur:
                raise AssertionError(f"diverged at step {step} after {k!r}\ninkline:\n{a.dump()}\nplain:\n{b.dump()}")
        s = a.status()
        assert "layout mismatches 0" in s and "internal errors 0" in s, s
    finally:
        a.close()
        b.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-k", default="")
    ap.add_argument("--rc", help="extra rc file to run every test against")
    args = ap.parse_args()
    rcs = [("simple", SIMPLE_RC), ("escaped-multiline-prompt", ESCAPED_RC)]
    if args.rc:
        rcs.append((os.path.basename(args.rc), open(args.rc).read()))
    failed = 0
    for rc_name, rc in rcs:
        for fn in TESTS:
            if args.k not in fn.__name__:
                continue
            name = f"{rc_name}::{fn.__name__}"
            t0 = time.time()
            try:
                fn(rc)
                print(f"PASS {name} ({time.time() - t0:.1f}s)")
            except Exception:
                failed += 1
                print(f"FAIL {name}\n{traceback.format_exc()}")
    print(f"{failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
