//! JSON truthiness and `urllib.parse.unquote`, shared by call sites that had
//! copied the same behavior.
//!
//! [`truthy`] is the JSON predicate. `null` and `false` are false. A number is
//! true only when `as_f64` yields a float other than `0.0`: `0`, `0.0` and
//! `-0.0` are false, and a non-zero float is true. A number `as_f64` does not
//! represent is false. A string, array or object is true only when it is
//! non-empty, so an empty collection is false. Whitespace does not make a
//! string false.
//!
//! [`unquote`] is not a JSON helper. It is `urllib.parse.unquote` (not
//! `unquote_plus`) for Grok cwd display metadata and for the hub's query and
//! path decoding. Two ASCII hex digits after `%` become one byte. Any other
//! `%` stays in place and the scan resumes at the next byte, so a malformed
//! escape is unchanged (`%`, `%2`, `%2G`; `%%41` becomes `%A`). `+` stays
//! `+`. The decoded bytes are UTF-8, and invalid sequences become U+FFFD.
//! A caller that wants `unquote_plus` replaces `+` with a space and then
//! calls [`unquote`].

use serde_json::Value;

/// Truthiness of a JSON value.
///
/// `null`, `false`, zero (`0`, `0.0`, `-0.0`) and empty strings, arrays and
/// objects are false. Any other number that converts with `as_f64`, including
/// a non-zero float, is true. A number that does not convert is false.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

/// `urllib.parse.unquote`: percent-decode UTF-8.
///
/// A `%` followed by two ASCII hex digits becomes one byte. A `%` without
/// those two digits is copied unchanged and scanning continues at the next
/// byte. `+` stays `+`. Invalid UTF-8 is replaced with U+FFFD. This is not
/// `unquote_plus`.
pub fn unquote(value: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%'
            && offset + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[offset + 1]), hex(bytes[offset + 2]))
        {
            decoded.push((high << 4) | low);
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
