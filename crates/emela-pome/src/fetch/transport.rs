//! HTTP GET, behind a trait so that tests (and targets without sockets) can supply their own.

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().to_string()
    }
}

pub trait Transport {
    /// GET `url`. `Err` is a failure to get any answer (DNS, TLS, timeout); any HTTP status is
    /// `Ok`.
    fn get(&self, url: &str) -> Result<Response, String>;
}

/// The largest body read: an archive is at most 32 MiB of files, plus tar headers.
pub const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(feature = "net")]
pub use net::Http;

#[cfg(feature = "net")]
mod net {
    use std::time::Duration;

    use super::{MAX_BODY_BYTES, Response, Transport};

    /// The transport over the network.
    pub struct Http {
        agent: ureq::Agent,
    }

    impl Http {
        pub fn new() -> Http {
            let agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(60)))
                .user_agent(concat!("emela/", env!("CARGO_PKG_VERSION")))
                .build()
                .into();
            Http { agent }
        }
    }

    impl Default for Http {
        fn default() -> Self {
            Http::new()
        }
    }

    impl Transport for Http {
        fn get(&self, url: &str) -> Result<Response, String> {
            let mut res = self
                .agent
                .get(url)
                .call()
                .map_err(|e| format!("GET {url}: {e}"))?;
            let status = res.status().as_u16();
            let body = res
                .body_mut()
                .with_config()
                .limit(MAX_BODY_BYTES)
                .read_to_vec()
                .map_err(|e| format!("GET {url}: {e}"))?;
            Ok(Response { status, body })
        }
    }
}
