use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[allow(clippy::trivially_copy_pass_by_ref)]
/// Serializes a `u64` as a decimal string.
///
/// # Errors
///
/// Returns the serializer's error when the string cannot be emitted.
pub fn serialize<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value.to_string().serialize(serializer)
}

/// Deserializes a decimal string into a `u64`.
///
/// # Errors
///
/// Returns the deserializer's error when the value is not a valid `u64`.
pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    value.parse().map_err(serde::de::Error::custom)
}

pub mod option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[allow(clippy::ref_option)]
    /// Serializes an optional `u64` as an optional decimal string.
    ///
    /// # Errors
    ///
    /// Returns the serializer's error when the value cannot be emitted.
    pub fn serialize<S>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.map(|value| value.to_string()).serialize(serializer)
    }

    /// Deserializes an optional decimal string into an optional `u64`.
    ///
    /// # Errors
    ///
    /// Returns the deserializer's error when the value is not a valid `u64`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)?
            .map(|value| value.parse().map_err(serde::de::Error::custom))
            .transpose()
    }
}
