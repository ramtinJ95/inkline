//! Syntax highlighting modelled on zsh-syntax-highlighting's `main` highlighter
//! (default styles), reduced to bash syntax. Works on bytes; every byte of the
//! input gets a `Style`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    /// Command word that does not resolve (zsh-s-h `unknown-token`).
    Unknown,
    Reserved,
    /// alias, function, builtin or external command.
    Command,
    /// sudo, command, exec, ... (zsh-s-h `precommand`).
    Precommand,
    /// Directory in command position with `shopt -s autocd`.
    Autodirectory,
    /// Argument naming an existing path (or a prefix of one at the cursor).
    Path,
    Glob,
    History,
    Quoted,
    /// `$var` / `\"` inside double quotes, `\n` inside `$'...'`.
    QuotedExpansion,
    Redirection,
    /// zsh-s-h uses `fg=black,bold`, but Omarchy (like many themes) sets
    /// palette 0 to the background, which hides comments. Bright black is the
    /// theme's muted colour; bold keeps them apart from the suggestion (`fg=8`).
    Comment,
    /// `$(`, `)`, backquotes, `<(`.
    Delimiter,
}

impl Style {
    /// SGR parameters, using the 16 base colours so the terminal theme applies.
    pub fn sgr(self) -> &'static str {
        match self {
            Style::Plain => "",
            Style::Unknown => "1;31",
            Style::Reserved => "33",
            Style::Command => "32",
            Style::Precommand | Style::Autodirectory => "32;4",
            Style::Path => "4",
            Style::Glob | Style::History => "34",
            Style::Quoted | Style::Redirection => "33",
            Style::QuotedExpansion => "36",
            Style::Comment => "1;90",
            Style::Delimiter => "35",
        }
    }

    /// `sgr` plus standout, for cells in readline's active region.
    pub fn sgr_standout(self) -> &'static str {
        match self {
            Style::Plain => "7",
            Style::Unknown => "1;31;7",
            Style::Reserved => "33;7",
            Style::Command => "32;7",
            Style::Precommand | Style::Autodirectory => "32;4;7",
            Style::Path => "4;7",
            Style::Glob | Style::History => "34;7",
            Style::Quoted | Style::Redirection => "33;7",
            Style::QuotedExpansion => "36;7",
            Style::Comment => "1;90;7",
            Style::Delimiter => "35;7",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Missing,
    Dir,
    /// Executable regular file.
    Exec,
    /// Exists (or is a symlink), neither of the above.
    Other,
}

/// Lookups the highlighter needs from the shell and filesystem. Paths are
/// tilde-expanded by the implementation.
pub trait Resolver {
    /// alias, function, builtin, PATH command, or executable path (word contains `/`).
    fn command(&self, word: &str) -> bool;
    fn path_kind(&self, word: &str) -> PathKind;
    /// Some entry of the word's (existing) directory starts with its last
    /// component; in command position only executables and directories count.
    fn path_prefix(&self, word: &str, command_position: bool) -> bool;
    /// bash `shopt autocd`.
    fn autocd(&self) -> bool;
}

const RESERVED: &[&str] = &[
    "!", "[[", "]]", "{", "}", "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for",
    "function", "if", "in", "select", "then", "time", "until", "while",
];
/// zsh-syntax-highlighting 0.8.0 `precommand_options`: (name, options taking
/// an argument, options after which no command follows). `noglob` and `-` are
/// zsh-only and left out.
const PRECOMMANDS: &[(&str, &str, &str)] = &[
    ("builtin", "", ""),
    ("command", "", "vV"),
    ("exec", "a", ""),
    ("doas", "aCu", ""),
    ("nice", "n", ""),
    ("pkexec", "", ""),
    ("sudo", "Cgprtu", "eKkVv"),
    ("stdbuf", "ioe", ""),
    ("eatmydata", "", ""),
    ("catchsegv", "", ""),
];
const REDIRECTIONS: &[&[u8]] = &[
    b"&>>", b"&>", b"<<<", b"<<-", b"<<", b"<>", b"<&", b">>", b">&", b">|", b"<", b">",
];

/// `finishing`: the line is being accepted (zsh-s-h's `zle-line-finish` pass),
/// so no path-prefix underline is left in scrollback.
pub fn highlight(src: &[u8], resolver: &dyn Resolver, finishing: bool) -> Vec<Style> {
    let mut h = Highlighter { s: src, out: vec![Style::Plain; src.len()], r: resolver, finishing, overlays: Vec::new() };
    h.cmdline(0, None);
    h.out
}

#[derive(Default)]
struct Ctx {
    expect_cmd: bool,
    redirect_target: bool,
    in_test: bool,
    /// After a precommand: (options taking an argument, options ending command position).
    precommand: Option<(&'static str, &'static str)>,
    precommand_opt_arg: bool,
}

impl Ctx {
    fn new() -> Self {
        Ctx { expect_cmd: true, ..Default::default() }
    }
    fn separator(&mut self) {
        *self = Ctx::new();
    }
}

struct Word {
    start: usize,
    end: usize,
    /// Unquoted, unexpanded value (best effort) used for lookups.
    literal: String,
    /// Byte ranges that are not inside quotes/expansions; they take the word's class style.
    runs: Vec<(usize, usize)>,
    quoted: bool,
    dynamic: bool,
    /// Unquoted `*` / `?` positions.
    globs: Vec<usize>,
}

struct Highlighter<'a> {
    s: &'a [u8],
    out: Vec<Style>,
    r: &'a dyn Resolver,
    finishing: bool,
    /// Paints from `classify` that go on top of the word's class style.
    overlays: Vec<(usize, usize, Style)>,
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl Highlighter<'_> {
    fn paint(&mut self, a: usize, b: usize, st: Style) {
        let b = b.min(self.s.len());
        if a < b {
            self.out[a..b].fill(st);
        }
    }

    /// Highlight a command line until `close` (unquoted) or end; returns the index of `close`.
    fn cmdline(&mut self, mut i: usize, close: Option<u8>) -> usize {
        let s = self.s;
        let mut ctx = Ctx::new();
        while i < s.len() {
            let c = s[i];
            if Some(c) == close {
                return i;
            }
            match c {
                b' ' | b'\t' => i += 1,
                b'\n' => {
                    ctx.separator();
                    i += 1;
                }
                b'#' => {
                    let e = s[i..].iter().position(|&b| b == b'\n').map_or(s.len(), |p| i + p);
                    self.paint(i, e, Style::Comment);
                    i = e;
                }
                b'&' if s.get(i + 1) == Some(&b'>') => i = self.redirection(i, &mut ctx),
                b';' | b'&' | b'|' => {
                    let mut n = 1;
                    while n < 3 && i + n < s.len() && matches!(s[i + n], b';' | b'&' | b'|') {
                        n += 1;
                    }
                    i += n;
                    ctx.separator();
                }
                b'(' => {
                    let reserved = if ctx.expect_cmd { Style::Reserved } else { Style::Plain };
                    if s.get(i + 1) == Some(&b'(') {
                        let end = self.balanced(i, b'(', b')');
                        self.paint(i, i + 2, reserved);
                        if end >= i + 4 && s[end - 2..end] == *b"))" {
                            self.paint(end - 2, end, reserved);
                        }
                        i = end;
                    } else {
                        self.paint(i, i + 1, reserved);
                        let j = self.cmdline(i + 1, Some(b')'));
                        self.paint(j, j + 1, reserved);
                        i = (j + 1).min(s.len());
                    }
                    ctx.expect_cmd = false;
                }
                b')' => i += 1,
                b'<' | b'>' if s.get(i + 1) == Some(&b'(') => {
                    self.paint(i, i + 2, Style::Delimiter);
                    let j = self.cmdline(i + 2, Some(b')'));
                    self.paint(j, j + 1, Style::Delimiter);
                    i = (j + 1).min(s.len());
                    ctx.expect_cmd = false;
                }
                b'<' | b'>' => i = self.redirection(i, &mut ctx),
                b'0'..=b'9' if self.fd_redirect_at(i) => i = self.redirection(i, &mut ctx),
                _ => {
                    let w = self.scan_word(i, close);
                    let st = self.classify(&w, &mut ctx);
                    for &(a, b) in &w.runs {
                        self.paint(a, b, st);
                    }
                    for &g in &w.globs {
                        self.paint(g, g + 1, Style::Glob);
                    }
                    for (a, b, st) in std::mem::take(&mut self.overlays) {
                        self.paint(a, b, st);
                    }
                    i = w.end.max(i + 1);
                }
            }
        }
        i
    }

    fn fd_redirect_at(&self, i: usize) -> bool {
        let digits = self.s[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        matches!(self.s.get(i + digits), Some(b'<' | b'>'))
    }

    fn redirection(&mut self, start: usize, ctx: &mut Ctx) -> usize {
        let s = self.s;
        let mut i = start;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        let op = REDIRECTIONS.iter().find(|op| s[i..].starts_with(op)).map_or(1, |op| op.len());
        let dup = s[i + op - 1] == b'&';
        self.paint(i, i + op, Style::Redirection);
        i += op;
        if dup && i < s.len() && (s[i].is_ascii_digit() || s[i] == b'-') {
            while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'-') {
                i += 1;
            }
        } else {
            ctx.redirect_target = true;
        }
        i
    }

    /// `$((...))` at `start`: zsh-s-h's `arithmetic-expansion` has no default style.
    fn arithmetic(&mut self, start: usize) -> usize {
        self.balanced(start + 1, b'(', b')')
    }

    /// Index just past the `close` matching the `open` at `i` (or end of input).
    fn balanced(&self, i: usize, open: u8, close: u8) -> usize {
        let mut depth = 0usize;
        let mut j = i;
        while j < self.s.len() {
            let b = self.s[j];
            if b == b'\\' {
                j += 2;
                continue;
            }
            if b == open {
                depth += 1;
            } else if b == close {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            j += 1;
        }
        self.s.len()
    }

    fn scan_word(&mut self, start: usize, close: Option<u8>) -> Word {
        let s = self.s;
        let mut w = Word { start, end: start, literal: String::new(), runs: Vec::new(), quoted: false, dynamic: false, globs: Vec::new() };
        let mut lit: Vec<u8> = Vec::new();
        let mut run = start;
        let mut i = start;
        macro_rules! end_run {
            () => {
                if run < i {
                    w.runs.push((run, i));
                }
            };
        }
        while i < s.len() {
            let c = s[i];
            if Some(c) == close {
                break;
            }
            match c {
                b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b')' => break,
                b'(' => {
                    // `a=(1 2)` array assignment or extglob `@(a|b)`; otherwise a metachar.
                    if i > start && matches!(s[i - 1], b'=' | b'@' | b'!' | b'?' | b'*' | b'+') {
                        // `a=(1 2)` stays an assignment; extglob `@(a|b)` is a pattern.
                        w.dynamic |= s[i - 1] != b'=';
                        i = self.balanced(i, b'(', b')');
                        continue;
                    }
                    break;
                }
                b'\\' => {
                    if let Some(&n) = s.get(i + 1) {
                        lit.push(n);
                    }
                    i = (i + 2).min(s.len());
                }
                b'\'' => {
                    end_run!();
                    let close_at = s[i + 1..].iter().position(|&b| b == b'\'').map(|p| i + 1 + p);
                    let e = close_at.map_or(s.len(), |c| c + 1);
                    lit.extend_from_slice(&s[i + 1..close_at.unwrap_or(s.len())]);
                    self.paint(i, e, Style::Quoted);
                    w.quoted = true;
                    i = e;
                    run = i;
                }
                b'"' => {
                    end_run!();
                    let (e, dynamic) = self.dquote(i, &mut lit);
                    w.quoted = true;
                    w.dynamic |= dynamic;
                    i = e;
                    run = i;
                }
                b'`' => {
                    end_run!();
                    i = self.backquote(i);
                    w.dynamic = true;
                    run = i;
                }
                b'$' => match s.get(i + 1) {
                    Some(b'\'') => {
                        end_run!();
                        let mut j = i + 2;
                        let mut escapes = Vec::new();
                        while j < s.len() && s[j] != b'\'' {
                            if s[j] == b'\\' {
                                escapes.push(j);
                                j += 2;
                            } else {
                                j += 1;
                            }
                        }
                        let e = (j + 1).min(s.len());
                        self.paint(i, e, Style::Quoted);
                        for x in escapes {
                            self.paint(x, x + 2, Style::QuotedExpansion);
                        }
                        w.quoted = true;
                        w.dynamic = true;
                        i = e;
                        run = i;
                    }
                    Some(b'(') => {
                        end_run!();
                        i = self.dollar_paren(i);
                        w.dynamic = true;
                        run = i;
                    }
                    Some(b'{') => {
                        i = self.balanced(i + 1, b'{', b'}');
                        w.dynamic = true;
                    }
                    _ => {
                        i += 1;
                        w.dynamic = true;
                        if i < s.len() && is_name_byte(s[i]) {
                            while i < s.len() && is_name_byte(s[i]) {
                                i += 1;
                            }
                        } else if i < s.len() && b"@*#?$!-0".contains(&s[i]) {
                            i += 1;
                        }
                    }
                },
                b'*' | b'?' => {
                    w.globs.push(i);
                    lit.push(c);
                    i += 1;
                }
                _ => {
                    lit.push(c);
                    i += 1;
                }
            }
        }
        end_run!();
        w.end = i;
        w.literal = String::from_utf8_lossy(&lit).into_owned();
        w
    }

    /// `$(` command substitution or `$((` arithmetic at `i`; returns index after it.
    fn dollar_paren(&mut self, i: usize) -> usize {
        if self.s.get(i + 2) == Some(&b'(') {
            return self.arithmetic(i);
        }
        self.paint(i, i + 2, Style::Delimiter);
        let j = self.cmdline(i + 2, Some(b')'));
        self.paint(j, j + 1, Style::Delimiter);
        (j + 1).min(self.s.len())
    }

    fn backquote(&mut self, i: usize) -> usize {
        self.paint(i, i + 1, Style::Delimiter);
        let j = self.cmdline(i + 1, Some(b'`'));
        self.paint(j, j + 1, Style::Delimiter);
        (j + 1).min(self.s.len())
    }

    /// Double-quoted string at `i`; returns (index after it, contains expansions).
    fn dquote(&mut self, i: usize, lit: &mut Vec<u8>) -> (usize, bool) {
        let s = self.s;
        let mut dynamic = false;
        self.paint(i, i + 1, Style::Quoted);
        let mut j = i + 1;
        while j < s.len() {
            match s[j] {
                b'"' => {
                    self.paint(j, j + 1, Style::Quoted);
                    return (j + 1, dynamic);
                }
                b'\\' => {
                    let special = matches!(s.get(j + 1), Some(b'$' | b'`' | b'"' | b'\\'));
                    let st = if special { Style::QuotedExpansion } else { Style::Quoted };
                    self.paint(j, j + 2, st);
                    if let Some(&n) = s.get(j + 1) {
                        if !special {
                            lit.push(b'\\');
                        }
                        lit.push(n);
                    }
                    j = (j + 2).min(s.len());
                }
                b'`' => {
                    dynamic = true;
                    j = self.backquote(j);
                }
                b'$' => match s.get(j + 1) {
                    Some(b'(') => {
                        dynamic = true;
                        j = self.dollar_paren(j);
                    }
                    Some(b'{') => {
                        dynamic = true;
                        let e = self.balanced(j + 1, b'{', b'}');
                        self.paint(j, e, Style::QuotedExpansion);
                        j = e;
                    }
                    Some(&n) if is_name_byte(n) || b"@*#?$!-".contains(&n) => {
                        dynamic = true;
                        let mut e = j + 2;
                        if is_name_byte(n) {
                            while e < s.len() && is_name_byte(s[e]) {
                                e += 1;
                            }
                        }
                        self.paint(j, e, Style::QuotedExpansion);
                        j = e;
                    }
                    _ => {
                        self.paint(j, j + 1, Style::Quoted);
                        lit.push(b'$');
                        j += 1;
                    }
                },
                b => {
                    self.paint(j, j + 1, Style::Quoted);
                    lit.push(b);
                    j += 1;
                }
            }
        }
        (j, dynamic)
    }

    fn classify(&mut self, w: &Word, ctx: &mut Ctx) -> Style {
        let raw = &self.s[w.start..w.end];
        let lit = w.literal.clone();
        let lit = lit.as_str();
        let plain_word = !w.quoted && !w.dynamic;
        let at_end = w.end == self.s.len();

        if ctx.redirect_target {
            ctx.redirect_target = false;
            return self.argument(w, at_end);
        }
        if !ctx.expect_cmd {
            if ctx.in_test && plain_word && lit == "]]" {
                ctx.in_test = false;
                return Style::Reserved;
            }
            if is_history(raw) {
                return Style::History;
            }
            return self.argument(w, at_end);
        }
        if ctx.precommand_opt_arg {
            ctx.precommand_opt_arg = false;
            return Style::Plain;
        }
        if let Some((with_arg, solo)) = ctx.precommand {
            if plain_word && lit.starts_with('-') {
                if lit == "--" {
                    ctx.precommand = None;
                    return Style::Plain;
                }
                let flags: Vec<char> = lit.chars().skip(1).collect();
                for (k, f) in flags.iter().enumerate() {
                    if solo.contains(*f) {
                        ctx.expect_cmd = false;
                        break;
                    }
                    if with_arg.contains(*f) {
                        ctx.precommand_opt_arg = k + 1 == flags.len();
                        break;
                    }
                }
                return Style::Plain;
            }
        }
        if is_history(raw) || (raw.len() > 1 && raw[0] == b'^') {
            ctx.expect_cmd = false;
            return Style::History;
        }
        if plain_word && is_assignment(lit) {
            // `a=(...)`: the parentheses are reserved words (zsh-s-h).
            if let Some(p) = raw.windows(2).position(|x| x == b"=(") {
                self.overlays.push((w.start + p + 1, w.start + p + 2, Style::Reserved));
                if raw.last() == Some(&b')') {
                    self.overlays.push((w.end - 1, w.end, Style::Reserved));
                }
            }
            return Style::Plain;
        }
        if plain_word && RESERVED.contains(&lit) {
            match lit {
                "[[" => {
                    ctx.expect_cmd = false;
                    ctx.in_test = true;
                }
                "for" | "select" | "case" | "function" | "fi" | "done" | "esac" | "}" | "in" | "]]" => {
                    ctx.expect_cmd = false
                }
                _ => {}
            }
            return Style::Reserved;
        }
        if w.dynamic || lit.is_empty() {
            ctx.expect_cmd = false;
            return Style::Plain;
        }
        if !w.quoted {
            if let Some(&(_, with_arg, solo)) = PRECOMMANDS.iter().find(|p| p.0 == lit) {
                if self.r.command(lit) {
                    ctx.precommand = Some((with_arg, solo));
                    return Style::Precommand;
                }
            }
        }
        ctx.expect_cmd = false;
        ctx.precommand = None;
        if self.r.command(lit) {
            return Style::Command;
        }
        let prefix = || at_end && !self.finishing && self.r.path_prefix(lit, true);
        match self.r.path_kind(lit) {
            PathKind::Exec => Style::Path,
            PathKind::Dir if self.r.autocd() => Style::Autodirectory,
            _ if prefix() => Style::Path,
            _ => Style::Unknown,
        }
    }

    fn argument(&self, w: &Word, at_end: bool) -> Style {
        let lit = w.literal.as_str();
        if w.dynamic || lit.is_empty() || lit.starts_with('-') || !w.globs.is_empty() {
            return Style::Plain;
        }
        if self.r.path_kind(lit) != PathKind::Missing || (at_end && !self.finishing && self.r.path_prefix(lit, false)) {
            return Style::Path;
        }
        Style::Plain
    }
}

fn is_history(raw: &[u8]) -> bool {
    raw.len() > 1 && raw[0] == b'!' && !matches!(raw[1], b'=' | b'(' | b' ' | b'\t')
}

fn is_assignment(word: &str) -> bool {
    let Some(eq) = word.find('=') else { return false };
    let name = word[..eq].strip_suffix('+').unwrap_or(&word[..eq]);
    let name = name.split_once('[').map_or(name, |(n, rest)| if rest.ends_with(']') { n } else { "" });
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Resolver for Fake {
        fn command(&self, w: &str) -> bool {
            matches!(w, "ls" | "git" | "echo" | "cat" | "grep" | "[" | "y" | "./run.sh" | "sudo" | "command" | "nice" | "env")
        }
        fn path_kind(&self, w: &str) -> PathKind {
            match w {
                "/etc" | "~/dotfiles" | "/usr/bin" => PathKind::Dir,
                "/etc/hosts" | "README.md" => PathKind::Other,
                "tool" => PathKind::Exec,
                _ => PathKind::Missing,
            }
        }
        fn path_prefix(&self, w: &str, cmd: bool) -> bool {
            if cmd {
                "/usr/bin/".starts_with(w)
            } else {
                "/etc/hosts".starts_with(w) || "README.md".starts_with(w)
            }
        }
        fn autocd(&self) -> bool {
            false
        }
    }

    /// Compact rendering: one letter per byte.
    fn render(src: &str) -> String {
        highlight(src.as_bytes(), &Fake, false)
            .iter()
            .map(|s| match s {
                Style::Plain => '.',
                Style::Unknown => 'U',
                Style::Reserved => 'R',
                Style::Command => 'C',
                Style::Precommand => 'P',
                Style::Autodirectory => 'A',
                Style::Path => '_',
                Style::Glob => 'G',
                Style::History => 'H',
                Style::Quoted => 'q',
                Style::QuotedExpansion => 'v',
                Style::Redirection => '>',
                Style::Comment => '#',
                Style::Delimiter => 'D',
            })
            .collect()
    }

    #[track_caller]
    fn check(src: &str, want: &str) {
        assert_eq!(render(src), want, "\n src: {src:?}");
    }

    #[test]
    fn commands_and_unknown() {
        check("ls -la /etc", "CC.....____");
        check("lss", "UUU");
        check("git status", "CCC.......");
        check("./run.sh x", "CCCCCCCC..");
    }

    #[test]
    fn separators_reset_command_position() {
        check("ls; git x && nope | cat", "CC..CCC......UUUU...CCC");
        check("ls|cat", "CC.CCC");
    }

    #[test]
    fn quotes_and_expansions() {
        check("echo 'hi there'", "CCCC.qqqqqqqqqq");
        check("echo \"a $HOME b\"", "CCCC.qqqvvvvvqqq");
        check("echo \"\\\"x\"", "CCCC.qvvqq");
        check("echo $(ls)", "CCCC.DDCCD");
        check("echo `ls`", "CCCC.DCCD");
        check("echo \"$(ls)\"", "CCCC.qDDCCDq");
        check("echo $((1+2))", "CCCC.........");
    }

    #[test]
    fn unterminated_quote_highlights_to_end() {
        check("echo 'abc", "CCCC.qqqq");
        check("echo \"ab", "CCCC.qqq");
    }

    #[test]
    fn precommands_keep_command_position() {
        check("sudo ls", "PPPP.CC");
        check("sudo -u root ls", "PPPP.........CC");
        check("env FOO=1 ls", "CCC.........");
        check("FOO=bar git", "........CCC");
    }

    #[test]
    fn reserved_words() {
        check("if ls; then echo; fi", "RR.CC..RRRR.CCCC..RR");
        check("for f in *.md; do cat; done", "RRR......G.....RR.CCC..RRRR");
        check("[[ -f x ]]", "RR......RR");
    }

    #[test]
    fn redirections_globs_comments_history() {
        check("ls > out 2>&1", "CC.>......>>.");
        check("cat < /etc/hosts", "CCC.>.__________");
        check("ls *.md", "CC.G...");
        check("ls # note", "CC.######");
        check("!!", "HH");
        check("sudo !!", "PPPP.HH");
    }

    #[test]
    fn path_prefix_only_at_end_of_line() {
        check("cat /etc/ho", "CCC._______");
        check("cat /etc/ho x", "CCC..........");
    }

    #[test]
    fn subshells_and_process_substitution() {
        check("(ls)", "RCCR");
        check("diff <(ls) x", "UUUU.DDCCD..");
    }

    #[test]
    fn command_position_paths_and_prefixes() {
        check("tool", "____");
        check("/usr/bi", "_______");
        check("/usr/bin", "________");
        check("/usr/bin x", "UUUUUUUU..");
        check("( ls )", "R.CC.R");
        check("(( x ))", "RR...RR");
        check("a=(1 2)", "..R...R");
    }

    #[test]
    fn precommand_options() {
        check("command -v ls", "PPPPPPP......");
        check("sudo -e x", "PPPP.....");
        check("nice -n 5 ls", "PPPP......CC");
        check("sudo -- ls", "PPPP....CC");
    }

    #[test]
    fn dollar_quote_escapes() {
        check("echo $'a\\nb'", "CCCC.qqqvvqq");
    }

    #[test]
    fn finishing_drops_path_prefix() {
        let st = highlight(b"cat /etc/ho", &Fake, true);
        assert!(st[4..].iter().all(|s| *s == Style::Plain));
    }

    #[test]
    fn assignment_detection() {
        assert!(is_assignment("FOO=1"));
        assert!(is_assignment("a_b+=x"));
        assert!(is_assignment("arr[2]=x"));
        assert!(!is_assignment("1x=2"));
        assert!(!is_assignment("--opt=1"));
    }
}
