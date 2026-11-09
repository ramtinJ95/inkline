//! Micro-benchmark of the pure parts on real data:
//! `cargo run --release --example bench -- ~/.bash_history`
use inkline::{highlight, suggest};
use std::time::Instant;

struct Fs;
impl highlight::Resolver for Fs {
    fn command(&self, w: &str) -> bool {
        std::env::var("PATH").unwrap_or_default().split(':').any(|d| std::path::Path::new(d).join(w).exists())
    }
    fn path_kind(&self, w: &str) -> highlight::PathKind {
        match std::fs::metadata(w) {
            Ok(m) if m.is_dir() => highlight::PathKind::Dir,
            Ok(_) => highlight::PathKind::Other,
            Err(_) => highlight::PathKind::Missing,
        }
    }
    fn path_prefix(&self, _w: &str, _cmd: bool) -> bool {
        false
    }
    fn autocd(&self) -> bool {
        false
    }
}

fn main() {
    let hist = std::fs::read(std::env::args().nth(1).expect("history file")).unwrap();
    let lines: Vec<&[u8]> = hist.split(|&b| b == b'\n').filter(|l| !l.is_empty()).collect();
    let typed = ["git status", "ls ~/dotfiles/arch-config/", "cat /etc/hosts | grep local", "echo \"$HOME\" $(date)", "qzx nothing matches"];
    for t in typed {
        let n = 2000;
        let t0 = Instant::now();
        for _ in 0..n {
            for k in 1..=t.len() {
                std::hint::black_box(highlight::highlight(&t.as_bytes()[..k], &Fs, false));
            }
        }
        let hl = t0.elapsed().as_nanos() as f64 / (n * t.len()) as f64 / 1e3;
        let t0 = Instant::now();
        for _ in 0..n {
            for k in 1..=t.len() {
                std::hint::black_box(suggest::from_history(&t.as_bytes()[..k], lines.iter().rev().copied()));
            }
        }
        let sg = t0.elapsed().as_nanos() as f64 / (n * t.len()) as f64 / 1e3;
        println!("{t:32} highlight (uncached fs) {hl:7.2} µs/key   history scan ({} entries) {sg:6.2} µs/key", lines.len());
    }
}
