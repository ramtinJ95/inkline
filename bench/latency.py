"""Line-editor latency and CPU benchmark: drive an interactive shell through a
pty, type real commands, and measure per keystroke:

- first: time until the shell writes anything back
- done:  time until its redraw finishes (last byte before `--quiet` ms of silence)
- cpu:   CPU time the shell process spent (from /proc/<pid>/schedstat, ns)

Usage:
  python3 bench/latency.py --name bash+inkline -- bash --rcfile RC -i
  python3 bench/latency.py --name zsh+plugins --env ZDOTDIR=DIR -- zsh -i

The prompt must contain `❯` (starship) so the first prompt can be detected.
"""

import argparse
import fcntl
import json
import os
import pty
import select
import statistics
import struct
import sys
import termios
import time

PROMPT = "❯".encode()


def schedstat_ns(pid):
    with open(f"/proc/{pid}/schedstat") as f:
        return int(f.read().split()[0])


class Shell:
    def __init__(self, argv, env, cwd, cols, rows):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ.update(env)
            os.environ.update(TERM="xterm-256color", COLUMNS=str(cols), LINES=str(rows))
            os.chdir(cwd)
            os.execvp(argv[0], argv)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def read_some(self, timeout):
        r, _, _ = select.select([self.fd], [], [], timeout)
        if not r:
            return b""
        try:
            data = os.read(self.fd, 65536)
        except OSError:
            return b""
        if b"\x1b[6n" in data:
            os.write(self.fd, b"\x1b[5;1R")
        return data

    def wait_for(self, needle, limit=20.0):
        buf, t0 = b"", time.perf_counter()
        while time.perf_counter() - t0 < limit:
            buf += self.read_some(0.2)
            if needle in buf:
                return time.perf_counter()
        raise SystemExit(f"timeout waiting for {needle!r}; tail: {buf[-200:]!r}")

    def drain(self, quiet):
        """Read until `quiet` seconds of silence; return time of the last byte (or None)."""
        last = None
        while True:
            data = self.read_some(quiet)
            if not data:
                return last
            last = time.perf_counter()

    def key(self, data, quiet):
        """Send one key; return (first-byte latency, redraw-done latency) in ms."""
        t0 = time.perf_counter()
        os.write(self.fd, data)
        r, _, _ = select.select([self.fd], [], [], 2.0)
        first = time.perf_counter()
        if r:
            self.read_some(0)
        last = self.drain(quiet) or first
        return (first - t0) * 1e3, (last - t0) * 1e3

    def close(self):
        try:
            os.write(self.fd, b"\x15exit\r")
            time.sleep(0.2)
            os.kill(self.pid, 9)
            os.waitpid(self.pid, 0)
        except OSError:
            pass


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def summary(xs):
    return {"median": round(statistics.median(xs), 3), "p90": round(pct(xs, 90), 3), "max": round(max(xs), 3)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--env", action="append", default=[])
    ap.add_argument("--cwd", default=os.getcwd())
    ap.add_argument("--lines", required=True, help="file with one command per line to type")
    ap.add_argument("--gap", type=float, default=0.08, help="seconds between keys (realistic typing)")
    ap.add_argument("--quiet", type=float, default=0.03)
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--json", help="append results as JSON lines to this file")
    ap.add_argument("--dump", help="write per-keystroke samples (cpu µs, done ms) to this file")
    ap.add_argument("argv", nargs="+")
    args = ap.parse_args()

    env = dict(e.split("=", 1) for e in args.env)
    lines = [l.rstrip("\n") for l in open(args.lines) if l.strip()]
    sh = Shell(args.argv, env, args.cwd, 120, 40)
    try:
        t0 = time.perf_counter()
        t = sh.wait_for(PROMPT)
        startup = (t - t0) * 1e3
        sh.drain(0.5)

        firsts, dones, cpus = [], [], []
        for _ in range(args.rounds):
            for line in lines:
                for ch in line:
                    c0 = schedstat_ns(sh.pid)
                    f, d = sh.key(ch.encode(), args.quiet)
                    cpus.append((schedstat_ns(sh.pid) - c0) / 1e3)
                    firsts.append(f)
                    dones.append(d)
                    time.sleep(max(0.0, args.gap - d / 1e3))
                sh.key(b"\x15", 0.15)  # Ctrl-U: kill the line

        # burst: whole line written at once, like very fast typing or a paste
        bursts = []
        for line in lines:
            c0 = schedstat_ns(sh.pid)
            sh.key(line.encode(), args.quiet)
            bursts.append((schedstat_ns(sh.pid) - c0) / 1e3 / max(1, len(line)))
            sh.key(b"\x15", 0.15)

        # Enter -> next prompt, with a trivial command
        enters = []
        for i in range(6):
            sh.drain(0.6)
            t0 = time.perf_counter()
            os.write(sh.fd, f"true #{i}\r".encode())
            sh.wait_for(PROMPT)
            enters.append((time.perf_counter() - t0) * 1e3)

        if args.dump:
            with open(args.dump, "w") as f:
                for k, (c, d) in enumerate(zip(cpus, dones)):
                    f.write(f"{k} {c:.1f} {d:.3f}\n")
        res = {
            "name": args.name,
            "keys": len(firsts),
            "startup_ms": round(startup, 1),
            "first_ms": summary(firsts),
            "done_ms": summary(dones),
            "cpu_us_per_key": summary(cpus),
            "cpu_us_per_key_burst": round(statistics.mean(bursts), 1),
            "enter_to_prompt_ms": summary(enters),
        }
        print(json.dumps(res))
        if args.json:
            with open(args.json, "a") as f:
                f.write(json.dumps(res) + "\n")
    finally:
        sh.close()


if __name__ == "__main__":
    main()
