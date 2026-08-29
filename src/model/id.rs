use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }

            /// Mint a fresh random (version 4) identifier.
            ///
            /// # Panics
            ///
            /// Panics only if the operating system cannot provide randomness.
            #[must_use]
            pub fn generate() -> Self {
                let mut bytes = [0u8; 16];
                getrandom::fill(&mut bytes).expect("system randomness is available");
                bytes[6] = (bytes[6] & 0x0f) | 0x40; // RFC 4122 version 4
                bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
                Self(bytes)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_uuid(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                format_uuid(&self.0, formatter)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                String::deserialize(deserializer)?
                    .parse()
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}

uuid_id!(ProfileId);
uuid_id!(SessionId);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CredentialKey([u8; 32]);

impl CredentialKey {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for CredentialKey {
    type Err = IdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; 32];
        decode_hex(value.as_bytes(), &mut bytes)?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for CredentialKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for CredentialKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CredentialKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

fn parse_uuid(value: &str) -> Result<[u8; 16], IdParseError> {
    if value.len() != 36
        || !value
            .bytes()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) == (byte == b'-'))
    {
        return Err(IdParseError::InvalidUuid);
    }
    let compact = value
        .bytes()
        .filter(|byte| *byte != b'-')
        .collect::<Vec<_>>();
    let mut bytes = [0; 16];
    decode_hex(&compact, &mut bytes)?;
    Ok(bytes)
}

fn decode_hex(value: &[u8], output: &mut [u8]) -> Result<(), IdParseError> {
    if value.len() != output.len() * 2 {
        return Err(IdParseError::InvalidLength);
    }
    for (destination, pair) in output.iter_mut().zip(value.chunks_exact(2)) {
        *destination = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Ok(())
}

const fn hex_digit(value: u8) -> Result<u8, IdParseError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(IdParseError::InvalidHex),
    }
}

fn format_uuid(bytes: &[u8; 16], formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    for (index, byte) in bytes.iter().enumerate() {
        if [4, 6, 8, 10].contains(&index) {
            formatter.write_str("-")?;
        }
        write!(formatter, "{byte:02x}")?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum IdParseError {
    InvalidUuid,
    InvalidLength,
    InvalidHex,
}

impl fmt::Display for IdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUuid => formatter.write_str("identifier must use canonical UUID syntax"),
            Self::InvalidLength => formatter.write_str("identifier has the wrong length"),
            Self::InvalidHex => {
                formatter.write_str("identifier contains a non-hexadecimal character")
            }
        }
    }
}

impl std::error::Error for IdParseError {}

#[cfg(test)]
mod tests {
    use super::{CredentialKey, ProfileId};

    #[test]
    fn uuid_ids_have_canonical_roundtrips() {
        let text = "550e8400-e29b-41d4-a716-446655440000";
        let id = text.parse::<ProfileId>().unwrap();
        assert_eq!(id.to_string(), text);
        let yaml = serde_yaml_ng::to_string(&id).unwrap();
        assert_eq!(serde_yaml_ng::from_str::<ProfileId>(&yaml).unwrap(), id);
    }

    #[test]
    fn generated_ids_are_distinct_version_4_uuids() {
        let first = ProfileId::generate();
        let second = ProfileId::generate();
        assert_ne!(first, second);
        let text = first.to_string();
        assert!(text.parse::<ProfileId>().is_ok());
        assert_eq!(text.as_bytes()[14], b'4');
        assert!(matches!(text.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn credential_keys_are_fixed_length_hex() {
        let text = "ab".repeat(32);
        let key = text.parse::<CredentialKey>().unwrap();
        assert_eq!(key.to_string(), text);
        assert!("ab".repeat(31).parse::<CredentialKey>().is_err());
    }
}
