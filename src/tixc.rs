// Adapted from the MIT-licensed TnsTools TIXC0100 decoder.
use anyhow::{Context, Result, bail, ensure};

const ETA: &[u8] = b" etaionsrhAlcduFmfpgybwvkxqjz,.'";
const DIGITS: &[u8] = b"0123456789AN,.EFx; (){}[]^+-/*PB";

#[derive(Default)]
enum Text {
    #[default]
    Plain,
    Nibbles {
        digits: bool,
        extended: bool,
    },
    Unicode {
        count: u8,
    },
}

impl Text {
    fn plain(&self) -> bool {
        matches!(self, Self::Plain)
    }

    fn decode(&mut self, byte: u8, rest: &mut &[u8], out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Plain => match byte {
                0xa0..=0xaf | 0xf0..=0xff => {
                    let digits = byte >= 0xf0;
                    *self = Self::Nibbles {
                        digits,
                        extended: false,
                    };
                    self.nibble(byte & 15, out)?;
                }
                0x80..=0x8f => {
                    *self = Self::Unicode {
                        count: (byte & 15) + 1,
                    }
                }
                0x00..=0x07 => append_char(out, (u32::from(byte) << 8) | u32::from(take(rest)?))?,
                0x08 => {
                    let hi = take(rest)?;
                    ensure!(hi < 0x20, "invalid four-byte TIXC character");
                    let mid = take(rest)?;
                    let lo = take(rest)?;
                    append_char(
                        out,
                        (u32::from(hi) << 16) | (u32::from(mid) << 8) | u32::from(lo),
                    )?;
                }
                0x09 | 0x0a | 0x0d | 0x20..=0x7e => out.push(byte),
                _ => bail!("unsupported TIXC text byte {byte:#04x}"),
            },
            Self::Nibbles { .. } => {
                self.nibble(byte >> 4, out)?;
                if !self.plain() {
                    self.nibble(byte & 15, out)?;
                }
            }
            Self::Unicode { count } => {
                append_char(out, (u32::from(byte) << 8) | u32::from(take(rest)?))?;
                *count -= 1;
                if *count == 0 {
                    *self = Self::Plain;
                }
            }
        }
        Ok(())
    }

    fn nibble(&mut self, value: u8, out: &mut Vec<u8>) -> Result<()> {
        let Self::Nibbles { digits, extended } = self else {
            unreachable!()
        };
        let table = if *digits { DIGITS } else { ETA };
        let index = usize::from(value) + if *extended { 16 } else { 0 };
        let ch = *table.get(index).context("invalid TIXC packed character")?;
        if *extended {
            *extended = false;
        } else if ch == if *digits { b'F' } else { b'A' } {
            *self = Self::Plain;
            return Ok(());
        } else if ch == if *digits { b'A' } else { b'F' } {
            *extended = true;
            return Ok(());
        }
        match (*digits, ch) {
            (true, b'N') => out.extend("−".as_bytes()),
            (true, b'E') => out.extend("\u{f000}".as_bytes()),
            (true, b'P') => out.extend(b"))"),
            (true, b'B') => out.extend(b"]["),
            _ => out.push(ch),
        }
        Ok(())
    }
}

fn append_char(out: &mut Vec<u8>, code: u32) -> Result<()> {
    let ch = char::from_u32(code).context("invalid TIXC Unicode scalar")?;
    out.extend(ch.encode_utf8(&mut [0; 4]).as_bytes());
    Ok(())
}

fn take(data: &mut &[u8]) -> Result<u8> {
    let (byte, tail) = data.split_first().context("truncated TIXC stream")?;
    *data = tail;
    Ok(*byte)
}

#[derive(PartialEq)]
enum State {
    Content,
    Tag,
    TagRef,
    CloseRef,
    AfterTagRef,
    AfterTagRefSlash,
    ClosingTag,
    SelfClosing,
    Attribute,
    AttrRef,
    Quote,
    Value,
    AfterValue,
}

pub fn expand(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    use State::*;
    let header = b"TIXC0100-";
    let data = data.strip_prefix(header).context("invalid TIXC header")?;
    let version_len = data
        .windows(2)
        .position(|pair| pair == b"?>")
        .context("missing TIXC version")?;
    let version = &data[..version_len];
    ensure!(
        matches!(version, b"1.0" | b"1.1"),
        "unsupported XML version"
    );
    let mut data = &data[version_len + 2..];
    let mut out = b"<?xml version=\"".to_vec();
    out.extend(version);
    out.extend(b"\" encoding=\"UTF-8\" ?>");

    let mut tags: Vec<Vec<u8>> = Vec::new();
    let mut attrs: Vec<Vec<u8>> = Vec::new();
    let mut token = Vec::new();
    let mut state = Content;
    let mut text = Text::Plain;
    while let Some((&b, rest)) = data.split_first() {
        let before = data;
        data = rest;
        match state {
            Content if text.plain() && b == b'<' => {
                if data.starts_with(b"![CDATA[") {
                    let end = data
                        .windows(3)
                        .position(|s| s == b"]]>")
                        .context("unclosed TIXC CDATA")?
                        + 3;
                    out.push(b'<');
                    out.extend(&data[..end]);
                    data = &data[end..];
                } else {
                    out.push(b'<');
                    state = Tag;
                }
            }
            Content if text.plain() && b == 0x0c => {
                out.push(b'<');
                state = TagRef;
            }
            Content if text.plain() && b == 0x0e => {
                out.extend(b"</");
                state = CloseRef;
            }
            Content => text.decode(b, &mut data, &mut out)?,
            Tag if matches!(b, 0x0e | 0x0f | b' ' | b'/' | b'>') => {
                let has_name = !token.is_empty();
                save_token(&mut token, &mut tags);
                match b {
                    0x0e => {
                        out.extend(b"></");
                        state = CloseRef;
                    }
                    0x0f => {
                        out.push(b' ');
                        state = AttrRef;
                    }
                    b' ' if has_name => {
                        out.push(b' ');
                        state = Attribute;
                    }
                    b'/' => {
                        out.push(b'/');
                        state = if has_name { ClosingTag } else { SelfClosing };
                    }
                    b'>' if has_name => {
                        out.push(b'>');
                        state = Content;
                    }
                    _ => bail!("empty TIXC tag"),
                }
            }
            Tag => {
                ensure!(token.len() < 64, "TIXC tag name exceeds 64 bytes");
                token.push(b);
                out.push(b);
            }
            TagRef | CloseRef => {
                out.extend(
                    tags.get(usize::from(b))
                        .context("unknown TIXC tag reference")?,
                );
                if state == CloseRef {
                    out.push(b'>');
                    state = Content;
                } else {
                    state = AfterTagRef;
                }
            }
            AfterTagRef => match b {
                b' ' => {
                    out.push(b);
                    state = Attribute;
                }
                0x0f => {
                    out.push(b' ');
                    state = AttrRef;
                }
                b'/' => state = AfterTagRefSlash,
                _ => {
                    out.push(b'>');
                    data = before;
                    state = Content;
                }
            },
            AfterTagRefSlash if b == b'>' => {
                out.extend(b"/>");
                state = Content;
            }
            AfterTagRefSlash => {
                out.push(b'>');
                data = before;
                state = Content;
            }
            ClosingTag => {
                out.push(b);
                if b == b'>' {
                    state = Content;
                }
            }
            SelfClosing => {
                ensure!(b == b'>', "expected closing bracket in TIXC");
                out.push(b);
                state = Content;
            }
            Attribute => {
                out.push(b);
                if b == b'=' {
                    save_token(&mut token, &mut attrs);
                    state = Quote;
                } else {
                    token.push(b);
                }
            }
            AttrRef => {
                out.extend(
                    attrs
                        .get(usize::from(b))
                        .context("unknown TIXC attribute reference")?,
                );
                out.extend(b"=\"");
                state = Value;
            }
            Quote if b == b' ' => {}
            Quote if b == b'"' => {
                out.push(b);
                state = Value;
            }
            Quote => bail!("expected quoted TIXC attribute"),
            Value if b == b'"' && text.plain() => {
                out.push(b);
                state = AfterValue;
            }
            Value => text.decode(b, &mut data, &mut out)?,
            AfterValue => {
                match b {
                    b'>' => state = Content,
                    b'/' => state = SelfClosing,
                    b' ' => state = Attribute,
                    0x0f => state = AttrRef,
                    _ => bail!("invalid TIXC attribute separator"),
                }
                out.push(if b == 0x0f { b' ' } else { b });
            }
        }
        ensure!(out.len() <= expected, "TIXC output exceeds expected size");
    }
    ensure!(state == Content && text.plain(), "incomplete TIXC document");
    Ok(out)
}

fn save_token(token: &mut Vec<u8>, dictionary: &mut Vec<Vec<u8>>) {
    if !token.is_empty() && dictionary.len() < 256 {
        dictionary.push(std::mem::take(token));
    }
    token.clear();
}
