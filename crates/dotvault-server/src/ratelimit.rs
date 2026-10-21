//! A tiny in-memory fixed-window rate limiter keyed by client address.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

use crate::config::RateLimit;

/// Soft cap on tracked clients; above it stale windows are swept on insert.
const SWEEP_THRESHOLD: usize = 4096;
/// Hard cap: if sweeping cannot get below it, start over rather than grow.
const HARD_CAP: usize = 200_000;

#[derive(Debug)]
struct Bucket {
    window_start: Instant,
    count: u32,
}

#[derive(Debug)]
pub struct RateLimiter {
    limit: RateLimit,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    pub fn new(limit: RateLimit) -> Self {
        Self {
            limit,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn limit(&self) -> RateLimit {
        self.limit
    }

    /// Counts one request from `ip`. `Err` carries how long to wait.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        self.check_at(ip, Instant::now())
    }

    pub fn check_at(&self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        let key = normalize(ip);
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        if buckets.len() >= SWEEP_THRESHOLD && !buckets.contains_key(&key) {
            self.sweep(&mut buckets, now);
            if buckets.len() >= HARD_CAP {
                buckets.clear();
            }
        }

        let bucket = buckets.entry(key).or_insert(Bucket {
            window_start: now,
            count: 0,
        });
        if now.saturating_duration_since(bucket.window_start) >= self.limit.window {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= self.limit.max {
            let elapsed = now.saturating_duration_since(bucket.window_start);
            return Err(self.limit.window.saturating_sub(elapsed));
        }
        bucket.count += 1;
        Ok(())
    }

    /// Drops windows that have ended. Called periodically by the janitor.
    pub fn prune(&self) {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        self.sweep(&mut buckets, Instant::now());
    }

    pub fn tracked(&self) -> usize {
        self.buckets.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn sweep(&self, buckets: &mut HashMap<IpAddr, Bucket>, now: Instant) {
        let window = self.limit.window;
        buckets.retain(|_, b| now.saturating_duration_since(b.window_start) < window);
    }
}

/// IPv4-mapped IPv6 addresses count as the IPv4 address, and IPv6 clients are
/// grouped by /64 (one subscriber typically owns a whole /64).
fn normalize(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            octets[8..].fill(0);
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        v4 => v4,
    }
}

/// The address to rate-limit by.
///
/// Normally the socket peer. With `trust_proxy` set, the last
/// `X-Forwarded-For` entry is used instead: that is the one appended by our
/// own reverse proxy, whereas earlier entries are client-controlled. Falls
/// back to the peer when the header is missing or unparsable, and to
/// `0.0.0.0` when there is no peer information at all (only possible when the
/// router is driven without `ConnectInfo`, e.g. in tests).
pub fn client_ip(trust_proxy: bool, headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    if trust_proxy {
        let forwarded = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .rfind(|s| !s.is_empty())
            .and_then(|s| s.parse::<IpAddr>().ok());
        if let Some(ip) = forwarded {
            return ip;
        }
    }
    peer.map(|p| p.ip()).unwrap_or(IpAddr::from([0, 0, 0, 0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(max: u32, secs: u64) -> RateLimiter {
        RateLimiter::new(RateLimit {
            max,
            window: Duration::from_secs(secs),
        })
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn allows_up_to_max_then_blocks() {
        let rl = limiter(3, 60);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(rl.check_at(ip("1.2.3.4"), t0).is_ok());
        }
        let wait = rl.check_at(ip("1.2.3.4"), t0).unwrap_err();
        assert_eq!(wait, Duration::from_secs(60));
        // still blocked later in the same window, with a shrinking wait
        let wait = rl
            .check_at(ip("1.2.3.4"), t0 + Duration::from_secs(45))
            .unwrap_err();
        assert_eq!(wait, Duration::from_secs(15));
    }

    #[test]
    fn window_resets() {
        let rl = limiter(1, 10);
        let t0 = Instant::now();
        assert!(rl.check_at(ip("1.1.1.1"), t0).is_ok());
        assert!(rl.check_at(ip("1.1.1.1"), t0).is_err());
        assert!(rl
            .check_at(ip("1.1.1.1"), t0 + Duration::from_secs(10))
            .is_ok());
    }

    #[test]
    fn clients_are_independent() {
        let rl = limiter(1, 60);
        let t0 = Instant::now();
        assert!(rl.check_at(ip("1.1.1.1"), t0).is_ok());
        assert!(rl.check_at(ip("1.1.1.1"), t0).is_err());
        assert!(rl.check_at(ip("2.2.2.2"), t0).is_ok());
    }

    #[test]
    fn ipv6_is_grouped_by_64_and_mapped_v4_is_v4() {
        let rl = limiter(1, 60);
        let t0 = Instant::now();
        assert!(rl.check_at(ip("2001:db8:1:2::1"), t0).is_ok());
        assert!(rl.check_at(ip("2001:db8:1:2:ffff::9"), t0).is_err());
        assert!(rl.check_at(ip("2001:db8:1:3::1"), t0).is_ok());

        assert!(rl.check_at(ip("9.9.9.9"), t0).is_ok());
        assert!(rl.check_at(ip("::ffff:9.9.9.9"), t0).is_err());
    }

    #[test]
    fn prune_drops_finished_windows_only() {
        let rl = limiter(5, 0); // zero-length windows end immediately
        let _ = rl.check(ip("1.1.1.1"));
        assert_eq!(rl.tracked(), 1);
        rl.prune();
        assert_eq!(rl.tracked(), 0);

        let rl = limiter(5, 3600);
        let _ = rl.check(ip("1.1.1.1"));
        rl.prune();
        assert_eq!(rl.tracked(), 1);
    }

    #[test]
    fn stale_entries_are_swept_when_the_table_grows() {
        let rl = limiter(5, 10);
        let t0 = Instant::now();
        for i in 0..SWEEP_THRESHOLD as u32 {
            let ip = IpAddr::from(i.to_be_bytes());
            assert!(rl.check_at(ip, t0).is_ok());
        }
        assert_eq!(rl.tracked(), SWEEP_THRESHOLD);
        assert!(rl
            .check_at(ip("10.255.255.255"), t0 + Duration::from_secs(11))
            .is_ok());
        assert_eq!(rl.tracked(), 1);
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn client_ip_ignores_forwarded_for_unless_trusted() {
        let peer = Some("10.0.0.1:5000".parse().unwrap());
        let h = headers(&[("x-forwarded-for", "203.0.113.7")]);
        assert_eq!(client_ip(false, &h, peer), ip("10.0.0.1"));
        assert_eq!(client_ip(true, &h, peer), ip("203.0.113.7"));
    }

    #[test]
    fn trusted_proxy_uses_the_last_entry() {
        let peer = Some("10.0.0.1:5000".parse().unwrap());
        // the client claims 6.6.6.6; our proxy appended 203.0.113.7
        let h = headers(&[("x-forwarded-for", "6.6.6.6, 203.0.113.7")]);
        assert_eq!(client_ip(true, &h, peer), ip("203.0.113.7"));
        let h = headers(&[
            ("x-forwarded-for", "6.6.6.6"),
            ("x-forwarded-for", "203.0.113.8"),
        ]);
        assert_eq!(client_ip(true, &h, peer), ip("203.0.113.8"));
    }

    #[test]
    fn client_ip_falls_back_to_the_peer() {
        let peer = Some("10.0.0.1:5000".parse().unwrap());
        let junk = headers(&[("x-forwarded-for", "not an ip")]);
        assert_eq!(client_ip(true, &junk, peer), ip("10.0.0.1"));
        assert_eq!(client_ip(true, &HeaderMap::new(), peer), ip("10.0.0.1"));
        assert_eq!(client_ip(false, &HeaderMap::new(), None), ip("0.0.0.0"));
    }
}
