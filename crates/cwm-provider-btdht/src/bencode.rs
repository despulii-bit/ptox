//! Minimal bencode (BEP 3) encoder/decoder.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    pub fn int(value: i64) -> Self {
        Value::Int(value)
    }

    pub fn bytes(value: impl Into<Vec<u8>>) -> Self {
        Value::Bytes(value.into())
    }

    pub fn string(value: &str) -> Self {
        Value::Bytes(value.as_bytes().to_vec())
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(map) => Some(map),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict().and_then(|map| map.get(key.as_bytes()))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BencodeError {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("invalid bencode at byte {0}")]
    Invalid(usize),
    #[error("trailing data after value")]
    Trailing,
}

pub fn decode(input: &[u8]) -> Result<Value, BencodeError> {
    let mut pos = 0;
    let value = decode_value(input, &mut pos)?;
    if pos != input.len() {
        return Err(BencodeError::Trailing);
    }
    Ok(value)
}

fn decode_value(input: &[u8], pos: &mut usize) -> Result<Value, BencodeError> {
    match input
        .get(*pos)
        .copied()
        .ok_or(BencodeError::UnexpectedEof)?
    {
        b'i' => {
            *pos += 1;
            let end = find(input, *pos, b'e')?;
            let text =
                std::str::from_utf8(&input[*pos..end]).map_err(|_| BencodeError::Invalid(*pos))?;
            let number = text
                .parse::<i64>()
                .map_err(|_| BencodeError::Invalid(*pos))?;
            *pos = end + 1;
            Ok(Value::Int(number))
        }
        b'l' => {
            *pos += 1;
            let mut items = Vec::new();
            while input.get(*pos) != Some(&b'e') {
                items.push(decode_value(input, pos)?);
            }
            *pos += 1;
            Ok(Value::List(items))
        }
        b'd' => {
            *pos += 1;
            let mut map = BTreeMap::new();
            while input.get(*pos) != Some(&b'e') {
                let key = match decode_value(input, pos)? {
                    Value::Bytes(bytes) => bytes,
                    _ => return Err(BencodeError::Invalid(*pos)),
                };
                let value = decode_value(input, pos)?;
                map.insert(key, value);
            }
            *pos += 1;
            Ok(Value::Dict(map))
        }
        b'0'..=b'9' => {
            let colon = find(input, *pos, b':')?;
            let text = std::str::from_utf8(&input[*pos..colon])
                .map_err(|_| BencodeError::Invalid(*pos))?;
            let len = text
                .parse::<usize>()
                .map_err(|_| BencodeError::Invalid(*pos))?;
            let start = colon + 1;
            let end = start + len;
            if end > input.len() {
                return Err(BencodeError::UnexpectedEof);
            }
            *pos = end;
            Ok(Value::Bytes(input[start..end].to_vec()))
        }
        _ => Err(BencodeError::Invalid(*pos)),
    }
}

fn find(input: &[u8], from: usize, byte: u8) -> Result<usize, BencodeError> {
    input[from..]
        .iter()
        .position(|value| *value == byte)
        .map(|offset| offset + from)
        .ok_or(BencodeError::UnexpectedEof)
}

pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_value(value, &mut out);
    out
}

fn encode_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Int(number) => {
            out.push(b'i');
            out.extend_from_slice(number.to_string().as_bytes());
            out.push(b'e');
        }
        Value::Bytes(bytes) => {
            out.extend_from_slice(bytes.len().to_string().as_bytes());
            out.push(b':');
            out.extend_from_slice(bytes);
        }
        Value::List(items) => {
            out.push(b'l');
            for item in items {
                encode_value(item, out);
            }
            out.push(b'e');
        }
        Value::Dict(map) => {
            out.push(b'd');
            for (key, value) in map {
                out.extend_from_slice(key.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(key);
                encode_value(value, out);
            }
            out.push(b'e');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_scalars() {
        assert_eq!(decode(b"i42e"), Ok(Value::Int(42)));
        assert_eq!(decode(b"i-3e"), Ok(Value::Int(-3)));
        assert_eq!(decode(b"4:spam"), Ok(Value::Bytes(b"spam".to_vec())));
        assert_eq!(decode(b"0:"), Ok(Value::Bytes(Vec::new())));
    }

    #[test]
    fn decodes_lists_and_dicts() {
        assert_eq!(
            decode(b"l4:spami3ee"),
            Ok(Value::List(vec![
                Value::Bytes(b"spam".to_vec()),
                Value::Int(3)
            ]))
        );
        let decoded = decode(b"d3:bar4:spam3:fooi42ee").expect("dict");
        assert_eq!(decoded.get("foo"), Some(&Value::Int(42)));
        assert_eq!(decoded.get("bar"), Some(&Value::Bytes(b"spam".to_vec())));
    }

    #[test]
    fn round_trips() {
        let value = Value::Dict(BTreeMap::from([
            (b"a".to_vec(), Value::Int(1)),
            (
                b"b".to_vec(),
                Value::List(vec![Value::string("x"), Value::Int(2)]),
            ),
        ]));
        let encoded = encode(&value);
        assert_eq!(decode(&encoded), Ok(value));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(decode(b"i42"), Err(BencodeError::UnexpectedEof));
        assert_eq!(decode(b"i42ei43e"), Err(BencodeError::Trailing));
        assert_eq!(decode(b"x"), Err(BencodeError::Invalid(0)));
    }
}
