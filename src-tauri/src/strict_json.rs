use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}
struct StrictVisitor;
impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON without duplicate object keys")
    }
    fn visit_bool<E: Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(v)))
    }
    fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(v.into())))
    }
    fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(v.into())))
    }
    fn visit_f64<E: Error>(self, v: f64) -> Result<Self::Value, E> {
        Number::from_f64(v)
            .map(|n| StrictValue(Value::Number(n)))
            .ok_or_else(|| E::custom("invalid JSON number"))
    }
    fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(v.into())))
    }
    fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(v)))
    }
    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }
    fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(StrictValue(value)) = sequence.next_element()? {
            values.push(value);
        }
        Ok(StrictValue(Value::Array(values)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate JSON object key"));
            }
            let StrictValue(value) = map.next_value()?;
            values.insert(key, value);
        }
        Ok(StrictValue(Value::Object(values)))
    }
}
pub(crate) fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str::<StrictValue>(text).map(|v| v.0)
}

pub(crate) fn parse_provider_output(text: &str) -> Result<Value, serde_json::Error> {
    let invalid = |message: &str| <serde_json::Error as serde::de::Error>::custom(message);
    if text.len() > 256 * 1024 {
        return Err(invalid("provider JSON exceeds byte limit"));
    }
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 32 {
                        return Err(invalid("provider JSON exceeds nesting limit"));
                    }
                }
                b'}' | b']' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| invalid("invalid JSON nesting"))?;
                }
                _ => (),
            }
        }
    }
    parse(text)
}

#[cfg(test)]
mod bounded_provider_tests {
    use super::*;
    #[test]
    fn exact_depth_bytes_escapes_and_duplicates_are_bounded() {
        let nested = |depth: usize| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        assert!(parse_provider_output(&nested(32)).is_ok());
        assert!(parse_provider_output(&nested(33)).is_err());
        let exact = format!("\"{}\"", "x".repeat(256 * 1024 - 2));
        assert!(parse_provider_output(&exact).is_ok());
        assert!(parse_provider_output(&(exact + " ")).is_err());
        assert!(parse_provider_output(r#"{"x":"[\"{}]","n":{"a":1,"a":2}}"#).is_err());
        assert!(parse_provider_output(r#"{"x":"[\"{}]"}"#).is_ok());
    }
}
