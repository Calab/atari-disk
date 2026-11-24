//! ATASCII, the Atari's character set, and plain ASCII.
//!
//! File names and paths are given to this crate as ATASCII bytes, exactly
//! as they're stored on the disk, so every file can be named, even one
//! whose name holds inverse or graphics characters. ATASCII and ASCII
//! agree on the space, digits, letters (both cases) and most punctuation
//! (`$20`–`$5F`, `$61`–`$7A` and `|`), so an ordinary name such as
//! `"GAMES>PACMAN.COM"` can be passed as a Rust string as it is. These
//! two functions convert the rest.
//!
//! | ATASCII | Is | ASCII |
//! |---|---|---|
//! | `$00`–`$1F` | graphics characters | none |
//! | `$60` | ♦ | none (ASCII `` ` `` is not ATASCII) |
//! | `$7B` | ♠ | none (ASCII `{` is not ATASCII) |
//! | `$7D`–`$7F` | clear screen, backspace, tab | none (`}` and `~` aren't ATASCII) |
//! | `$9B` | end of line | `\n` |
//! | `$80`–`$FF` | inverse video | none |

use crate::error::{Error, Result};

/// The ATASCII end-of-line character.
pub const EOL: u8 = 0x9B;

/// ASCII text as ATASCII bytes: each character is kept as it is, and
/// `\n` becomes the ATASCII end of line (`$9B`). Fails with
/// [`Error::BadName`] on a character ATASCII doesn't have: anything that
/// isn't printable ASCII, and `` ` ``, `{`, `}` and `~`.
pub fn ascii_to_atascii(text: &str) -> Result<Vec<u8>> {
    text.chars()
        .map(|c| match c {
            '\n' => Ok(EOL),
            ' '..='_' | 'a'..='z' | '|' => Ok(c as u8),
            _ => Err(Error::BadName(format!(
                "{text:?}: {c:?} has no ATASCII character"
            ))),
        })
        .collect()
}

/// ATASCII bytes as ASCII text, for showing: the end of line (`$9B`)
/// becomes `\n`, and every character ASCII doesn't have becomes `?`.
pub fn atascii_to_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            EOL => '\n',
            b' '..=b'_' | b'a'..=b'z' | b'|' => b as char,
            _ => '?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_both_ways() {
        assert_eq!(ascii_to_atascii("HELLO.TXT").unwrap(), b"HELLO.TXT");
        assert_eq!(ascii_to_atascii("Line 1\n").unwrap(), b"Line 1\x9b");
        for bad in ["a`b", "{", "}", "~", "\t", "é"] {
            assert!(
                matches!(ascii_to_atascii(bad), Err(Error::BadName(_))),
                "{bad:?}"
            );
        }
        assert_eq!(atascii_to_ascii(b"OK|ok\x9b"), "OK|ok\n");
        assert_eq!(atascii_to_ascii(&[0x01, 0x60, 0x7B, 0x7E, 0xC1]), "?????");
        // Everything printable goes there and back unchanged.
        let all: String = (' '..='z').filter(|&c| c != '`').collect();
        assert_eq!(atascii_to_ascii(&ascii_to_atascii(&all).unwrap()), all);
    }
}
