use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

pub const DEFAULT_RDP_PORT: u16 = 3389;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Host {
    Hostname(String),
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
}

impl Host {
    /// Parse a hostname or IP address without a port.
    ///
    /// # Errors
    ///
    /// Returns an error for empty values, endpoint delimiters, whitespace,
    /// control characters, or a malformed value that resembles an IP address.
    pub fn parse(value: &str) -> Result<Self, EndpointParseError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(EndpointParseError::EmptyHost);
        }
        if let Ok(address) = value.parse::<Ipv4Addr>() {
            return Ok(Self::Ipv4(address));
        }
        if let Ok(address) = value.parse::<Ipv6Addr>() {
            return Ok(Self::Ipv6(address));
        }
        if value.contains('.')
            && value
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
        {
            return Err(EndpointParseError::InvalidHost(value.to_owned()));
        }
        if value.contains([':', '[', ']', '/', '\\'])
            || value.chars().any(char::is_whitespace)
            || value.chars().any(char::is_control)
        {
            return Err(EndpointParseError::InvalidHost(value.to_owned()));
        }
        if value.len() > 253 {
            return Err(EndpointParseError::InvalidHost(value.to_owned()));
        }
        Ok(Self::Hostname(value.to_owned()))
    }
}

impl fmt::Display for Host {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hostname(hostname) => formatter.write_str(hostname),
            Self::Ipv4(address) => address.fmt(formatter),
            Self::Ipv6(address) => address.fmt(formatter),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Endpoint {
    pub host: Host,
    pub port: u16,
}

impl Endpoint {
    /// Construct an endpoint from an already-parsed host and a nonzero port.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointParseError::InvalidPort`] when `port` is zero.
    pub fn new(host: Host, port: u16) -> Result<Self, EndpointParseError> {
        if port == 0 {
            Err(EndpointParseError::InvalidPort)
        } else {
            Ok(Self { host, port })
        }
    }
}

impl FromStr for Endpoint {
    type Err = EndpointParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.is_empty() {
            return Err(EndpointParseError::EmptyHost);
        }
        if let Some(bracketed) = input.strip_prefix('[') {
            return parse_bracketed_ipv6(bracketed);
        }
        if input.matches(':').count() > 1 {
            return Err(EndpointParseError::Ipv6RequiresBrackets);
        }
        let (host, port) = match input.rsplit_once(':') {
            Some((host, port)) => (host, parse_port(port)?),
            None => (input, DEFAULT_RDP_PORT),
        };
        Self::new(Host::parse(host)?, port)
    }
}

fn parse_bracketed_ipv6(input: &str) -> Result<Endpoint, EndpointParseError> {
    let (address, suffix) = input
        .split_once(']')
        .ok_or(EndpointParseError::MissingIpv6Bracket)?;
    let address = address
        .parse::<Ipv6Addr>()
        .map_err(|_| EndpointParseError::InvalidIpv6(address.to_owned()))?;
    let port = if suffix.is_empty() {
        DEFAULT_RDP_PORT
    } else {
        parse_port(
            suffix
                .strip_prefix(':')
                .ok_or(EndpointParseError::UnexpectedIpv6Suffix)?,
        )?
    };
    Endpoint::new(Host::Ipv6(address), port)
}

fn parse_port(value: &str) -> Result<u16, EndpointParseError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or(EndpointParseError::InvalidPort)
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Host::Ipv6(address) => write!(formatter, "[{address}]:{}", self.port),
            host => write!(formatter, "{host}:{}", self.port),
        }
    }
}

impl Serialize for Endpoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Endpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum EndpointParseError {
    EmptyHost,
    InvalidHost(String),
    InvalidPort,
    Ipv6RequiresBrackets,
    MissingIpv6Bracket,
    InvalidIpv6(String),
    UnexpectedIpv6Suffix,
}

impl fmt::Display for EndpointParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyHost => formatter.write_str("endpoint host is empty"),
            Self::InvalidHost(host) => write!(formatter, "invalid endpoint host {host:?}"),
            Self::InvalidPort => {
                formatter.write_str("endpoint port must be an integer from 1 to 65535")
            }
            Self::Ipv6RequiresBrackets => {
                formatter.write_str("IPv6 endpoints must use [address] or [address]:port syntax")
            }
            Self::MissingIpv6Bracket => {
                formatter.write_str("IPv6 endpoint is missing its closing bracket")
            }
            Self::InvalidIpv6(address) => write!(formatter, "invalid IPv6 address {address:?}"),
            Self::UnexpectedIpv6Suffix => {
                formatter.write_str("unexpected text after bracketed IPv6 address")
            }
        }
    }
}

impl std::error::Error for EndpointParseError {}

#[cfg(test)]
mod tests {
    use super::{Endpoint, EndpointParseError, Host};
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parses_supported_endpoint_forms() {
        assert_eq!(
            "anima.local".parse::<Endpoint>().unwrap(),
            Endpoint::new(Host::Hostname("anima.local".into()), 3389).unwrap()
        );
        assert_eq!(
            "anima:3390".parse::<Endpoint>().unwrap(),
            Endpoint::new(Host::Hostname("anima".into()), 3390).unwrap()
        );
        assert_eq!(
            "10.0.0.111".parse::<Endpoint>().unwrap().host,
            Host::Ipv4(Ipv4Addr::new(10, 0, 0, 111))
        );
        assert_eq!(
            "[2001:db8::1]:3391".parse::<Endpoint>().unwrap(),
            Endpoint::new(Host::Ipv6("2001:db8::1".parse::<Ipv6Addr>().unwrap()), 3391).unwrap()
        );
    }

    #[test]
    fn canonical_display_is_unambiguous() {
        assert_eq!(
            "anima".parse::<Endpoint>().unwrap().to_string(),
            "anima:3389"
        );
        assert_eq!(
            "[2001:0db8::1]".parse::<Endpoint>().unwrap().to_string(),
            "[2001:db8::1]:3389"
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_endpoints() {
        assert_eq!(
            "2001:db8::1".parse::<Endpoint>(),
            Err(EndpointParseError::Ipv6RequiresBrackets)
        );
        assert_eq!(
            "host:0".parse::<Endpoint>(),
            Err(EndpointParseError::InvalidPort)
        );
        assert!(matches!(
            "host name".parse::<Endpoint>(),
            Err(EndpointParseError::InvalidHost(_))
        ));
        assert!(matches!(
            "999.999.999.999".parse::<Endpoint>(),
            Err(EndpointParseError::InvalidHost(_))
        ));
    }

    #[test]
    fn serde_uses_the_canonical_parser() {
        let endpoint = "[2001:db8::1]:3390".parse::<Endpoint>().unwrap();
        let yaml = serde_yaml_ng::to_string(&endpoint).unwrap();
        assert_eq!(
            serde_yaml_ng::from_str::<Endpoint>(&yaml).unwrap(),
            endpoint
        );
    }
}
