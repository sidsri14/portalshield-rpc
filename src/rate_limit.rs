use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const WINDOW: Duration = Duration::from_secs(1);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct ClientWindow {
    started_at: Instant,
    requests: u32,
}

#[derive(Debug)]
struct LimiterState {
    clients: HashMap<IpAddr, ClientWindow>,
    last_cleanup: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    requests_per_second: u32,
    state: Mutex<LimiterState>,
}

impl RateLimiter {
    pub fn new(requests_per_second: u32) -> Self {
        let now = Instant::now();
        Self {
            requests_per_second,
            state: Mutex::new(LimiterState {
                clients: HashMap::new(),
                last_cleanup: now,
            }),
        }
    }

    pub async fn allow(&self, client_ip: IpAddr) -> bool {
        self.allow_at(client_ip, Instant::now()).await
    }

    async fn allow_at(&self, client_ip: IpAddr, now: Instant) -> bool {
        if self.requests_per_second == 0 {
            return true;
        }

        let mut state = self.state.lock().await;
        if now.duration_since(state.last_cleanup) >= CLEANUP_INTERVAL {
            state
                .clients
                .retain(|_, window| now.duration_since(window.started_at) < CLEANUP_INTERVAL);
            state.last_cleanup = now;
        }

        let window = state.clients.entry(client_ip).or_insert(ClientWindow {
            started_at: now,
            requests: 0,
        });

        if now.duration_since(window.started_at) >= WINDOW {
            window.started_at = now;
            window.requests = 0;
        }

        if window.requests >= self.requests_per_second {
            return false;
        }

        window.requests += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn limits_each_client_independently_and_resets() {
        let limiter = RateLimiter::new(2);
        let first = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
        let second = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));
        let start = Instant::now();

        assert!(limiter.allow_at(first, start).await);
        assert!(limiter.allow_at(first, start).await);
        assert!(!limiter.allow_at(first, start).await);
        assert!(limiter.allow_at(second, start).await);
        assert!(limiter.allow_at(first, start + WINDOW).await);
    }

    #[tokio::test]
    async fn zero_disables_rate_limiting() {
        let limiter = RateLimiter::new(0);
        let client = IpAddr::V4(Ipv4Addr::LOCALHOST);

        for _ in 0..10 {
            assert!(limiter.allow(client).await);
        }
    }
}
