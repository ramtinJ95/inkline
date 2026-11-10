//! zsh-autosuggestions' default `history` strategy: `${history[(r)$prefix*]}`,
//! the most recent history entry starting with the current buffer.

/// Suffix of the most recent entry starting with `buf`. If that entry equals
/// `buf` there is no suggestion (older, longer entries are not considered),
/// exactly as in zsh-autosuggestions. Multi-line entries are included.
pub fn from_history<'a>(buf: &[u8], newest_first: impl Iterator<Item = &'a [u8]>) -> Option<&'a [u8]> {
    lookup(buf, newest_first).0
}

/// (suggestion, whether any entry starts with `buf`).
fn lookup<'a>(buf: &[u8], mut newest_first: impl Iterator<Item = &'a [u8]>) -> (Option<&'a [u8]>, bool) {
    if buf.is_empty() {
        return (None, true);
    }
    match newest_first.find(|line| line.starts_with(buf)) {
        Some(line) => (Some(&line[buf.len()..]).filter(|s| !s.is_empty()), true),
        None => (None, false),
    }
}

/// Remembers the last answer so typing along a suggestion costs O(1), like
/// zsh-autosuggestions' "typing in the suggestion" shortcut: if the newest
/// entry with prefix `old` continues with the typed bytes, it is also the
/// newest entry with the longer prefix.
#[derive(Default)]
pub struct Cache {
    buf: Vec<u8>,
    suffix: Option<Vec<u8>>,
    /// Some history entry starts with `buf` (false: no longer buffer can match either).
    any_match: bool,
    history_key: (i32, i32, usize),
}

impl Cache {
    pub fn get<'a>(
        &mut self,
        buf: &[u8],
        history_key: (i32, i32, usize),
        newest_first: impl FnOnce() -> Box<dyn Iterator<Item = &'a [u8]> + 'a>,
    ) -> Option<&[u8]> {
        let same_history = history_key == self.history_key;
        let reusable = same_history
            && buf.len() >= self.buf.len()
            && buf.starts_with(&self.buf)
            && match &self.suffix {
                Some(sfx) => sfx.starts_with(&buf[self.buf.len()..]),
                None => false,
            };
        // Nothing starts with the old buffer, so nothing starts with a longer one.
        let still_none = same_history && !self.any_match && !self.buf.is_empty() && buf.starts_with(&self.buf);
        if still_none {
            self.buf = buf.to_vec();
        } else if reusable {
            let typed = buf.len() - self.buf.len();
            let sfx = self.suffix.take().map(|s| s[typed..].to_vec()).filter(|s| !s.is_empty());
            self.buf = buf.to_vec();
            self.suffix = sfx;
        } else if !(same_history && buf == self.buf.as_slice()) {
            let (sfx, any) = lookup(buf, newest_first());
            self.suffix = sfx.map(<[u8]>::to_vec);
            self.any_match = any;
            self.buf = buf.to_vec();
            self.history_key = history_key;
        }
        self.suffix.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist() -> Vec<&'static [u8]> {
        // newest first
        vec![b"git status", b"git push origin main", b"ls -la", b"git commit -m 'x'\nmore", b"ls -la --color", b"git"]
    }

    #[test]
    fn newest_matching_entry_wins() {
        assert_eq!(from_history(b"git", hist().into_iter()), Some(&b" status"[..]));
        assert_eq!(from_history(b"git p", hist().into_iter()), Some(&b"ush origin main"[..]));
        assert_eq!(from_history(b"", hist().into_iter()), None);
        assert_eq!(from_history(b"git c", hist().into_iter()), Some(&b"ommit -m 'x'\nmore"[..]), "multi-line entries count");
    }

    #[test]
    fn newest_match_equal_to_buffer_means_no_suggestion() {
        // "ls -la --color" is older than "ls -la": zsh-autosuggestions shows nothing.
        assert_eq!(from_history(b"ls -la", hist().into_iter()), None);
    }

    #[test]
    fn cache_matches_fresh_lookup() {
        let mut c = Cache::default();
        let key = (1, 5, 0);
        for buf in [&b"g"[..], b"gi", b"git", b"git ", b"git p", b"git pu", b"git", b"x", b"git s", b"git status", b"l", b"ls -la", b"ls -la "] {
            let fresh = from_history(buf, hist().into_iter()).map(<[u8]>::to_vec);
            let cached = c.get(buf, key, || Box::new(hist().into_iter())).map(<[u8]>::to_vec);
            assert_eq!(cached, fresh, "buf {:?}", String::from_utf8_lossy(buf));
        }
    }

    #[test]
    fn cache_invalidates_on_history_change() {
        let mut c = Cache::default();
        assert_eq!(c.get(b"ls", (1, 1, 0), || Box::new(vec![&b"ls -la"[..]].into_iter())), Some(&b" -la"[..]));
        assert_eq!(c.get(b"ls", (1, 2, 0), || Box::new(vec![&b"ls ~"[..]].into_iter())), Some(&b" ~"[..]));
    }
}
