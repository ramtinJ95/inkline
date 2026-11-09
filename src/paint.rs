//! Terminal output on top of readline's own redisplay.
//!
//! readline has already drawn the line (including any suggestion text, which
//! is temporarily part of the buffer while it draws) and left the cursor at
//! its tracked position. We repaint the same cells with colour, using relative
//! cursor motion only, then return to exactly where readline left it, so
//! readline's model of the screen stays true.

use crate::layout::Cell;
use std::io::Write;

struct Cursor {
    row: i64,
    /// `None` after writing text: the terminal may be in the pending-wrap state.
    col: Option<i64>,
}

fn move_to(out: &mut Vec<u8>, cur: &mut Cursor, row: i64, col: i64) {
    if row < cur.row {
        let _ = write!(out, "\x1b[{}A", cur.row - row);
    } else if row > cur.row {
        let _ = write!(out, "\x1b[{}B", row - cur.row);
    }
    if cur.col != Some(col) || row != cur.row {
        out.push(b'\r');
        if col > 0 {
            let _ = write!(out, "\x1b[{col}C");
        }
    }
    *cur = Cursor { row, col: Some(col) };
}

/// Repaint `cells` (rows relative to the layout, shifted by `row_offset` into
/// readline's coordinates) with SGR parameters `sgr[i]` for `cells[i]`, then
/// put the cursor back at readline's `cursor`.
pub fn repaint(out: &mut Vec<u8>, cells: &[Cell], sgr: &[&str], row_offset: i64, cursor: (i64, i64), cols: i64) {
    let mut cur = Cursor { row: cursor.0, col: Some(cursor.1) };
    let mut k = 0;
    while k < cells.len() {
        let first = &cells[k];
        let style = sgr[k];
        let row = first.row + row_offset;
        let mut e = k + 1;
        let mut next_col = first.col + first.width;
        while e < cells.len() && cells[e].row == first.row && cells[e].col == next_col && sgr[e] == style {
            next_col += cells[e].width;
            e += 1;
        }
        move_to(out, &mut cur, row, first.col);
        if style.is_empty() {
            out.extend_from_slice(b"\x1b[0m");
        } else {
            let _ = write!(out, "\x1b[0;{style}m");
        }
        for c in &cells[k..e] {
            out.extend_from_slice(&c.text);
        }
        cur.col = (next_col < cols).then_some(next_col);
        k = e;
    }
    out.extend_from_slice(b"\x1b[0m");
    move_to(out, &mut cur, cursor.0, cursor.1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Opts, layout};

    fn w(_: char) -> i32 {
        1
    }

    #[test]
    fn single_row_runs_and_no_trailing_move() {
        let l = layout(b"ls x", 4, 2, &Opts { cols: 80, multibyte: true, output_meta: true }, &w).unwrap();
        let mut out = Vec::new();
        let sgr: Vec<&str> = l.cells.iter().map(|c| if c.start < 2 { "32" } else { "" }).collect();
        repaint(&mut out, &l.cells, &sgr, 0, (0, 6), 80);
        assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[2C\x1b[0;32mls\x1b[0m x\x1b[0m");
    }

    #[test]
    fn wrapped_rows_move_between_rows_and_back() {
        let l = layout(b"abcdefghijkl", 12, 2, &Opts { cols: 10, multibyte: true, output_meta: true }, &w).unwrap();
        let mut out = Vec::new();
        repaint(&mut out, &l.cells, &vec![""; l.cells.len()], 0, (1, 4), 10);
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b[1A\r\x1b[2C\x1b[0mabcdefgh\x1b[1B\r\x1b[0mijkl\x1b[0m");
    }

    #[test]
    fn row_offset_and_cursor_in_middle() {
        let l = layout(b"abc", 1, 2, &Opts { cols: 80, multibyte: true, output_meta: true }, &w).unwrap();
        let mut out = Vec::new();
        repaint(&mut out, &l.cells, &vec!["1;31"; l.cells.len()], 3, (3, 3), 80);
        assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[2C\x1b[0;1;31mabc\x1b[0m\r\x1b[3C");
    }
}
