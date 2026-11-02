//! Reading Pi's JSON as Go's decoder does: null (or a missing field, with
//! `default`) leaves a field at its zero value.

use serde::{Deserialize, Deserializer};

/// Null as the type's zero value.
pub(crate) fn nd<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
