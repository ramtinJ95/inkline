//! Randomized property tests for the pure modules. Deterministic per seed.
//!
//!   cargo test --release --test fuzz                         # quick (default iterations)
//!   INKLINE_FUZZ_ITERS=200000 cargo test --release --test fuzz
//!   INKLINE_FUZZ_SEED=42 ...                                  # reproduce a failure

use inkline::highlight::{self, PathKind, Resolver};
use inkline::layout::{self, Layout, Opts};
use inkline::{paint, suggest};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

fn iters() -> usize {
    std::env::var("INKLINE_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(3000)
}

fn seed() -> u64 {
    std::env::var("INKLINE_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x1234_5678_9abc_def1)
}

const TOKENS: &[&str] = &[
    "ls", "git", "sudo", "-u", "root", "echo", "cat", "/etc", "/etc/ho", "~/", "./x", "for", "in", "do", "done", "if", "then", "fi", "[[",
    "]]", "{", "}", "(", ")", "((", "))", "$(", "`", "'", "\"", "$'", "${", "$HOME", "$1", "*", "?", "!!", "^a^b", "|", "||", "&&", "&", ";",
    ";;", "<", ">", ">>", "2>&1", "&>", "<<<", "<(", ">(", "#", "\\", "\\\"", "=", "a=(", "FOO=bar", "--opt=1", " ", " ", " ", "\t", "\n",
    "日本", "🦀", "é", "e\u{301}", "\u{200b}", "x",
];

fn shellish(r: &mut Rng) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..r.below(24) {
        if r.chance(4) {
            // raw hostile bytes: invalid UTF-8, control characters, NUL-free
            v.push(*r.pick(&[0xff, 0xc3, 0xe6, 0x80, 0x01, 0x1b, 0x7f, 0x08]));
        } else {
            v.extend_from_slice(r.pick(TOKENS).as_bytes());
        }
    }
    v
}

struct Fake;
impl Resolver for Fake {
    fn command(&self, w: &str) -> bool {
        w.len() % 3 == 0
    }
    fn path_kind(&self, w: &str) -> PathKind {
        [PathKind::Missing, PathKind::Dir, PathKind::Exec, PathKind::Other][w.len() % 4]
    }
    fn path_prefix(&self, w: &str, cmd: bool) -> bool {
        (w.len() + cmd as usize) % 2 == 0
    }
    fn autocd(&self) -> bool {
        false
    }
}

fn width(c: char) -> i32 {
    match c {
        '\u{301}' | '\u{200b}' => 0,
        '日' | '本' | '🦀' => 2,
        _ => 1,
    }
}

#[test]
fn highlight_never_panics_and_covers_every_byte() {
    let mut r = Rng(seed());
    for i in 0..iters() {
        let buf = shellish(&mut r);
        for finishing in [false, true] {
            let st = std::panic::catch_unwind(|| highlight::highlight(&buf, &Fake, finishing))
                .unwrap_or_else(|_| panic!("iteration {i}: highlight panicked on {:?}", String::from_utf8_lossy(&buf)));
            assert_eq!(st.len(), buf.len(), "iteration {i}");
        }
    }
}

fn check_layout(buf: &[u8], point: usize, start: i64, o: &Opts, lay: &Layout, ctx: &str) {
    let mut prev: Option<&layout::Cell> = None;
    for c in &lay.cells {
        assert!(c.col >= 0 && c.width >= 0 && c.col + c.width <= o.cols, "{ctx}: cell out of row {c:?}");
        assert!(c.start < c.end && c.end <= buf.len(), "{ctx}: bad byte range {c:?}");
        if let Some(p) = prev {
            assert!(c.start >= p.start, "{ctx}: byte order {p:?} {c:?}");
            assert!((c.row, c.col) > (p.row, p.col) || (p.width == 0 && (c.row, c.col) >= (p.row, p.col)), "{ctx}: screen order {p:?} {c:?}");
            if c.row == p.row {
                assert!(p.col + p.width <= c.col, "{ctx}: overlap {p:?} {c:?}");
            }
        }
        prev = Some(c);
    }
    // every displayable byte before a NUL is inside some cell
    let stop = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let mut covered = vec![false; buf.len()];
    for c in &lay.cells {
        covered[c.start..c.end].fill(true);
    }
    for (i, &b) in buf[..stop].iter().enumerate() {
        assert!(covered[i] || b == b'\n', "{ctx}: byte {i} ({b:#x}) not displayed");
    }
    if let Some(last) = lay.cells.last() {
        assert!(lay.end >= (last.row, last.col), "{ctx}: end before last cell");
    }
    let first_row = start / o.cols;
    assert!(lay.point.0 >= first_row && lay.end.0 >= lay.point.0, "{ctx}: point/end rows");
    // the cursor sits on the cell that starts at `point` (unless that is a zero-width char)
    if let Some(c) = lay.cells.iter().find(|c| c.start == point) {
        assert_eq!(lay.point, (c.row, c.col), "{ctx}: point cell");
    }
}

#[test]
fn layout_invariants() {
    let mut r = Rng(seed() ^ 0xabcdef);
    for i in 0..iters() * 4 {
        let buf = shellish(&mut r);
        let narrow = r.chance(30);
        let cols = 1 + r.below(if narrow { 8 } else { 120 }) as i64;
        let start = r.below((cols * 2) as usize) as i64;
        let o = Opts { cols, multibyte: r.chance(85), output_meta: r.chance(70) };
        let bounds: Vec<usize> = (0..=buf.len()).filter(|&k| k == buf.len() || (buf[k] & 0xc0) != 0x80).collect();
        let point = *r.pick(&bounds);
        let ctx = format!("iteration {i}: cols {cols} start {start} point {point} mb {} buf {:?}", o.multibyte, String::from_utf8_lossy(&buf));
        let Some(lay) = layout::layout(&buf, point, start, &o, &width) else {
            assert!(cols < 8 && (buf.contains(&b'\t') || cols < 2), "{ctx}: no layout");
            continue;
        };
        check_layout(&buf, point, start, &o, &lay, &ctx);
    }
}

/// Minimal VT model: cursor motion, CR, SGR ignored, printable text with
/// autowrap and the pending-wrap state at the right margin.
struct Vt {
    cols: i64,
    rows: i64,
    cells: Vec<Vec<Vec<u8>>>,
    row: i64,
    col: i64,
    pending_wrap: bool,
    /// Last cell written: combining characters attach to it.
    last: Option<(i64, i64)>,
}

impl Vt {
    fn new(cols: i64, rows: i64, row: i64, col: i64) -> Self {
        Vt { cols, rows, cells: vec![vec![Vec::new(); cols as usize]; rows as usize], row, col, pending_wrap: false, last: None }
    }

    fn feed(&mut self, out: &[u8], widths: &dyn Fn(&[u8]) -> i64) {
        let mut i = 0;
        while i < out.len() {
            match out[i] {
                b'\r' => {
                    self.col = 0;
                    self.pending_wrap = false;
                    i += 1;
                }
                0x1b => {
                    assert_eq!(out.get(i + 1), Some(&b'['), "only CSI sequences expected");
                    let mut j = i + 2;
                    while !out[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    let n: i64 = std::str::from_utf8(&out[i + 2..j]).unwrap().split(';').next().unwrap().parse().unwrap_or(1).max(1);
                    match out[j] {
                        b'A' => self.row -= n,
                        b'B' => self.row += n,
                        b'C' => self.col = (self.col + n).min(self.cols - 1),
                        b'm' => {}
                        other => panic!("unexpected CSI {}", other as char),
                    }
                    if out[j] != b'm' {
                        self.pending_wrap = false;
                    }
                    assert!(self.row >= 0 && self.row < self.rows, "cursor left the screen");
                    i = j + 1;
                }
                _ => {
                    let len = std::str::from_utf8(&out[i..]).map(|s| s.chars().next().unwrap().len_utf8()).unwrap_or_else(|e| {
                        if e.valid_up_to() > 0 { std::str::from_utf8(&out[i..i + e.valid_up_to()]).unwrap().chars().next().unwrap().len_utf8() } else { 1 }
                    });
                    let ch = &out[i..i + len];
                    let w = widths(ch);
                    if w == 0 {
                        // combining: attach to the last written cell (or the one before the cursor)
                        let (r, c) = self.last.unwrap_or((self.row, (self.col - 1).max(0)));
                        self.cells[r as usize][c as usize].extend_from_slice(ch);
                    } else {
                        if self.pending_wrap || self.col + w > self.cols {
                            self.row += 1;
                            self.col = 0;
                            self.pending_wrap = false;
                        }
                        self.cells[self.row as usize][self.col as usize] = ch.to_vec();
                        self.last = Some((self.row, self.col));
                        if self.col + w >= self.cols {
                            self.col = self.cols - 1;
                            self.pending_wrap = true;
                        } else {
                            self.col += w;
                        }
                    }
                    i += len;
                }
            }
        }
    }
}

#[test]
fn repaint_writes_each_cell_in_place_and_restores_cursor() {
    let mut r = Rng(seed() ^ 0x5151);
    for i in 0..iters() * 2 {
        let buf = shellish(&mut r);
        let narrow = r.chance(30);
        let cols = 2 + r.below(if narrow { 8 } else { 100 }) as i64;
        let o = Opts { cols, multibyte: true, output_meta: true };
        let start = r.below(cols as usize) as i64;
        let bounds: Vec<usize> = (0..=buf.len()).filter(|&k| k == buf.len() || (buf[k] & 0xc0) != 0x80).collect();
        let point = *r.pick(&bounds);
        let Some(lay) = layout::layout(&buf, point, start, &o, &width) else { continue };
        let offset = r.below(3) as i64;
        let cursor = (lay.point.0 + offset, lay.point.1);
        let rows = lay.end.0 + offset + 2;
        let styles = ["", "32", "1;31", "90", "33;7"];
        let sgr: Vec<&str> = lay.cells.iter().map(|_| *r.pick(&styles)).collect();
        let mut out = Vec::new();
        paint::repaint(&mut out, &lay.cells, &sgr, offset, cursor, cols);
        let mut vt = Vt::new(cols, rows, cursor.0, cursor.1);
        let w = |ch: &[u8]| match std::str::from_utf8(ch).ok().and_then(|s| s.chars().next()) {
            Some(c) if !c.is_ascii() => width(c) as i64,
            _ => 1,
        };
        vt.feed(&out, &w);
        let ctx = format!(
            "iteration {i}: cols {cols} start {start} point {point} offset {offset} cursor {cursor:?} buf {:?}\nout {:?}",
            String::from_utf8_lossy(&buf),
            String::from_utf8_lossy(&out)
        );
        assert_eq!((vt.row, vt.col), cursor, "{ctx}: cursor not restored");
        // Compare visible characters column by column; zero-width characters
        // ride along with whatever cell precedes them and are ignored here.
        let visible = |b: &[u8]| -> String { String::from_utf8_lossy(b).chars().filter(|&ch| ch.is_ascii() || width(ch) != 0).collect() };
        for c in lay.cells.iter().filter(|c| c.width > 0) {
            let want = visible(&c.text);
            let row = (c.row + offset) as usize;
            if want.len() > 1 && want.is_ascii() {
                for (k, ch) in want.chars().enumerate() {
                    assert_eq!(visible(&vt.cells[row][c.col as usize + k]), ch.to_string(), "{ctx}: cell {c:?}");
                }
            } else {
                assert_eq!(visible(&vt.cells[row][c.col as usize]), want, "{ctx}: cell {c:?}");
            }
        }
    }
}

#[test]
fn suggestion_cache_matches_fresh_lookup() {
    let mut r = Rng(seed() ^ 0x77);
    let words: &[&[u8]] = &[b"git", b" ", b"status", b"s", b"push", b"-", b"ls", b"/etc", b"\n", b"x"];
    for i in 0..iters() {
        let n = r.below(12);
        let mut history: Vec<Vec<u8>> = (0..n)
            .map(|_| {
                let k = 1 + r.below(5);
                (0..k).flat_map(|_| r.pick(words).to_vec()).collect()
            })
            .collect();
        let mut cache = suggest::Cache::default();
        let mut buf: Vec<u8> = Vec::new();
        let mut generation = 0;
        for step in 0..40 {
            match r.below(10) {
                0..=5 => {
                    let w: &[u8] = *r.pick::<&[u8]>(words);
                    buf.extend_from_slice(w)
                }
                6 | 7 => {
                    buf.pop();
                }
                8 => buf.clear(),
                _ => {
                    history.push(buf.clone());
                    generation += 1;
                }
            }
            let newest_first: Vec<&[u8]> = history.iter().rev().map(Vec::as_slice).collect();
            let fresh = suggest::from_history(&buf, newest_first.iter().copied()).map(<[u8]>::to_vec);
            let key = (1, history.len() as i32, generation);
            let cached = cache.get(&buf, key, || Box::new(newest_first.clone().into_iter())).map(<[u8]>::to_vec);
            assert_eq!(cached, fresh, "iteration {i} step {step}: buf {:?}", String::from_utf8_lossy(&buf));
        }
    }
}
