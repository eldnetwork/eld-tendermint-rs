//! Amino JSON for the two Ed25519 key names.
//!
//! Go's `libs/json` writes a registered `[]byte` as
//! `{"type":"<name>","value":"<stdlib base64>"}` with no spaces.
//! `value` uses `encoding/base64.StdEncoding` (padding kept).

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::Error;
use crate::ed25519::{PRIV_KEY_NAME, PUB_KEY_NAME, PrivKey, PubKey};

/// Compact Amino JSON for `ed25519.PubKey`.
#[must_use]
pub fn marshal_pub_key(key: &PubKey) -> String {
    marshal(PUB_KEY_NAME, key.as_bytes())
}

/// Compact Amino JSON for `ed25519.PrivKey`.
#[must_use]
pub fn marshal_priv_key(key: &PrivKey) -> String {
    marshal(PRIV_KEY_NAME, key.as_bytes())
}

/// Reads Amino JSON produced for `tendermint/PubKeyEd25519`.
///
/// # Errors
///
/// Returns an error when the envelope is not that type, `value` is not standard
/// base64, or the decoded length is not 32.
pub fn unmarshal_pub_key(json: &str) -> Result<PubKey, Error> {
    let bytes = unmarshal(PUB_KEY_NAME, json)?;
    PubKey::from_bytes(&bytes)
}

/// Reads Amino JSON produced for `tendermint/PrivKeyEd25519`.
///
/// # Errors
///
/// Returns an error when the envelope is not that type, `value` is not standard
/// base64, or the decoded length is not 64.
pub fn unmarshal_priv_key(json: &str) -> Result<PrivKey, Error> {
    let bytes = unmarshal(PRIV_KEY_NAME, json)?;
    PrivKey::from_bytes(&bytes)
}

fn marshal(type_name: &str, bytes: &[u8]) -> String {
    let value = STANDARD.encode(bytes);
    format!(r#"{{"type":"{type_name}","value":"{value}"}}"#)
}

fn unmarshal(expected_type: &'static str, json: &str) -> Result<Vec<u8>, Error> {
    let (type_name, value) = parse_envelope(json)?;
    if type_name != expected_type {
        return Err(Error::WrongAminoType {
            expected: expected_type,
            got: type_name,
        });
    }
    STANDARD.decode(value).map_err(|_| Error::InvalidBase64)
}

/// `type` and `value` strings from an Amino interface envelope.
///
/// Accepts either field order and insignificant whitespace, which is what
/// `encoding/json` accepts. Rejects any other key. Base64 itself is not
/// decoded here.
fn parse_envelope(input: &str) -> Result<(String, String), Error> {
    let mut parser = Parser {
        input: input.as_bytes(),
        index: 0,
    };
    parser.skip_ws()?;
    parser.expect(b'{')?;
    parser.skip_ws()?;

    let mut type_name = None;
    let mut value = None;
    if !parser.eat(b'}')? {
        loop {
            let key = parser.parse_string()?;
            parser.skip_ws()?;
            parser.expect(b':')?;
            parser.skip_ws()?;
            let field = parser.parse_string()?;
            match key.as_str() {
                "type" => type_name = Some(field),
                "value" => value = Some(field),
                _ => return Err(Error::InvalidAminoJson),
            }
            parser.skip_ws()?;
            if parser.eat(b'}')? {
                break;
            }
            parser.expect(b',')?;
            parser.skip_ws()?;
            if parser.peek() == Some(b'}') {
                return Err(Error::InvalidAminoJson);
            }
        }
    }

    parser.skip_ws()?;
    if parser.index != parser.input.len() {
        return Err(Error::InvalidAminoJson);
    }

    let type_name = type_name.ok_or(Error::InvalidAminoJson)?;
    if type_name.is_empty() {
        return Err(Error::InvalidAminoJson);
    }
    let value = value.ok_or(Error::InvalidAminoJson)?;
    Ok((type_name, value))
}

struct Parser<'a> {
    input: &'a [u8],
    index: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) -> Result<(), Error> {
        while let Some(byte) = self.peek() {
            if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
                self.bump()?;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.index).copied()
    }

    fn bump(&mut self) -> Result<u8, Error> {
        let byte = self.peek().ok_or(Error::InvalidAminoJson)?;
        self.index += 1;
        Ok(byte)
    }

    fn expect(&mut self, want: u8) -> Result<(), Error> {
        if self.bump()? == want {
            Ok(())
        } else {
            Err(Error::InvalidAminoJson)
        }
    }

    fn eat(&mut self, want: u8) -> Result<bool, Error> {
        if self.peek() == Some(want) {
            self.bump()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn parse_string(&mut self) -> Result<String, Error> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.bump()? {
                b'"' => break,
                b'\\' => {
                    let mut buf = [0u8; 4];
                    let encoded = self.parse_escape()?.encode_utf8(&mut buf);
                    out.extend(encoded.as_bytes());
                }
                byte if byte < 0x20 => return Err(Error::InvalidAminoJson),
                byte => out.push(byte),
            }
        }
        String::from_utf8(out).map_err(|_| Error::InvalidAminoJson)
    }

    fn parse_escape(&mut self) -> Result<char, Error> {
        let ch = match self.bump()? {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{0008}',
            b'f' => '\u{000c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.parse_unicode_escape(),
            _ => return Err(Error::InvalidAminoJson),
        };
        Ok(ch)
    }

    fn parse_unicode_escape(&mut self) -> Result<char, Error> {
        let mut hex = [0u8; 4];
        for slot in &mut hex {
            *slot = self.bump()?;
        }
        let text = std::str::from_utf8(&hex).map_err(|_| Error::InvalidAminoJson)?;
        let code = u32::from_str_radix(text, 16).map_err(|_| Error::InvalidAminoJson)?;
        char::from_u32(code).ok_or(Error::InvalidAminoJson)
    }
}
