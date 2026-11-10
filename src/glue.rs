//! bash builtin + readline hooks. Everything here is FFI; every entry point
//! runs under `guard`, which turns a panic into "inkline disables itself and
//! readline carries on" instead of taking the shell down.
//!
//! Display: while readline redraws, the suggestion is temporarily appended to
//! the line buffer, so readline itself wraps, scrolls and later erases it like
//! real text. Afterwards inkline recolours the cells (syntax styles for the
//! buffer, colour 8 for the suggestion) at positions computed by `layout`,
//! after checking them against readline's actual cursor and bottom row. If the
//! check fails the frame is redrawn without the suggestion, so it can never
//! show up looking like typed text.
//!
//! Behaviour follows zsh-autosuggestions 0.7.1: the suggestion is the newest
//! history entry starting with the buffer (multi-line entries included), shown
//! wherever the cursor is; accept widgets accept it only with the cursor at the
//! end; partial-accept widgets run the editor's own motion over buffer +
//! suggestion; history navigation clears it until the buffer changes.

#![allow(non_upper_case_globals, static_mut_refs)]

use crate::highlight::{self, PathKind};
use crate::{layout, paint, suggest};
use std::borrow::Cow;
use std::cell::UnsafeCell;
use std::ffi::{CStr, CString, OsStr, c_char, c_int, c_ulong, c_void};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

type RlFn = unsafe extern "C" fn(c_int, c_int) -> c_int;
type VoidFn = unsafe extern "C" fn();
type PrepFn = unsafe extern "C" fn(c_int);
type Keymap = *mut c_void;

#[repr(C)]
struct HistEntry {
    line: *mut c_char,
    timestamp: *mut c_char,
    data: *mut c_void,
}

#[repr(C)]
pub struct WordDesc {
    word: *mut c_char,
    flags: c_int,
}

#[repr(C)]
pub struct WordList {
    next: *mut WordList,
    word: *mut WordDesc,
}

/// bash's `struct builtin` (builtins.h).
#[repr(C)]
pub struct Builtin {
    name: *const c_char,
    function: Option<unsafe extern "C" fn(*mut WordList) -> c_int>,
    flags: c_int,
    long_doc: *const *const c_char,
    short_doc: *const c_char,
    handle: *mut c_char,
}

#[repr(C)]
struct DlInfo {
    fname: *const c_char,
    fbase: *mut c_void,
    sname: *const c_char,
    saddr: *mut c_void,
}

const RL_STATE_INITIALIZED: c_ulong = 0x2;
/// isearch, non-incremental search, history search.
const RL_STATE_SEARCHING: c_ulong = 0x80 | 0x100 | 0x200;
const RL_STATE_VIMOTION: c_ulong = 0x100000;
/// Completion in progress (listing, "Display all N?" query, --More-- pager) or
/// a command reading an extra key: the cursor is not where readline's line
/// model says, so painting relative to it would land on the listing.
const RL_STATE_LINE_OFFSCREEN: c_ulong = 0x4000 | 0x40;
const RL_STATE_SIGHANDLER: c_ulong = 0x8000;
const BUILTIN_ENABLED: c_int = 0x1;
const EX_USAGE: c_int = 258;
const RTLD_NOW: c_int = 0x2;
const RTLD_NOLOAD: c_int = 0x4;
const RTLD_NODELETE: c_int = 0x1000;
/// zsh-autosuggestions' default `fg=8`: palette colour 8 of the terminal theme.
const GHOST_SGR: &str = "90";

unsafe extern "C" {
    static mut rl_line_buffer: *mut c_char;
    static mut rl_point: c_int;
    static mut rl_end: c_int;
    static mut rl_mark: c_int;
    static mut rl_display_prompt: *mut c_char;
    static mut rl_readline_state: c_ulong;
    static rl_readline_version: c_int;
    static mut rl_byte_oriented: c_int;
    static mut rl_outstream: *mut c_void;
    static mut rl_redisplay_function: Option<VoidFn>;
    static mut rl_deprep_term_function: Option<VoidFn>;
    static mut rl_prep_term_function: Option<PrepFn>;
    static mut rl_last_func: Option<RlFn>;
    static mut rl_undo_list: *mut c_void;
    static mut _rl_last_c_pos: c_int;
    static mut _rl_last_v_pos: c_int;
    static mut _rl_vis_botlin: c_int;
    static mut _rl_term_up: *mut c_char;
    static mut _rl_horizontal_scroll_mode: c_int;
    static mut _rl_utf8locale: c_int;
    static mut _rl_output_meta_chars: c_int;
    static mut _rl_mark_modified_lines: c_int;
    static mut _rl_echoctl: c_int;
    static mut _rl_echo_control_chars: c_int;
    static mut _rl_intr_char: c_int;
    static mut history_base: c_int;
    static mut history_length: c_int;

    fn history_get(offset: c_int) -> *mut HistEntry;
    fn current_history() -> *mut HistEntry;
    fn rl_redisplay();
    fn rl_reset_terminal(name: *const c_char) -> c_int;
    fn rl_redraw_prompt_last_line();
    fn rl_get_screen_size(rows: *mut c_int, cols: *mut c_int);
    fn rl_extend_line_buffer(len: c_int);
    fn rl_insert_text(text: *const c_char) -> c_int;
    fn rl_mark_active_p() -> c_int;
    fn rl_get_keymap() -> Keymap;
    fn rl_get_keymap_by_name(name: *const c_char) -> Keymap;
    fn rl_named_function(name: *const c_char) -> Option<RlFn>;
    fn rl_invoking_keyseqs_in_map(f: RlFn, map: Keymap) -> *mut *mut c_char;
    fn rl_bind_keyseq_in_map(seq: *const c_char, f: Option<RlFn>, map: Keymap) -> c_int;
    fn rl_function_of_keyseq(seq: *const c_char, map: Keymap, kind: *mut c_int) -> Option<RlFn>;
    fn rl_add_defun(name: *const c_char, f: RlFn, key: c_int) -> c_int;

    // bash (exported by the bash executable for loadable builtins)
    static mut autocd: c_int;
    fn find_alias(name: *const c_char) -> *mut c_void;
    fn find_function(name: *const c_char) -> *mut c_void;
    fn find_shell_builtin(name: *const c_char) -> *mut c_void;
    fn find_user_command(name: *const c_char) -> *mut c_char;
    fn get_string_value(name: *const c_char) -> *mut c_char;

    fn fwrite(ptr: *const c_void, size: usize, n: usize, stream: *mut c_void) -> usize;
    fn fflush(stream: *mut c_void) -> c_int;
    fn free(ptr: *mut c_void);
    fn wcwidth(c: i32) -> c_int;
    fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
    fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
}

/// What a wrapped readline/bash command does around the original.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// zsh-autosuggestions ACCEPT_WIDGETS.
    Accept,
    /// PARTIAL_ACCEPT_WIDGETS.
    Partial,
    /// CLEAR_WIDGETS (history navigation).
    ClearAfter,
    /// accept-line and friends: redraw without the suggestion (and without
    /// path-prefix underline) first, so neither lands in scrollback.
    AcceptLine,
    /// Commands that print below the line (completion lists, dumps): redraw
    /// without the suggestion first so the output starts right below the text.
    HideBefore,
}

const WRAPPED: &[(&str, Kind)] = &[
    ("forward-char", Kind::Accept),
    ("end-of-line", Kind::Accept),
    ("vi-append-eol", Kind::Accept),
    ("forward-word", Kind::Partial),
    ("shell-forward-word", Kind::Partial),
    ("vi-next-word", Kind::Partial),
    ("vi-forward-word", Kind::Partial),
    ("vi-end-word", Kind::Partial),
    ("vi-forward-bigword", Kind::Partial),
    ("vi-fWord", Kind::Partial),
    ("vi-end-bigword", Kind::Partial),
    ("vi-eWord", Kind::Partial),
    ("previous-history", Kind::ClearAfter),
    ("next-history", Kind::ClearAfter),
    ("history-search-backward", Kind::ClearAfter),
    ("history-search-forward", Kind::ClearAfter),
    ("history-substring-search-backward", Kind::ClearAfter),
    ("history-substring-search-forward", Kind::ClearAfter),
    ("fetch-history", Kind::ClearAfter),
    ("vi-fetch-history", Kind::ClearAfter),
    ("accept-line", Kind::AcceptLine),
    ("operate-and-get-next", Kind::AcceptLine),
    ("edit-and-execute-command", Kind::AcceptLine),
    ("vi-edit-and-execute-command", Kind::AcceptLine),
    ("complete", Kind::HideBefore),
    ("menu-complete", Kind::HideBefore),
    ("menu-complete-backward", Kind::HideBefore),
    ("old-menu-complete", Kind::HideBefore),
    ("possible-completions", Kind::HideBefore),
    ("insert-completions", Kind::HideBefore),
    ("delete-char-or-list", Kind::HideBefore),
    ("bash-vi-complete", Kind::HideBefore),
    ("vi-complete", Kind::HideBefore),
    ("complete-command", Kind::HideBefore),
    ("complete-filename", Kind::HideBefore),
    ("complete-hostname", Kind::HideBefore),
    ("complete-into-braces", Kind::HideBefore),
    ("complete-username", Kind::HideBefore),
    ("complete-variable", Kind::HideBefore),
    ("possible-command-completions", Kind::HideBefore),
    ("possible-filename-completions", Kind::HideBefore),
    ("possible-hostname-completions", Kind::HideBefore),
    ("possible-username-completions", Kind::HideBefore),
    ("possible-variable-completions", Kind::HideBefore),
    ("dynamic-complete-history", Kind::HideBefore),
    ("glob-complete-word", Kind::HideBefore),
    ("glob-expand-word", Kind::HideBefore),
    ("glob-list-expansions", Kind::HideBefore),
    ("display-shell-version", Kind::HideBefore),
    ("dump-functions", Kind::HideBefore),
    ("dump-macros", Kind::HideBefore),
    ("dump-variables", Kind::HideBefore),
    ("export-completions", Kind::HideBefore),
    ("execute-named-command", Kind::HideBefore),
];

/// Where the suggestion was last drawn, in readline's coordinates.
#[derive(Clone, Copy)]
struct Shown {
    cursor: (c_int, c_int),
    bottom_row: c_int,
    real_end: (i64, i64),
}

#[derive(Default)]
struct State {
    installed: bool,
    active: bool,
    hooked: bool,
    bound: bool,
    orig_deprep: Option<VoidFn>,
    orig_prep: Option<PrepFn>,
    /// Redraw without a suggestion (set around accept-line / listings).
    hide: bool,
    finishing: bool,
    /// After a CLEAR widget: no suggestion while the buffer still equals this.
    cleared_for: Option<Vec<u8>>,
    shown: Option<Shown>,
    path_index: PathIndex,
    /// Screen size and readline cursor after the last frame of this line.
    last_frame: Option<((c_int, c_int), (c_int, c_int))>,
    resizes: u64,
    slots: Vec<(RlFn, Kind)>,
    cache: suggest::Cache,
    rebinds: Vec<(Keymap, CString, Option<RlFn>)>,
    dirs: Vec<(PathBuf, SystemTime, Vec<Vec<u8>>)>,
    /// Lookups for the line being edited; cleared at every new prompt.
    memo: [std::collections::HashMap<String, u8>; 4],
    frames: u64,
    painted: u64,
    mismatches: u64,
    repairs: u64,
    panics: u64,
    work_ns_total: u128,
    work_ns_max: u128,
    compute_ns_total: u128,
    compute_ns_max: u128,
}

struct Global(UnsafeCell<Option<State>>);
// bash is single-threaded; all access happens on its main thread.
unsafe impl Sync for Global {}
static STATE: Global = Global(UnsafeCell::new(None));

/// Short-lived access only: never hold the reference across a call back into readline.
fn st() -> &'static mut State {
    unsafe { (*STATE.0.get()).get_or_insert_with(State::default) }
}

fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            let s = st();
            s.panics += 1;
            if s.active {
                s.active = false;
                eprint!("\r\ninkline: internal error, disabled for this shell (see `inkline status`)\r\n");
            }
            fallback
        }
    }
}

fn width(ch: char) -> i32 {
    unsafe { wcwidth(ch as i32) }
}

unsafe fn line() -> &'static [u8] {
    unsafe {
        if rl_line_buffer.is_null() || rl_end <= 0 {
            return &[];
        }
        std::slice::from_raw_parts(rl_line_buffer as *const u8, rl_end as usize)
    }
}

fn cursor() -> (c_int, c_int) {
    unsafe { (_rl_last_v_pos, _rl_last_c_pos) }
}

unsafe fn emit(bytes: &[u8]) {
    unsafe {
        if !bytes.is_empty() && !rl_outstream.is_null() {
            fwrite(bytes.as_ptr().cast(), 1, bytes.len(), rl_outstream);
        }
    }
}

/// readline's redisplay, after writing what `out` holds so far (the
/// synchronized-update start). readline flushes its stream when it is done.
/// (Capturing readline's output to send one write per frame was measured to
/// add ~40 µs per keystroke; terminals with synchronized updates already show
/// the frame atomically.)
unsafe fn redisplay_into(out: &mut Vec<u8>) {
    unsafe {
        emit(out);
        out.clear();
        rl_redisplay();
    }
}

/// bash's own value of a shell variable (the process environment can be stale).
fn bash_var(name: &CStr) -> Option<Vec<u8>> {
    unsafe {
        let v = get_string_value(name.as_ptr());
        (!v.is_null()).then(|| CStr::from_ptr(v).to_bytes().to_vec())
    }
}

fn in_vi_command() -> bool {
    unsafe { rl_get_keymap() == rl_get_keymap_by_name(c"vi-command".as_ptr()) }
}

// ---------------------------------------------------------------- suggestions

/// The current suggestion (suffix after the buffer), or None.
unsafe fn suggestion() -> Option<Vec<u8>> {
    unsafe {
        let buf = line();
        if buf.is_empty() || !st().active {
            return None;
        }
        match &st().cleared_for {
            Some(c) if c.as_slice() == buf => return None,
            Some(_) => st().cleared_for = None,
            None => {}
        }
        let (base, len) = (history_base, history_length);
        let newest = if len > 0 { history_get(base + len - 1) as usize } else { 0 };
        let entries = move || -> Box<dyn Iterator<Item = &'static [u8]>> {
            Box::new((0..len).rev().filter_map(move |i| {
                let e = history_get(base + i);
                if e.is_null() || (*e).line.is_null() { None } else { Some(CStr::from_ptr((*e).line).to_bytes()) }
            }))
        };
        st().cache.get(buf, (base, len, newest), entries).map(<[u8]>::to_vec)
    }
}

/// Temporarily append `extra` to the line buffer (no undo record); returns
/// the real end to pass to `restore_buffer`.
unsafe fn extend_buffer(extra: &[u8]) -> c_int {
    unsafe {
        let real_end = rl_end;
        rl_extend_line_buffer(real_end + extra.len() as c_int + 1);
        std::ptr::copy_nonoverlapping(extra.as_ptr(), (rl_line_buffer as *mut u8).add(real_end as usize), extra.len());
        rl_end = real_end + extra.len() as c_int;
        *rl_line_buffer.add(rl_end as usize) = 0;
        real_end
    }
}

unsafe fn restore_buffer(real_end: c_int) {
    unsafe {
        rl_end = real_end;
        *rl_line_buffer.add(real_end as usize) = 0;
    }
}

unsafe fn insert(text: &[u8]) {
    if let Ok(t) = CString::new(text) {
        unsafe { rl_insert_text(t.as_ptr()) };
    }
}

// ------------------------------------------------------------------- display

struct BashResolver;

fn expand_tilde(w: &str) -> Cow<'_, str> {
    if w == "~" || w.starts_with("~/") {
        if let Some(home) = bash_var(c"HOME") {
            return Cow::Owned(format!("{}{}", String::from_utf8_lossy(&home), &w[1..]));
        }
    }
    Cow::Borrowed(w)
}

/// Names in the `$PATH` directories, so "not a command" needs no syscalls.
/// Validated once per prompt (PATH value and directory mtimes); a hit is
/// confirmed with bash's own `find_user_command`.
#[derive(Default)]
struct PathIndex {
    path: Vec<u8>,
    checked: bool,
    dirs: std::collections::HashMap<PathBuf, (SystemTime, Vec<Vec<u8>>)>,
    names: std::collections::HashSet<Vec<u8>>,
    valid: bool,
}

impl PathIndex {
    fn ensure(&mut self) {
        if self.checked && self.valid {
            return;
        }
        self.checked = true;
        let path = bash_var(c"PATH").unwrap_or_default();
        let entries: Vec<PathBuf> = path
            .split(|&b| b == b':')
            .map(|d| if d.is_empty() { PathBuf::from(".") } else { PathBuf::from(OsStr::from_bytes(d)) })
            .collect();
        let mut changed = path != self.path || !self.valid;
        for d in &entries {
            let mtime = std::fs::metadata(d).and_then(|m| m.modified()).ok();
            match (mtime, self.dirs.get(d)) {
                (Some(t), Some((old, _))) if *old == t => {}
                (Some(t), _) => {
                    let names = std::fs::read_dir(d)
                        .map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().into_encoded_bytes()).collect())
                        .unwrap_or_default();
                    self.dirs.insert(d.clone(), (t, names));
                    changed = true;
                }
                (None, Some(_)) => {
                    self.dirs.remove(d);
                    changed = true;
                }
                (None, None) => {}
            }
        }
        if changed {
            self.names = entries.iter().filter_map(|d| self.dirs.get(d)).flat_map(|(_, n)| n.iter().cloned()).collect();
            self.path = path;
        }
        self.valid = true;
    }
}

fn memoized(kind: u8, word: &str, f: impl FnOnce() -> u8) -> u8 {
    if let Some(&hit) = st().memo[kind as usize].get(word) {
        return hit;
    }
    let v = f();
    let memo = &mut st().memo[kind as usize];
    if memo.len() < 4096 {
        memo.insert(word.to_owned(), v);
    }
    v
}

fn is_exec(m: &std::fs::Metadata) -> bool {
    m.permissions().mode() & 0o111 != 0
}

impl highlight::Resolver for BashResolver {
    fn command(&self, word: &str) -> bool {
        memoized(0, word, || command_lookup(word) as u8) != 0
    }

    fn path_kind(&self, word: &str) -> PathKind {
        let k = memoized(1, word, || {
            let p = expand_tilde(word);
            match std::fs::metadata(&*p) {
                Ok(m) if m.is_dir() => 1,
                Ok(m) if m.is_file() && is_exec(&m) => 2,
                Ok(_) => 3,
                Err(_) if std::fs::symlink_metadata(&*p).is_ok() => 3,
                Err(_) => 0,
            }
        });
        [PathKind::Missing, PathKind::Dir, PathKind::Exec, PathKind::Other][k as usize]
    }

    fn path_prefix(&self, word: &str, command_position: bool) -> bool {
        memoized(2 + command_position as u8, word, || path_prefix_lookup(word, command_position) as u8) != 0
    }

    fn autocd(&self) -> bool {
        unsafe { autocd != 0 }
    }
}

fn command_lookup(word: &str) -> bool {
    if word.contains('/') {
        return std::fs::metadata(&*expand_tilde(word)).is_ok_and(|m| m.is_file() && is_exec(&m));
    }
    let Ok(name) = CString::new(word) else { return false };
    unsafe {
        if !find_alias(name.as_ptr()).is_null() || !find_function(name.as_ptr()).is_null() || !find_shell_builtin(name.as_ptr()).is_null() {
            return true;
        }
        let index = &mut st().path_index;
        index.ensure();
        if !index.names.contains(word.as_bytes()) {
            return false;
        }
        let path = find_user_command(name.as_ptr());
        if path.is_null() {
            return false;
        }
        free(path.cast());
        true
    }
}

fn path_prefix_lookup(word: &str, command_position: bool) -> bool {
    let full = expand_tilde(word);
    let (dir, base) = match full.rfind('/') {
        Some(0) => ("/", &full[1..]),
        Some(i) => (&full[..i], &full[i + 1..]),
        None => (".", &full[..]),
    };
    let Ok(dir) = std::env::current_dir().map(|cwd| cwd.join(dir)) else { return false };
    let Ok(mtime) = std::fs::metadata(&dir).and_then(|m| m.modified()) else { return false };
    let dirs = &mut st().dirs;
    let idx = match dirs.iter().position(|(p, t, _)| *p == dir && *t == mtime) {
        Some(i) => i,
        None => {
            let Ok(rd) = std::fs::read_dir(&dir) else { return false };
            let names = rd.filter_map(Result::ok).take(20_000).map(|e| e.file_name().into_encoded_bytes()).collect();
            if dirs.len() >= 4 {
                dirs.remove(0);
            }
            dirs.push((dir.clone(), mtime, names));
            dirs.len() - 1
        }
    };
    let mut matches = dirs[idx].2.iter().filter(|n| n.starts_with(base.as_bytes()));
    if !command_position {
        return matches.next().is_some();
    }
    // zsh-s-h: in command position only executables and directories count.
    matches.take(64).any(|n| std::fs::metadata(dir.join(OsStr::from_bytes(n))).is_ok_and(|m| m.is_dir() || is_exec(&m)))
}

unsafe fn decorations_allowed() -> bool {
    unsafe {
        _rl_horizontal_scroll_mode == 0
            && rl_readline_state & RL_STATE_LINE_OFFSCREEN == 0
            && !_rl_term_up.is_null()
            && *_rl_term_up != 0
            && !rl_line_buffer.is_null()
            && !rl_display_prompt.is_null()
    }
}

/// Recolour what readline just drew. `full` is the buffer readline drew (real
/// text followed by the suggestion, if any). Returns false, writing nothing,
/// if the computed layout disagrees with readline's.
unsafe fn decorate(out: &mut Vec<u8>, full: &[u8], real_end: usize) -> bool {
    unsafe {
        let (mut rows, mut cols) = (0, 0);
        rl_get_screen_size(&mut rows, &mut cols);
        let (v, c) = cursor();
        let opts = layout::Opts {
            cols: cols as i64,
            multibyte: _rl_utf8locale != 0 && rl_byte_oriented == 0,
            output_meta: _rl_output_meta_chars != 0,
        };
        let prompt = CStr::from_ptr(rl_display_prompt).to_bytes();
        let modmark = (_rl_mark_modified_lines != 0 && !current_history().is_null() && !rl_undo_list.is_null()) as i64;
        let start = layout::prompt_width(prompt, opts.multibyte, &width) + modmark;
        let point = (rl_point.max(0) as usize).min(full.len());
        let Some(lay) = layout::layout_with_mark(full, point, real_end, start, &opts, &width) else { return false };
        let offset = v as i64 - lay.point.0;
        if lay.point.1 != c as i64 || offset < 0 || lay.end.0 + offset != _rl_vis_botlin as i64 {
            return false;
        }
        let real = &full[..real_end];
        let styles = highlight::highlight(real, &BashResolver, st().finishing);
        let region = (rl_mark_active_p() != 0).then(|| (rl_mark.min(rl_point).max(0) as usize, rl_mark.max(rl_point).max(0) as usize));
        let sgr: Vec<&'static str> = lay
            .cells
            .iter()
            .map(|cell| match region {
                _ if cell.start >= real_end => GHOST_SGR,
                // readline draws the active region (e.g. a fresh paste) in standout.
                Some((a, b)) if cell.start >= a && cell.start < b => styles[cell.start].sgr_standout(),
                _ => styles[cell.start].sgr(),
            })
            .collect();
        paint::repaint(out, &lay.cells, &sgr, offset, (v as i64, c as i64), cols as i64);
        if real_end < full.len() {
            st().shown = Some(Shown { cursor: (v, c), bottom_row: _rl_vis_botlin, real_end: (lay.mark.0 + offset, lay.mark.1) });
        }
        true
    }
}

/// With a custom redisplay function installed, readline's `rl_resize_terminal`
/// skips its own `_rl_redisplay_after_sigwinch` and only forces a full update,
/// which would reprint a multi-line prompt from wherever the cursor is. Do what
/// readline would have done: go back to the line's first row, clear below, and
/// redraw just the prompt's last line (which comes back here as a normal frame).
unsafe fn handle_resize() -> bool {
    unsafe {
        let (mut rows, mut cols) = (0, 0);
        rl_get_screen_size(&mut rows, &mut cols);
        let Some((size, (v, _))) = st().last_frame else { return false };
        if size == (rows, cols) {
            return false;
        }
        st().last_frame = Some(((rows, cols), (0, 0)));
        st().resizes += 1;
        let mut out = Vec::new();
        if v > 0 {
            let _ = write!(out, "\x1b[{v}A");
        }
        out.extend_from_slice(b"\r\x1b[J");
        emit(&out);
        fflush(rl_outstream);
        rl_redraw_prompt_last_line();
        true
    }
}

unsafe fn frame() {
    unsafe {
        unwrap_last_func();
        ensure_bound();
        if handle_resize() {
            return;
        }
        let t0 = Instant::now();
        let mut out = Vec::with_capacity(1024);
        out.extend_from_slice(b"\x1b[?2026h");
        st().shown = None;
        let allowed = decorations_allowed();
        let searching = rl_readline_state & RL_STATE_SEARCHING != 0;
        let ghost = if allowed && !searching && !st().hide { suggestion() } else { None };
        let real_end = match &ghost {
            Some(g) => extend_buffer(g),
            None => rl_end,
        };
        let full = line().to_vec();
        let t_readline = Instant::now();
        redisplay_into(&mut out);
        let mut readline_ns = t_readline.elapsed().as_nanos();
        restore_buffer(real_end);

        let t_compute = Instant::now();
        if allowed {
            if decorate(&mut out, &full, real_end as usize) {
                st().painted += 1;
            } else {
                st().mismatches += 1;
                st().shown = None;
                if ghost.is_some() {
                    // Never leave an uncoloured suggestion on screen: let readline erase it.
                    let t = Instant::now();
                    redisplay_into(&mut out);
                    readline_ns += t.elapsed().as_nanos();
                    let real = line().to_vec();
                    decorate(&mut out, &real, real.len());
                }
            }
        }
        let compute_ns = t_compute.elapsed().as_nanos();
        out.extend_from_slice(b"\x1b[?2026l");
        emit(&out);
        fflush(rl_outstream);
        let (mut rows, mut cols) = (0, 0);
        rl_get_screen_size(&mut rows, &mut cols);
        st().last_frame = Some(((rows, cols), cursor()));
        let s = st();
        let ns = t0.elapsed().as_nanos().saturating_sub(readline_ns);
        s.frames += 1;
        s.work_ns_total += ns;
        s.work_ns_max = s.work_ns_max.max(ns);
        s.compute_ns_total += compute_ns;
        s.compute_ns_max = s.compute_ns_max.max(compute_ns);
    }
}

unsafe extern "C" fn inkline_redisplay() {
    let decorated = guard(false, || {
        if !st().active {
            return false;
        }
        unsafe { frame() };
        true
    });
    if !decorated {
        unsafe { rl_redisplay() };
    }
}

/// Redraw once with no suggestion (readline erases it from the screen).
fn redraw_hidden(finishing: bool) {
    if !st().active {
        return;
    }
    let s = st();
    s.hide = true;
    s.finishing = finishing;
    unsafe { inkline_redisplay() };
    let s = st();
    s.hide = false;
    s.finishing = false;
}

/// readline is leaving the line without our accept-line path (Ctrl-C): clear
/// from the end of the real text down, so the suggestion is not left behind.
/// On a signal readline echoes `^C` at the cursor and then moves down to the
/// bottom row of its line with newlines (`_rl_clean_up_for_exit`, column 0)
/// before calling this hook.
unsafe extern "C" fn inkline_deprep() {
    guard((), || unsafe {
        if let Some(sh) = st().shown.take() {
            let (row, col) = cursor();
            let in_signal = rl_readline_state & RL_STATE_SIGHANDLER != 0;
            if (row, col) == sh.cursor || (in_signal && row == sh.bottom_row) {
                // Keep the `^C` readline just echoed when it sits where the suggestion began.
                let mut start = sh.real_end;
                let at_end = (sh.cursor.0 as i64, sh.cursor.1 as i64) == sh.real_end;
                if in_signal && at_end && _rl_echoctl != 0 && _rl_echo_control_chars != 0 {
                    let (mut rows, mut cols) = (0, 0);
                    rl_get_screen_size(&mut rows, &mut cols);
                    let ch = _rl_intr_char;
                    let echoed = if (0..0x20).contains(&ch) || ch == 0x7f { 2 } else { 1 };
                    start.1 += echoed;
                    if cols > 0 && start.1 >= cols as i64 {
                        start = (start.0 + 1, start.1 - cols as i64);
                    }
                }
                let mut out = Vec::new();
                let up = row as i64 - start.0;
                if up > 0 {
                    let _ = write!(out, "\x1b[{up}A");
                } else if up < 0 {
                    let _ = write!(out, "\x1b[{}B", -up);
                }
                out.push(b'\r');
                if start.1 > 0 {
                    let _ = write!(out, "\x1b[{}C", start.1);
                }
                out.extend_from_slice(b"\x1b[J");
                emit(&out);
                fflush(rl_outstream);
            }
        }
    });
    if let Some(orig) = st().orig_deprep {
        unsafe { orig() };
    }
}

// ------------------------------------------------------------- key wrappers

unsafe fn call(orig: RlFn, count: c_int, key: c_int) -> c_int {
    unsafe {
        unwrap_last_func();
        orig(count, key)
    }
}

unsafe fn dispatch(slot: usize, count: c_int, key: c_int) -> c_int {
    unsafe {
        let Some(&(orig, kind)) = st().slots.get(slot) else { return 0 };
        if !st().active {
            return call(orig, count, key);
        }
        match kind {
            Kind::Accept => accept(Some(orig), count, key),
            Kind::Partial => partial(orig, count, key),
            Kind::ClearAfter => {
                let r = call(orig, count, key);
                st().cleared_for = Some(line().to_vec());
                r
            }
            Kind::AcceptLine => {
                guard((), || redraw_hidden(true));
                call(orig, count, key)
            }
            // No synchronized-update block around the original: it may wait
            // for input ("Display all 300 possibilities?").
            Kind::HideBefore => {
                guard((), || redraw_hidden(false));
                call(orig, count, key)
            }
        }
    }
}

/// `_zsh_autosuggest_accept`.
unsafe fn accept(orig: Option<RlFn>, count: c_int, key: c_int) -> c_int {
    unsafe {
        let vicmd = in_vi_command();
        let max = rl_end - vicmd as c_int;
        let sfx = guard(None, || if rl_point == max && rl_readline_state & RL_STATE_VIMOTION == 0 { suggestion() } else { None });
        let Some(sfx) = sfx else {
            return orig.map_or(0, |f| call(f, count, key));
        };
        rl_point = rl_end;
        insert(&sfx);
        let r = orig.map_or(0, |f| call(f, count, key));
        rl_point = if in_vi_command() { (rl_end - 1).max(0) } else { rl_end };
        r
    }
}

/// `_zsh_autosuggest_partial_accept`: run the motion over buffer + suggestion
/// and keep the part of the suggestion the cursor moved over.
unsafe fn partial(orig: RlFn, count: c_int, key: c_int) -> c_int {
    unsafe {
        let sfx = guard(None, || if rl_readline_state & RL_STATE_VIMOTION == 0 { suggestion() } else { None });
        let Some(sfx) = sfx else { return call(orig, count, key) };
        let real_end = extend_buffer(&sfx);
        let r = call(orig, count, key);
        let moved_to = rl_point;
        restore_buffer(real_end);
        let loc = moved_to + in_vi_command() as c_int;
        if loc > real_end {
            rl_point = real_end;
            insert(&sfx[..(loc - real_end) as usize]);
        }
        rl_point = moved_to.min(rl_end);
        r
    }
}

/// zsh-autosuggestions' `autosuggest-accept` (bound to Ctrl-Y).
unsafe extern "C" fn w_accept(count: c_int, key: c_int) -> c_int {
    unsafe { accept(None, count, key) }
}

macro_rules! slots {
    ($($n:literal),*) => {
        const SLOT_FNS: &[RlFn] = &[$({
            unsafe extern "C" fn f(count: c_int, key: c_int) -> c_int {
                unsafe { dispatch($n, count, key) }
            }
            f
        }),*];
    };
}
slots!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
    38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63
);

/// readline records the dispatched function in `rl_last_func` and some
/// commands look at it (a second Tab lists matches, menu-complete keeps
/// cycling, history-search keeps its prefix). Report the original command.
unsafe fn unwrap_last_func() {
    unsafe {
        let Some(last) = rl_last_func else { return };
        if let Some(i) = SLOT_FNS.iter().position(|f| *f as *const () == last as *const ()) {
            if let Some(&(orig, _)) = st().slots.get(i) {
                rl_last_func = Some(orig);
            }
        }
    }
}

/// Point every key bound to a wrapped command at its slot, plus Ctrl-Y.
/// Deferred until readline has read its inputrc, so ours win.
unsafe fn ensure_bound() {
    unsafe {
        if st().bound || rl_readline_state & RL_STATE_INITIALIZED == 0 {
            return;
        }
        st().bound = true;
        for &(name, kind) in WRAPPED {
            let Ok(cname) = CString::new(name) else { continue };
            let Some(orig) = rl_named_function(cname.as_ptr()) else { continue };
            if SLOT_FNS.iter().any(|f| *f as *const () == orig as *const ()) {
                continue;
            }
            let slot = match st().slots.iter().position(|(f, _)| *f as *const () == orig as *const ()) {
                Some(i) => i,
                None if st().slots.len() < SLOT_FNS.len() => {
                    st().slots.push((orig, kind));
                    st().slots.len() - 1
                }
                None => continue,
            };
            for map_name in [c"emacs-standard", c"vi-insert", c"vi-command"] {
                let map = rl_get_keymap_by_name(map_name.as_ptr());
                if map.is_null() {
                    continue;
                }
                let seqs = rl_invoking_keyseqs_in_map(orig, map);
                if seqs.is_null() {
                    continue;
                }
                let mut i = 0;
                while !(*seqs.add(i)).is_null() {
                    let seq = *seqs.add(i);
                    st().rebinds.push((map, CStr::from_ptr(seq).to_owned(), Some(orig)));
                    rl_bind_keyseq_in_map(seq, Some(SLOT_FNS[slot]), map);
                    free(seq.cast());
                    i += 1;
                }
                free(seqs.cast());
            }
        }
        for map_name in [c"emacs-standard", c"vi-insert"] {
            let map = rl_get_keymap_by_name(map_name.as_ptr());
            if map.is_null() {
                continue;
            }
            let mut kind = 0;
            let prev = rl_function_of_keyseq(c"\x19".as_ptr(), map, &mut kind);
            st().rebinds.push((map, c"\\C-y".to_owned(), if kind == 0 { prev } else { None }));
            rl_bind_keyseq_in_map(c"\\C-y".as_ptr(), Some(w_accept), map);
        }
        rl_add_defun(c"inkline-accept".as_ptr(), w_accept, -1);
    }
}

// ------------------------------------------------------------------- builtin

/// readline skips terminfo when a custom redisplay function is set during its
/// terminal setup (it assumes the application draws everything itself), which
/// would leave it in single-line, no-cursor-motion mode. So the redisplay hook
/// is only installed once readline is initialized, and if bash re-initializes
/// the terminal later (assigning TERM does), the setup is redone without us.
unsafe fn hook_when_ready() {
    unsafe {
        if rl_readline_state & RL_STATE_INITIALIZED == 0 {
            return;
        }
        if st().hooked && (_rl_term_up.is_null() || *_rl_term_up == 0) {
            rl_redisplay_function = Some(rl_redisplay);
            rl_reset_terminal(std::ptr::null());
            st().repairs += 1;
        }
        rl_redisplay_function = Some(inkline_redisplay);
        st().hooked = true;
        ensure_bound();
    }
}

/// Runs at the start of every readline() call, after rl_initialize.
unsafe extern "C" fn inkline_prep(meta: c_int) {
    guard((), || unsafe {
        let s = st();
        s.memo.iter_mut().for_each(|m| m.clear());
        s.path_index.checked = false;
        s.last_frame = None;
        s.cleared_for = None;
        s.shown = None;
        hook_when_ready()
    });
    if let Some(orig) = st().orig_prep {
        unsafe { orig(meta) };
    }
}

/// Keep the library mapped even after `enable -d`: readline keymaps and the
/// funmap may still point at our functions.
unsafe fn pin_library() {
    unsafe {
        let mut info = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
        if dladdr(inkline_builtin_load as *const c_void, &mut info) != 0 && !info.fname.is_null() {
            dlopen(info.fname, RTLD_NOW | RTLD_NOLOAD | RTLD_NODELETE);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn inkline_builtin_load(_name: *const c_char) -> c_int {
    guard(0, || unsafe {
        if st().installed {
            st().active = true;
            return 1;
        }
        let v = rl_readline_version;
        if !(0x0800..0x0900).contains(&v) {
            eprintln!("inkline: needs GNU readline 8.x, bash uses {}.{}; not loaded", v >> 8, v & 0xff);
            return 0;
        }
        let current = rl_redisplay_function.map(|f| f as *const () as usize);
        if current.is_some_and(|f| f != rl_redisplay as *const () as usize) {
            eprintln!("inkline: another extension owns readline's redisplay hook; not loaded");
            return 0;
        }
        pin_library();
        let s = st();
        s.orig_deprep = rl_deprep_term_function;
        s.orig_prep = rl_prep_term_function;
        rl_deprep_term_function = Some(inkline_deprep);
        rl_prep_term_function = Some(inkline_prep);
        s.installed = true;
        s.active = true;
        hook_when_ready();
        1
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn inkline_builtin_unload(_name: *const c_char) {
    guard((), || unsafe {
        redraw_hidden(false);
        let s = st();
        s.active = false;
        for (map, seq, orig) in s.rebinds.drain(..).rev() {
            rl_bind_keyseq_in_map(seq.as_ptr(), orig, map);
        }
        s.bound = false;
        s.slots.clear();
        if rl_redisplay_function.map(|f| f as *const () as usize) == Some(inkline_redisplay as *const () as usize) {
            rl_redisplay_function = Some(rl_redisplay);
        }
        if rl_deprep_term_function.map(|f| f as *const () as usize) == Some(inkline_deprep as *const () as usize) {
            rl_deprep_term_function = s.orig_deprep;
        }
        if rl_prep_term_function.map(|f| f as *const () as usize) == Some(inkline_prep as *const () as usize) {
            rl_prep_term_function = s.orig_prep;
        }
        s.hooked = false;
        s.installed = false;
    });
}

fn status() {
    let s = st();
    let state = match (s.installed, s.active) {
        (true, true) => "active",
        (true, false) => "off",
        _ => "not installed",
    };
    let v = unsafe { rl_readline_version };
    let per = |total: u128| if s.frames > 0 { total as f64 / s.frames as f64 / 1e3 } else { 0.0 };
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "inkline {} ({state}), readline {}.{}", env!("CARGO_PKG_VERSION"), v >> 8, v & 0xff);
    let _ = writeln!(
        o,
        "frames {}  highlighted {}  layout mismatches {}  terminal repairs {}  resizes {}  internal errors {}  wrapped commands {}",
        s.frames,
        s.painted,
        s.mismatches,
        s.repairs,
        s.resizes,
        s.panics,
        s.slots.len()
    );
    let _ = writeln!(
        o,
        "per keystroke: highlight+suggest avg {:.1} µs (max {:.1}); incl. terminal output avg {:.1} µs (max {:.1})",
        per(s.compute_ns_total),
        s.compute_ns_max as f64 / 1e3,
        per(s.work_ns_total),
        s.work_ns_max as f64 / 1e3
    );
    let _ = o.flush();
}

unsafe extern "C" fn inkline_builtin(list: *mut WordList) -> c_int {
    guard(1, || unsafe {
        let mut args = Vec::new();
        let mut l = list;
        while !l.is_null() {
            if !(*l).word.is_null() && !(*(*l).word).word.is_null() {
                args.push(CStr::from_ptr((*(*l).word).word).to_string_lossy().into_owned());
            }
            l = (*l).next;
        }
        match args.first().map(String::as_str) {
            None | Some("status") => {
                status();
                0
            }
            Some("on") => {
                let s = st();
                s.active = s.installed;
                if s.active { 0 } else { 1 }
            }
            Some("off") => {
                redraw_hidden(false);
                st().active = false;
                0
            }
            Some(other) => {
                eprintln!("inkline: unknown argument `{other}`; usage: inkline [status|on|off]");
                EX_USAGE
            }
        }
    })
}

static mut LONG_DOC: [*const c_char; 4] = [
    c"Zsh-style autosuggestions and syntax highlighting for readline.".as_ptr(),
    c"  status  show state and per-keystroke cost (default)".as_ptr(),
    c"  on|off  enable or disable decorations in this shell".as_ptr(),
    std::ptr::null(),
];

#[unsafe(no_mangle)]
pub static mut inkline_struct: Builtin = Builtin {
    name: c"inkline".as_ptr(),
    function: Some(inkline_builtin),
    flags: BUILTIN_ENABLED,
    long_doc: unsafe { LONG_DOC.as_ptr() },
    short_doc: c"inkline [status|on|off]".as_ptr(),
    handle: std::ptr::null_mut(),
};
