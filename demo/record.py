"""Record the README demo: a scripted session typed into real bash + inkline
through a pty, captured with the pyte terminal emulator and written as
animated SVGs (one per colour theme) that GitHub plays inline.

    cargo build --release
    .venv/bin/python demo/record.py            # writes demo/demo-dark.svg, demo/demo-light.svg
    .venv/bin/python demo/record.py --still 9 20 --still-dir /tmp  # plus static frames, for checking

The session runs in a throwaway HOME and project directory with its own
history; nothing from your shell setup or history ends up in the recording.
"""

import argparse
import fcntl
import os
import pty
import random
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time
from xml.sax.saxutils import escape

import pyte

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SO = os.path.join(ROOT, "target", "release", "libinkline.so")
COLS, ROWS = 84, 11

HISTORY = [
    "cargo test",
    "git log --oneline -5",
    "docker run --rm -it -v \"$PWD\":/work -w /work rust:1.82 cargo test --release --all-features",
    "cargo build --release --target x86_64-unknown-linux-gnu",
    "git status --short",
    "cat README.md",
]

PROJECT = {
    "Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n',
    "README.md": "# demo\n",
    "src/main.rs": 'fn main() {\n    println!("{}", demo::greet("world"));\n}\n',
    "src/lib.rs": 'pub fn greet(name: &str) -> String {\n    format!("hello, {name}")\n}\n',
}

RC = r"""
PS1='\[\e[34m\]~/projects/demo\[\e[0m\] \[\e[35m\]❯\[\e[0m\] '
PROMPT_COMMAND=
HISTFILE={hist}; history -c; history -r
enable -f {so} inkline
"""

RIGHT, BS, TAB, ALT_F, CTRL_Y, CTRL_C, CTRL_L, ENTER = "\x1b[C", "\x7f", "\t", "\x1bf", "\x19", "\x03", "\x0c", "\r"


def scenes(s):
    s.pause(1.2)
    # unknown command in red, a known one in green, suggestion accepted with Right
    s.type("gti")
    s.pause(0.8)
    s.keys(BS, BS)
    s.type("it st")
    s.pause(1.4)
    s.keys(RIGHT)
    s.pause(0.7)
    s.keys(ENTER)
    s.pause(1.6)
    # reserved words, globs, paths, strings and variables
    s.type('for f in src/*.rs; do wc -l "$f"; done')
    s.pause(1.6)
    s.keys(ENTER)
    s.pause(1.6)
    # path prefixes are underlined as you type, readline's Tab completion as usual
    s.type("cat src/ma")
    s.pause(0.9)
    s.keys(TAB)
    s.pause(1.0)
    s.keys(ENTER)
    s.pause(1.8)
    s.keys(CTRL_L)
    s.pause(0.6)
    # partial accept, word by word (Alt-F)
    s.type("cargo b")
    s.pause(1.2)
    s.keys(ALT_F)
    s.pause(0.6)
    s.keys(ALT_F)
    s.pause(1.2)
    s.keys(CTRL_C)
    s.pause(0.8)
    # long suggestions wrap like typed text; Ctrl-Y accepts
    s.type("docker r")
    s.pause(1.8)
    s.keys(CTRL_Y)
    s.pause(1.6)
    s.keys(CTRL_C)
    s.pause(0.8)
    # precommands with their options
    s.type("sudo -u postgres psql -c 'select 1'")
    s.pause(2.0)
    s.keys(CTRL_C)
    s.pause(2.5)


class Session:
    def __init__(self, home):
        project = os.path.join(home, "projects", "demo")
        for path, body in PROJECT.items():
            os.makedirs(os.path.dirname(os.path.join(project, path)), exist_ok=True)
            with open(os.path.join(project, path), "w") as f:
                f.write(body)
        git = ["git", "-c", "user.name=demo", "-c", "user.email=demo@example.com", "-c", "init.defaultBranch=main"]
        subprocess.run(git + ["init", "-q"], cwd=project, check=True)
        subprocess.run(git + ["add", "."], cwd=project, check=True)
        subprocess.run(git + ["commit", "-qm", "init"], cwd=project, check=True)
        with open(os.path.join(project, "src/lib.rs"), "a") as f:
            f.write('\npub fn shout(name: &str) -> String {\n    greet(name).to_uppercase()\n}\n')
        open(os.path.join(project, "notes.txt"), "w").close()

        hist = os.path.join(home, ".bash_history")
        with open(hist, "w") as f:
            f.write("\n".join(HISTORY) + "\n")
        rc = os.path.join(home, ".bashrc")
        with open(rc, "w") as f:
            f.write(RC.replace("{hist}", hist).replace("{so}", SO))

        self.screen = pyte.Screen(COLS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        env = {
            "HOME": home, "PATH": os.environ["PATH"], "TERM": "xterm-256color",
            "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "INPUTRC": "/dev/null",
        }
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(project)
            os.execvpe("bash", ["bash", "--noprofile", "--rcfile", rc, "-i"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.rnd = random.Random(7)
        self.frames = []
        self.t0 = None
        self.read_for(1.5)
        self.t0 = time.perf_counter()
        self.snap()

    def read_for(self, seconds):
        end = time.perf_counter() + seconds
        while (left := end - time.perf_counter()) > 0:
            r, _, _ = select.select([self.fd], [], [], left)
            if not r:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return
            if b"\x1b[6n" in data:
                os.write(self.fd, f"\x1b[{self.screen.cursor.y + 1};{self.screen.cursor.x + 1}R".encode())
            self.stream.feed(data)
            if self.t0 is not None:
                self.snap()

    def snap(self):
        rows = tuple(
            tuple((c.data, c.fg, c.bg, c.bold, c.underscore, c.reverse) for c in (self.screen.buffer[y][x] for x in range(COLS)))
            for y in range(ROWS)
        )
        cur = None if self.screen.cursor.hidden else (self.screen.cursor.x, self.screen.cursor.y)
        t = time.perf_counter() - self.t0
        if self.frames and t - self.frames[-1][0] < 0.02:
            self.frames[-1] = (self.frames[-1][0], rows, cur)  # same redraw, split over reads
        elif not self.frames or self.frames[-1][1:] != (rows, cur):
            self.frames.append((t, rows, cur))

    def pause(self, seconds):
        self.read_for(seconds)

    def keys(self, *seqs):
        for k in seqs:
            os.write(self.fd, k.encode())
            self.read_for(0.12)

    def type(self, text):
        for ch in text:
            os.write(self.fd, ch.encode())
            self.read_for(self.rnd.uniform(0.05, 0.11))

    def close(self):
        os.kill(self.pid, 9)
        os.waitpid(self.pid, 0)


# Omarchy palettes, mapped to terminal colours the way Omarchy's Ghostty
# template does (0 = background, 7 = foreground, 8 = muted, 15 = bright fg).
THEMES = {
    "dark": dict(  # Tokyo Night
        bg="#1a1b26", fg="#a9b1d6", cursor="#c0caf5",
        palette=["#1a1b26", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#ad8ee6", "#449dab", "#a9b1d6",
                 "#414868", "#ff7a93", "#b9f27c", "#ff9e64", "#7da6ff", "#bb9af7", "#0db9d7", "#c0caf5"],
    ),
    "light": dict(  # Catppuccin Latte
        bg="#eff1f5", fg="#4c4f69", cursor="#4c4f69",
        palette=["#eff1f5", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#4c4f69",
                 "#acb0be", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#4c4f69"],
    ),
}
NAMES = ["black", "red", "green", "brown", "blue", "magenta", "cyan", "white"]
CW, LH, FS, PAD = 8.4, 19, 14, 18
FONT = "'JetBrains Mono','Cascadia Mono',SFMono-Regular,Menlo,Consolas,'DejaVu Sans Mono','Liberation Mono',monospace"


def color(name, theme, default):
    if name == "default":
        return default
    if name in NAMES:
        return theme["palette"][NAMES.index(name)]
    if name.startswith("bright") and name[6:] in NAMES:
        return theme["palette"][8 + NAMES.index(name[6:])]
    return "#" + name


def render_row(row, theme):
    """SVG for one screen row at y=0: background rects, then text runs. Each run
    gets an exact textLength so cells line up whatever monospace font renders."""
    out, x = [], 0
    while x < COLS:
        data, fg, bg, bold, under, rev = row[x]
        style = (fg, bg, bold, under, rev)
        ascii_run = data.isascii()
        end = x + 1
        while end < COLS and row[end][1:] == style and row[end][0].isascii() and ascii_run:
            end += 1
        text = "".join(c[0] or " " for c in row[x:end])
        f, b = color(fg, theme, theme["fg"]), color(bg, theme, None)
        if rev:
            f, b = b or theme["bg"], f
        if b:
            out.append(f'<rect x="{x * CW:.1f}" width="{(end - x) * CW:.1f}" height="{LH}" fill="{b}"/>')
        if text.strip() or under:
            cls = " ".join(c for c, on in (("b", bold), ("u", under)) if on)
            out.append(
                f'<text x="{x * CW:.1f}" y="{LH - 5}" fill="{f}"{f" class={chr(34)}{cls}{chr(34)}" if cls else ""}'
                f' textLength="{(end - x) * CW:.1f}" lengthAdjust="spacingAndGlyphs">{escape(text)}</text>'
            )
        x = end
    return "".join(out)


def svg(frames, theme, hold, still=None):
    w, h = COLS * CW, ROWS * LH
    if still is not None:
        frames = [max((f for f in frames if f[0] <= still), key=lambda f: f[0])]
    duration = frames[-1][0] + hold
    row_ids, defs, reel = {}, [], []
    for i, (t, rows, cur) in enumerate(frames):
        uses = []
        for y, row in enumerate(rows):
            if all(c[0] in ("", " ") and c[2] == "default" and not c[4] and not c[5] for c in row):
                continue
            if row not in row_ids:
                row_ids[row] = f"r{len(row_ids)}"
                defs.append(f'<g id="{row_ids[row]}">{render_row(row, theme)}</g>')
            uses.append(f'<use href="#{row_ids[row]}" y="{y * LH}"/>')
        if cur:
            uses.append(f'<rect x="{cur[0] * CW:.1f}" y="{cur[1] * LH + 2}" width="2" height="{LH - 3}" fill="{theme["cursor"]}"/>')
        reel.append(f'<g transform="translate(0 {i * h})">{"".join(uses)}</g>')
    steps = "".join(f"{t / duration * 100:.3f}%{{transform:translateY({-i * h}px)}}" for i, (t, _, _) in enumerate(frames))
    anim = "" if still is not None else (
        f"#reel{{animation:roll {duration:.2f}s step-end infinite}}@keyframes roll{{{steps}100%{{transform:translateY({-(len(frames) - 1) * h}px)}}}}"
    )
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w + 2 * PAD:.0f}" height="{h + 2 * PAD:.0f}" '
        f'viewBox="0 0 {w + 2 * PAD:.0f} {h + 2 * PAD:.0f}" role="img" aria-label="inkline demo: bash with zsh-style suggestions and syntax highlighting">'
        f"<style>text{{font-family:{FONT};font-size:{FS}px;white-space:pre}}.b{{font-weight:bold}}.u{{text-decoration:underline}}{anim}</style>"
        f'<rect width="100%" height="100%" rx="10" fill="{theme["bg"]}"/>'
        f'<defs>{"".join(defs)}</defs>'
        f'<svg x="{PAD}" y="{PAD}" width="{w:.0f}" height="{h:.0f}"><g id="reel">{"".join(reel)}</g></svg></svg>'
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(ROOT, "demo"))
    ap.add_argument("--still", type=float, nargs="*", default=[], help="also write still frames at these times (s)")
    ap.add_argument("--still-dir", help="directory for the still frames (default: --out)")
    ap.add_argument("--hold", type=float, default=1.0, help="seconds to hold the last frame before looping")
    args = ap.parse_args()
    if not os.path.exists(SO):
        raise SystemExit("build first: cargo build --release")
    home = tempfile.mkdtemp(prefix="inkline-demo-")
    try:
        s = Session(home)
        try:
            scenes(s)
        finally:
            s.close()
    finally:
        shutil.rmtree(home)
    for name, theme in THEMES.items():
        path = os.path.join(args.out, f"demo-{name}.svg")
        with open(path, "w") as f:
            f.write(svg(s.frames, theme, args.hold))
        print(f"{path}: {len(s.frames)} frames, {s.frames[-1][0] + args.hold:.1f}s, {os.path.getsize(path) // 1024} KiB")
        for t in args.still:
            with open(os.path.join(args.still_dir or args.out, f"demo-{name}-{t:g}s.svg"), "w") as f:
                f.write(svg(s.frames, theme, args.hold, still=t))


if __name__ == "__main__":
    main()
