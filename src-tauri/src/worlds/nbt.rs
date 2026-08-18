//! Just enough NBT to read a `level.dat`.
//!
//! `level.dat` is gzipped NBT in practice, but an uncompressed file is valid and
//! does occur (some tools write one, and a corrupted gzip header should not make
//! the world invisible), so both are accepted.

use std::collections::BTreeMap;
use std::io::Read;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<Value>),
    Compound(BTreeMap<String, Value>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Value {
    pub fn as_compound(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Compound(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    /// Any integral tag as an i64, since NBT writers are inconsistent about width.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Byte(value) => Some(*value as i64),
            Value::Short(value) => Some(*value as i64),
            Value::Int(value) => Some(*value as i64),
            Value::Long(value) => Some(*value),
            _ => None,
        }
    }

    /// Follows a path of compound keys: `get_path(["Data", "LevelName"])`.
    pub fn get_path(&self, path: &[&str]) -> Option<&Value> {
        let mut current = self;
        for key in path {
            current = current.as_compound()?.get(*key)?;
        }
        Some(current)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, count: usize) -> AppResult<&'a [u8]> {
        let end = self.position.checked_add(count).ok_or_else(truncated)?;
        if end > self.bytes.len() {
            return Err(truncated());
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> AppResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn i16(&mut self) -> AppResult<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().map_err(|_| truncated())?))
    }

    fn i32(&mut self) -> AppResult<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().map_err(|_| truncated())?))
    }

    fn i64(&mut self) -> AppResult<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().map_err(|_| truncated())?))
    }

    fn string(&mut self) -> AppResult<String> {
        let length = self.i16()? as usize;
        let bytes = self.take(length)?;
        // NBT uses modified UTF-8; lossy decoding keeps odd names readable
        // rather than failing the whole file.
        Ok(String::from_utf8_lossy(bytes).to_string())
    }

    fn payload(&mut self, tag: u8, depth: usize) -> AppResult<Value> {
        if depth > 64 {
            return Err(AppError::Other("NBT nesting is implausibly deep".into()));
        }
        Ok(match tag {
            1 => Value::Byte(self.u8()? as i8),
            2 => Value::Short(self.i16()?),
            3 => Value::Int(self.i32()?),
            4 => Value::Long(self.i64()?),
            5 => Value::Float(f32::from_bits(self.i32()? as u32)),
            6 => Value::Double(f64::from_bits(self.i64()? as u64)),
            7 => {
                let length = self.i32()?.max(0) as usize;
                Value::ByteArray(self.take(length)?.iter().map(|b| *b as i8).collect())
            }
            8 => Value::String(self.string()?),
            9 => {
                let element = self.u8()?;
                let length = self.i32()?.max(0) as usize;
                let mut items = Vec::with_capacity(length.min(1024));
                for _ in 0..length {
                    items.push(self.payload(element, depth + 1)?);
                }
                Value::List(items)
            }
            10 => {
                let mut map = BTreeMap::new();
                loop {
                    let tag = self.u8()?;
                    if tag == 0 {
                        break;
                    }
                    let name = self.string()?;
                    map.insert(name, self.payload(tag, depth + 1)?);
                }
                Value::Compound(map)
            }
            11 => {
                let length = self.i32()?.max(0) as usize;
                let mut items = Vec::with_capacity(length.min(1024));
                for _ in 0..length {
                    items.push(self.i32()?);
                }
                Value::IntArray(items)
            }
            12 => {
                let length = self.i32()?.max(0) as usize;
                let mut items = Vec::with_capacity(length.min(1024));
                for _ in 0..length {
                    items.push(self.i64()?);
                }
                Value::LongArray(items)
            }
            other => return Err(AppError::Other(format!("unknown NBT tag {other}"))),
        })
    }
}

fn truncated() -> AppError {
    AppError::Other("the NBT data ends unexpectedly".into())
}

/// Parses NBT, transparently gunzipping when the data is compressed.
pub fn parse(bytes: &[u8]) -> AppResult<Value> {
    let decompressed;
    let data = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map_err(|e| AppError::Other(format!("could not decompress the NBT data: {e}")))?;
        decompressed = out;
        &decompressed[..]
    } else {
        bytes
    };

    let mut reader = Reader::new(data);
    let tag = reader.u8()?;
    if tag != 10 {
        return Err(AppError::Other(
            "this file does not start with an NBT compound".into(),
        ));
    }
    let _root_name = reader.string()?;
    reader.payload(10, 0)
}

#[cfg(test)]
pub(crate) mod build {
    //! Minimal NBT writer, used by tests to produce realistic `level.dat` data.
    use std::io::Write;

    pub fn string(value: &str) -> Vec<u8> {
        let mut out = (value.len() as i16).to_be_bytes().to_vec();
        out.extend_from_slice(value.as_bytes());
        out
    }

    pub fn named(tag: u8, name: &str, mut payload: Vec<u8>) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend(string(name));
        out.append(&mut payload);
        out
    }

    pub fn compound(name: &str, body: Vec<u8>) -> Vec<u8> {
        let mut out = named(10, name, body);
        out.push(0);
        out
    }

    pub fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    /// A `level.dat` shaped like the real thing.
    pub fn level_dat(name: &str, seed: i64, game_type: i32, last_played: i64) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(named(8, "LevelName", string(name)));
        data.extend(named(3, "GameType", game_type.to_be_bytes().to_vec()));
        data.extend(named(4, "LastPlayed", last_played.to_be_bytes().to_vec()));
        data.extend(named(1, "hardcore", vec![0]));
        data.extend(named(8, "Version", string("ignored")));

        // Modern layout: the seed lives under WorldGenSettings.
        let mut world_gen = Vec::new();
        world_gen.extend(named(4, "seed", seed.to_be_bytes().to_vec()));
        data.extend(compound("WorldGenSettings", world_gen));

        let mut version = Vec::new();
        version.extend(named(8, "Name", string("1.21.4")));
        version.extend(named(3, "Id", 4189i32.to_be_bytes().to_vec()));
        data.extend(compound("Version", version));

        let mut root = Vec::new();
        root.extend(compound("Data", data));
        compound("", root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_gzipped_level_dat() {
        let raw = build::level_dat("My World", 1234567890, 0, 1_700_000_000_000);
        let parsed = parse(&build::gzip(&raw)).unwrap();

        assert_eq!(
            parsed.get_path(&["Data", "LevelName"]).and_then(Value::as_string),
            Some("My World")
        );
        assert_eq!(
            parsed
                .get_path(&["Data", "WorldGenSettings", "seed"])
                .and_then(Value::as_i64),
            Some(1234567890)
        );
        assert_eq!(
            parsed.get_path(&["Data", "GameType"]).and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            parsed.get_path(&["Data", "Version", "Name"]).and_then(Value::as_string),
            Some("1.21.4")
        );
    }

    #[test]
    fn reads_uncompressed_nbt_too() {
        let raw = build::level_dat("Plain", 42, 1, 0);
        let parsed = parse(&raw).unwrap();
        assert_eq!(
            parsed.get_path(&["Data", "LevelName"]).and_then(Value::as_string),
            Some("Plain")
        );
    }

    #[test]
    fn every_integral_width_reads_as_i64() {
        assert_eq!(Value::Byte(-3).as_i64(), Some(-3));
        assert_eq!(Value::Short(300).as_i64(), Some(300));
        assert_eq!(Value::Int(70_000).as_i64(), Some(70_000));
        assert_eq!(Value::Long(9_000_000_000).as_i64(), Some(9_000_000_000));
        assert_eq!(Value::String("x".into()).as_i64(), None);
    }

    #[test]
    fn a_missing_path_is_none_not_a_panic() {
        let parsed = parse(&build::gzip(&build::level_dat("W", 1, 0, 0))).unwrap();
        assert!(parsed.get_path(&["Data", "NoSuchKey"]).is_none());
        assert!(parsed.get_path(&["Nope", "Deeper"]).is_none());
    }

    #[test]
    fn truncated_data_is_an_error_rather_than_a_panic() {
        let raw = build::gzip(&build::level_dat("W", 1, 0, 0));
        let err = parse(&raw[..raw.len() / 2]).unwrap_err();
        assert!(err.to_string().len() > 5, "{err}");

        let err = parse(b"").unwrap_err();
        assert!(err.to_string().contains("ends unexpectedly"), "{err}");
    }

    #[test]
    fn a_file_that_is_not_nbt_is_rejected_clearly() {
        let err = parse(b"just some text, not NBT at all").unwrap_err();
        assert!(err.to_string().contains("NBT compound"), "{err}");
    }

    #[test]
    fn lists_and_arrays_parse() {
        let mut body = Vec::new();
        // A list of two strings.
        let mut list = vec![8u8];
        list.extend(2i32.to_be_bytes());
        list.extend(build::string("a"));
        list.extend(build::string("b"));
        body.extend(build::named(9, "names", list));
        // An int array.
        let mut ints = 3i32.to_be_bytes().to_vec();
        for value in [1i32, 2, 3] {
            ints.extend(value.to_be_bytes());
        }
        body.extend(build::named(11, "numbers", ints));

        let parsed = parse(&build::compound("", body)).unwrap();
        assert_eq!(
            parsed.get_path(&["names"]),
            Some(&Value::List(vec![
                Value::String("a".into()),
                Value::String("b".into())
            ]))
        );
        assert_eq!(parsed.get_path(&["numbers"]), Some(&Value::IntArray(vec![1, 2, 3])));
    }
}
