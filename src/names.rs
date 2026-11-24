//! File names: the 8 + 3 bytes a directory entry stores, the `NAME.EXT`
//! form paths use, and each file system's rules for new names. All of it
//! is ATASCII bytes (see [`crate::atascii`]).

use crate::atascii::atascii_to_ascii;
use crate::error::{Error, Result};

/// A name and extension as stored: 8 + 3 bytes, padded with spaces.
pub(crate) type Name = [u8; 11];

/// The rules a file system has for new names.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rules {
    /// `_` is allowed (SpartaDOS).
    pub underscore: bool,
    /// The name may start with a digit (SpartaDOS).
    pub digit_first: bool,
}

pub(crate) const DOS2: Rules = Rules {
    underscore: false,
    digit_first: false,
};

pub(crate) const SPARTA: Rules = Rules {
    underscore: true,
    digit_first: true,
};

/// `NAME.EXT` as ATASCII bytes from the stored 8 + 3 (spaces at the end
/// of each part dropped, and no dot when there is no extension). This is
/// what a path gives to find the file again.
pub(crate) fn joined(name: &[u8], ext: &[u8]) -> Vec<u8> {
    let trim = |b: &[u8]| -> Vec<u8> {
        let end = b.iter().rposition(|&c| c != b' ').map_or(0, |i| i + 1);
        b[..end].to_vec()
    };
    let mut out = trim(name);
    let ext = trim(ext);
    if !ext.is_empty() {
        out.push(b'.');
        out.extend(ext);
    }
    out
}

/// `NAME.EXT` as text to show, with characters ASCII hasn't got as `?`.
pub(crate) fn display(name: &[u8], ext: &[u8]) -> String {
    atascii_to_ascii(&joined(name, ext))
}

/// The 11 bytes `name` (e.g. `b"SAMPLE.LST"`) is stored as, without
/// checking any rules, to find files already on a disk whatever their
/// names. `None` if it can't be the name of any file.
pub(crate) fn stored(name: &[u8]) -> Option<Name> {
    let (base, ext) = split_dot(name);
    if base.len() > 8 || ext.len() > 3 {
        return None;
    }
    let mut out = [b' '; 11];
    out[..base.len()].copy_from_slice(base);
    out[8..8 + ext.len()].copy_from_slice(ext);
    Some(out)
}

/// A path's directory and its last name: `"A>B>C.TXT"` gives `"A>B"` and
/// `"C.TXT"`.
pub(crate) fn split_path(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&c| c == b'>') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (&[], path),
    }
}

fn split_dot(name: &[u8]) -> (&[u8], &[u8]) {
    match name.iter().position(|&c| c == b'.') {
        Some(i) => (&name[..i], &name[i + 1..]),
        None => (name, &[]),
    }
}

/// Check a new name against `rules` and give the 11 bytes it's stored
/// as: 1 to 8 letters and digits (and `_` if allowed), starting with a
/// letter (or a digit if allowed), then optionally a dot and up to 3
/// more. With `uppercase`, lower-case letters are turned into capitals
/// first; without it they're refused.
pub(crate) fn parse(name: &[u8], uppercase: bool, rules: Rules) -> Result<Name> {
    let bad = |why: &str| Err(Error::BadName(format!("{}: {why}", atascii_to_ascii(name))));
    let text = if uppercase {
        name.to_ascii_uppercase()
    } else {
        name.to_vec()
    };
    let (base, ext) = split_dot(&text);
    if base.is_empty() || base.len() > 8 {
        return bad("the name must be 1 to 8 characters");
    }
    if ext.len() > 3 {
        return bad("the extension can be at most 3 characters");
    }
    let ok =
        |c: &u8| c.is_ascii_uppercase() || c.is_ascii_digit() || (rules.underscore && *c == b'_');
    if !base.iter().chain(ext).all(ok) {
        return if text.iter().any(|c| c.is_ascii_lowercase()) {
            bad("lower-case letters aren't allowed")
        } else if rules.underscore {
            bad("only letters A-Z, digits 0-9 and _ are allowed")
        } else {
            bad("only letters A-Z and digits 0-9 are allowed")
        };
    }
    if !rules.digit_first && !base[0].is_ascii_uppercase() {
        return bad("the name must start with a letter");
    }
    stored(&text).ok_or_else(|| Error::BadName(atascii_to_ascii(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(&stored(b"AB.C").unwrap(), b"AB      C  ");
        assert_eq!(joined(b"AB      ", b"C  "), b"AB.C");
        assert_eq!(joined(b"A B     ", b"   "), b"A B");
        assert_eq!(
            stored(&joined(b"A\xc1      ", b"   ")).unwrap()[..2],
            *b"A\xc1"
        );
        assert!(stored(b"TOOLONGNAME").is_none());
        assert_eq!(display(b"A\xc1      ", b"X  "), "A?.X");
        assert!(parse(b"1ST", false, DOS2).is_err());
        assert!(parse(b"1ST_ONE", false, SPARTA).is_ok());
        assert!(parse(b"A_B", false, DOS2).is_err());
        assert_eq!(&parse(b"ab.c", true, DOS2).unwrap(), b"AB      C  ");
        assert!(parse(b"ab", false, DOS2).is_err());
        assert!(parse(b"A\xc1", false, SPARTA).is_err());
    }
}
