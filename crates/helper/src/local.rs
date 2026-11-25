//! Local domain and loopback address of the DevSpace sites.
//!
//! Both are configurable (`web.tld`, `web.address`) so that DevSpace can
//! coexist with other local development tools: the defaults,
//! `*.internal` on `127.0.0.2`, do not collide with Laravel Herd or Valet
//! (`*.test` on `127.0.0.1`).

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Default top-level domain: reserved by ICANN for private use (2024) and
/// never delegated on the Internet.
pub const DEFAULT_TLD: &str = "internal";

/// Default loopback address of the sites.
pub const DEFAULT_ADDRESS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);

/// Top-level domain of the local sites (`<project>.<tld>`): 1 to 63
/// lowercase ASCII letters. `local` (multicast DNS) and `localhost`
/// (resolved specially by operating systems and browsers) are refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tld(String);

impl Tld {
    /// The domain without a leading dot.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Tld {
    fn default() -> Self {
        Self(DEFAULT_TLD.to_owned())
    }
}

impl FromStr for Tld {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > 63 || !text.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(format!(
                "`{text}` is not a valid top-level domain (1 to 63 lowercase ASCII letters)"
            ));
        }
        if matches!(text, "local" | "localhost") {
            return Err(format!(
                "`{text}` is reserved for other uses by the operating system; choose another \
                 top-level domain such as `{DEFAULT_TLD}`"
            ));
        }
        Ok(Self(text.to_owned()))
    }
}

impl TryFrom<String> for Tld {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
    }
}

impl From<Tld> for String {
    fn from(tld: Tld) -> Self {
        tld.0
    }
}

impl fmt::Display for Tld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// IPv4 address of the loopback network `127.0.0.0/8` (network and
/// broadcast addresses excluded) on which the sites are served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LoopbackAddress(Ipv4Addr);

impl LoopbackAddress {
    /// The address.
    #[must_use]
    pub const fn ip(self) -> Ipv4Addr {
        self.0
    }

    /// Whether this is `127.0.0.1`, which every system configures; other
    /// addresses may need an alias on the loopback interface.
    #[must_use]
    pub fn is_primary(self) -> bool {
        self.0 == Ipv4Addr::LOCALHOST
    }
}

impl Default for LoopbackAddress {
    fn default() -> Self {
        Self(DEFAULT_ADDRESS)
    }
}

impl TryFrom<Ipv4Addr> for LoopbackAddress {
    type Error = String;

    fn try_from(ip: Ipv4Addr) -> Result<Self, String> {
        let network_or_broadcast =
            ip == Ipv4Addr::new(127, 0, 0, 0) || ip == Ipv4Addr::new(127, 255, 255, 255);
        if ip.octets()[0] != 127 || network_or_broadcast {
            return Err(format!(
                "`{ip}` is not a usable loopback address (127.0.0.1 to 127.255.255.254)"
            ));
        }
        Ok(Self(ip))
    }
}

impl FromStr for LoopbackAddress {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let ip: Ipv4Addr = text
            .parse()
            .map_err(|_| format!("`{text}` is not an IPv4 address"))?;
        ip.try_into()
    }
}

impl TryFrom<String> for LoopbackAddress {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
    }
}

impl From<LoopbackAddress> for String {
    fn from(address: LoopbackAddress) -> Self {
        address.0.to_string()
    }
}

impl fmt::Display for LoopbackAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tld_rules() {
        assert_eq!(Tld::default().as_str(), "internal");
        for valid in ["internal", "test", "dev", &"a".repeat(63)] {
            assert_eq!(valid.parse::<Tld>().unwrap().as_str(), valid);
        }
        for invalid in [
            "",
            "local",
            "localhost",
            "Test",
            "te.st",
            "t3st",
            "../x",
            &"a".repeat(64),
        ] {
            assert!(invalid.parse::<Tld>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn loopback_rules() {
        assert_eq!(LoopbackAddress::default().to_string(), "127.0.0.2");
        assert!(!LoopbackAddress::default().is_primary());
        assert!("127.0.0.1".parse::<LoopbackAddress>().unwrap().is_primary());
        for valid in ["127.0.0.2", "127.1.2.3", "127.255.255.254"] {
            assert_eq!(valid.parse::<LoopbackAddress>().unwrap().to_string(), valid);
        }
        for invalid in [
            "",
            "localhost",
            "10.0.0.1",
            "0.0.0.0",
            "127.0.0.0",
            "127.255.255.255",
            "::1",
            "127.0.0.2; rm -rf /",
        ] {
            assert!(invalid.parse::<LoopbackAddress>().is_err(), "{invalid}");
        }
    }
}
