//! JSON read as Go's decoder reads it: a `null` leaves a field at its zero
//! value, where serde would refuse a null for a string, number or list.
//! APIs write null for what's unset (a session not completed, a delta
//! with no content), and one such field mustn't lose the whole answer.

use serde::de::DeserializeOwned;
use serde_json::Value;

/// b as T, every null in an object read as the field left out.
pub(crate) fn from_slice<T: DeserializeOwned>(b: &[u8]) -> serde_json::Result<T> {
    let mut v: Value = serde_json::from_slice(b)?;
    drop_nulls(&mut v);
    T::deserialize(v)
}

fn drop_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(drop_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(drop_nulls),
        _ => {}
    }
}
