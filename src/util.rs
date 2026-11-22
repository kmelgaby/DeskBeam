//! Small shared helpers: one-time codes, addresses, time.

use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

/// Generate a 9-digit one-time code from the OS CSPRNG.
pub fn generate_code() -> String {
    let mut raw = [0u8; 9];
    if getrandom::fill(&mut raw).is_err() {
        // Extremely unlikely; fall back to a time-derived value rather than panic.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in raw.iter_mut().enumerate() {
            *b = ((nanos >> (i * 8)) & 0xff) as u8;
        }
    }
    let digits: String = raw
        .iter()
        .map(|b| char::from(b'0' + (b % 10)))
        .collect();
    format_code(&digits)
}

/// `123456789` -> `123-456-789`. Leaves other lengths unchanged.
pub fn format_code(digits: &str) -> String {
    let digits: String = digits.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 9 {
        return digits;
    }
    format!("{}-{}-{}", &digits[0..3], &digits[3..6], &digits[6..9])
}

/// All non-loopback, non-link-local IPv4 addresses with interface names.
pub fn local_ipv4_addresses() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(ifas) = local_ip_address::list_afinet_netifas() {
        for (name, ip) in ifas {
            if let IpAddr::V4(v4) = ip {
                if !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() {
                    out.push(format!("{v4}  ({name})"));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "this-pc".to_string())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn format_kbps(kbps: f32) -> String {
    if kbps >= 1000.0 {
        format!("{:.1} Mbps", kbps / 1000.0)
    } else {
        format!("{kbps:.0} kbps")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_format() {
        assert_eq!(format_code("123456789"), "123-456-789");
        assert_eq!(format_code("12345678"), "12345678");
        let code = generate_code();
        assert_eq!(code.len(), 11);
        assert_eq!(code.chars().filter(|c| c.is_ascii_digit()).count(), 9);
    }
}
