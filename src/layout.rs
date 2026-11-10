//! readline 8.3's line layout (`rl_redisplay` in display.c), reproduced so the
//! screen cell of every buffer character is known: wrapping at the screen
//! width, wide characters that do not fit moving to the next row, tabs to the
//! next multiple of 8, `^X` for control characters, `\ooo` / raw bytes for
//! invalid bytes, newlines starting a new row.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Byte range in the buffer this cell displays.
    pub start: usize,
    pub end: usize,
    pub row: i64,
    pub col: i64,
    /// Bytes readline writes for it (the character itself, `^X`, spaces for a tab).
    pub text: Vec<u8>,
    pub width: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub cells: Vec<Cell>,
    pub point: (i64, i64),
    /// Screen position of byte index `mark` (see `layout_with_mark`).
    pub mark: (i64, i64),
    pub end: (i64, i64),
}

pub struct Opts {
    pub cols: i64,
    /// UTF-8 locale and not byte-oriented.
    pub multibyte: bool,
    /// readline's `output-meta`.
    pub output_meta: bool,
}

struct Pos {
    row: i64,
    lpos: i64,
    cols: i64,
}

impl Pos {
    /// CHECK_LPOS
    fn advance(&mut self) {
        self.lpos += 1;
        if self.lpos >= self.cols {
            self.row += 1;
            self.lpos = 0;
        }
    }
}

/// Decode one character at `buf[i..]` the way readline's MBRTOWC loop does:
/// (char if valid, byte length). Invalid or truncated sequences are one byte.
fn decode(buf: &[u8], i: usize, multibyte: bool) -> (Option<char>, usize) {
    let b = buf[i];
    if !multibyte || b < 0x80 {
        return (Some(b as char).filter(|_| b < 0x80), 1);
    }
    let len = match b {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    match buf.get(i..i + len).and_then(|s| std::str::from_utf8(s).ok()) {
        Some(s) => (s.chars().next(), len),
        None => (None, 1),
    }
}

/// Visible width of a prompt line: `\001...\002` regions are invisible.
pub fn prompt_width(prompt: &[u8], multibyte: bool, width: &impl Fn(char) -> i32) -> i64 {
    let last = prompt.iter().rposition(|&b| b == b'\n').map_or(prompt, |p| &prompt[p + 1..]);
    let mut w = 0;
    let mut invisible = false;
    let mut i = 0;
    while i < last.len() {
        match last[i] {
            1 => invisible = true,
            2 => invisible = false,
            _ if invisible => {}
            _ => {
                let (ch, n) = decode(last, i, multibyte);
                w += match ch {
                    Some(c) if !c.is_ascii() => width(c).max(0) as i64,
                    _ => 1,
                };
                i += n;
                continue;
            }
        }
        i += 1;
    }
    w
}

pub fn layout(buf: &[u8], point: usize, start_col: i64, o: &Opts, width: &impl Fn(char) -> i32) -> Option<Layout> {
    layout_with_mark(buf, point, point, start_col, o, width)
}

/// Like `layout`, also reporting where byte index `mark` lands (where readline
/// would put the cursor if it were there). `None` when the line cannot be
/// displayed consistently (a tab or wide character wider than the screen);
/// callers then leave readline's output alone.
pub fn layout_with_mark(buf: &[u8], point: usize, mark: usize, start_col: i64, o: &Opts, width: &impl Fn(char) -> i32) -> Option<Layout> {
    if o.cols <= 0 || start_col < 0 {
        return None;
    }
    let mut p = Pos { row: start_col / o.cols, lpos: start_col % o.cols, cols: o.cols };
    let mut cells: Vec<Cell> = Vec::with_capacity(buf.len());
    let mut point_pos = None;
    let mut mark_pos = None;
    let mut i = 0;
    while i < buf.len() {
        let (ch, n) = decode(buf, i, o.multibyte);
        let c = buf[i];
        if c == 0 {
            break;
        }
        let w = match ch {
            Some(ch) if o.multibyte && !ch.is_ascii() => {
                let w = width(ch);
                if w >= 0 { w as i64 } else { 1 }
            }
            _ => 1,
        };
        if i == point {
            point_pos = Some((p.row, p.lpos));
        }
        if i == mark {
            mark_pos = Some((p.row, p.lpos));
        }
        let (start, end) = (i, i + n);
        let push = |cells: &mut Vec<Cell>, p: &Pos, text: Vec<u8>, width: i64| {
            cells.push(Cell { start, end, row: p.row, col: p.lpos, text, width });
        };
        if c >= 0x80 && n == 1 && w == 1 {
            // META_CHAR: invalid byte in a UTF-8 locale, or any high byte otherwise.
            if o.output_meta {
                push(&mut cells, &p, vec![c], 1);
                p.advance();
            } else {
                for d in format!("\\{c:o}").bytes() {
                    push(&mut cells, &p, vec![d], 1);
                    p.advance();
                }
            }
        } else if c == b'\t' {
            let temp = 8 - p.lpos % 8;
            if p.lpos + temp >= o.cols {
                let first = o.cols - p.lpos;
                if temp - first >= o.cols {
                    // Screen narrower than the tab: readline's own row bookkeeping breaks.
                    return None;
                }
                push(&mut cells, &p, vec![b' '; first as usize], first);
                p.row += 1;
                p.lpos = temp - first;
                if p.lpos > 0 {
                    cells.push(Cell { start, end, row: p.row, col: 0, text: vec![b' '; p.lpos as usize], width: p.lpos });
                }
            } else {
                push(&mut cells, &p, vec![b' '; temp as usize], temp);
                p.lpos += temp;
            }
        } else if c == b'\n' {
            p.row += 1;
            p.lpos = 0;
        } else if c < 0x20 || c == 0x7f {
            let shown = if c == 0x7f { b'?' } else { (c | 0x40).to_ascii_uppercase() };
            for d in [b'^', shown] {
                push(&mut cells, &p, vec![d], 1);
                p.advance();
            }
        } else if o.multibyte && !c.is_ascii() {
            if w > o.cols {
                return None; // cannot be displayed on this screen at all
            }
            if o.cols < p.lpos + w {
                while p.lpos != 0 {
                    p.advance();
                }
                if i == point {
                    point_pos = Some((p.row, p.lpos));
                }
                if i == mark {
                    mark_pos = Some((p.row, p.lpos));
                }
            }
            if w == 0 {
                // Combining character: drawn together with the previous one.
                match cells.last_mut() {
                    Some(prev) => {
                        prev.text.extend_from_slice(&buf[start..end]);
                        prev.end = end;
                    }
                    None => push(&mut cells, &p, buf[start..end].to_vec(), 0),
                }
            } else {
                push(&mut cells, &p, buf[start..end].to_vec(), w);
                for _ in 0..w {
                    p.advance();
                }
            }
        } else {
            push(&mut cells, &p, vec![c], 1);
            p.advance();
        }
        i = end;
    }
    let end = (p.row, p.lpos);
    Some(Layout { cells, point: point_pos.unwrap_or(end), mark: mark_pos.unwrap_or(end), end })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(c: char) -> i32 {
        match c {
            '日' | '本' | '🦀' => 2,
            '\u{301}' => 0,
            _ => 1,
        }
    }

    fn opts(cols: i64) -> Opts {
        Opts { cols, multibyte: true, output_meta: true }
    }

    fn pos(l: &Layout) -> Vec<(i64, i64, String)> {
        l.cells.iter().map(|c| (c.row, c.col, String::from_utf8_lossy(&c.text).into_owned())).collect()
    }

    #[test]
    fn ascii_wraps_at_width() {
        let l = layout(b"abcdef", 6, 2, &opts(4), &w).unwrap();
        assert_eq!(pos(&l), vec![(0, 2, "a".into()), (0, 3, "b".into()), (1, 0, "c".into()), (1, 1, "d".into()), (1, 2, "e".into()), (1, 3, "f".into())]);
        assert_eq!(l.end, (2, 0), "filling a row moves the cursor to the next one");
        assert_eq!(l.point, (2, 0));
    }

    #[test]
    fn wide_char_that_does_not_fit_moves_down() {
        let buf = "ab日".as_bytes();
        let l = layout(buf, 2, 1, &opts(4), &w).unwrap();
        // 'a' col1, 'b' col2, '日' needs 2 cols but only col3 is left.
        assert_eq!(pos(&l), vec![(0, 1, "a".into()), (0, 2, "b".into()), (1, 0, "日".into())]);
        assert_eq!(l.point, (1, 0), "point on the wide char lands after the padding");
        assert_eq!(l.end, (1, 2));
    }

    #[test]
    fn tabs_expand_and_split_across_rows() {
        let l = layout(b"a\tb", 3, 0, &opts(20), &w).unwrap();
        assert_eq!(pos(&l), vec![(0, 0, "a".into()), (0, 1, "       ".into()), (0, 8, "b".into())]);
        let l = layout(b"abcdef\tx", 8, 0, &opts(10), &w).unwrap();
        // tab from col 6 wants 2 spaces; 6+2 < 10 fits
        assert_eq!(l.cells[6].col, 6);
        let l = layout(b"abcdefgh\tx", 10, 0, &opts(10), &w).unwrap();
        // tab at col 8 wants 8 more columns: 2 on this row, 6 on the next
        assert_eq!((l.cells[8].row, l.cells[8].col, l.cells[8].width), (0, 8, 2));
        assert_eq!((l.cells[9].row, l.cells[9].col, l.cells[9].width), (1, 0, 6));
        assert_eq!((l.cells[10].row, l.cells[10].col), (1, 6));
    }

    #[test]
    fn control_chars_newlines_and_invalid_bytes() {
        let l = layout(b"a\x01b\nc\x7f", 0, 0, &opts(20), &w).unwrap();
        assert_eq!(
            pos(&l),
            vec![(0, 0, "a".into()), (0, 1, "^".into()), (0, 2, "A".into()), (0, 3, "b".into()), (1, 0, "c".into()), (1, 1, "^".into()), (1, 2, "?".into())]
        );
        let l = layout(b"\xffz", 0, 0, &Opts { cols: 20, multibyte: true, output_meta: false }, &w).unwrap();
        assert_eq!(pos(&l).iter().map(|c| c.2.as_str()).collect::<String>(), "\\377z");
    }

    #[test]
    fn combining_marks_join_previous_cell() {
        let buf = "e\u{301}x".as_bytes();
        let l = layout(buf, buf.len(), 0, &opts(20), &w).unwrap();
        assert_eq!(l.cells.len(), 2);
        assert_eq!(l.cells[0].text, "e\u{301}".as_bytes());
        assert_eq!(l.end, (0, 2));
    }

    #[test]
    fn prompt_width_skips_invisible_and_earlier_lines() {
        assert_eq!(prompt_width(b"\x01\x1b[34m\x02dir\x01\x1b[0m\x02\n\x01\x1b[35m\x02\xe2\x9d\xaf\x01\x1b[0m\x02 ", true, &w), 2);
        assert_eq!(prompt_width("日 ".as_bytes(), true, &w), 3);
    }
}
