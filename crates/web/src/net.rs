//! Web access for the tools: as the PC's browser goes (the Windows proxy settings) first. Some sites only open with
//! the user's VPN and others only without it, so when a fetch fails it is tried once more through the user's proxy
//! (when one is set and allowed), and last with no proxy at all: a VPN's local proxy can be up but broken.

use crate::page::http_client_builder;
use reqwest::Client;
use std::future::Future;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct Net {
    direct: Client,
    proxied: Arc<RwLock<Option<Client>>>,
    /// No proxy at all, not even the Windows one.
    bare: Client,
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Net {
    pub fn new() -> Self {
        Self { direct: crate::http_client(), proxied: Arc::default(), bare: http_client_builder().no_proxy().build().unwrap_or_default() }
    }

    /// Sets the proxy for second tries (a full proxy URL), or none.
    pub fn set_proxy(&self, url: Option<&str>) -> Result<(), String> {
        let client = match url {
            Some(url) => {
                let proxy = reqwest::Proxy::all(url).map_err(|e| format!("Bad proxy address \"{url}\": {e}"))?;
                Some(http_client_builder().proxy(proxy).build().map_err(|e| e.to_string())?)
            }
            None => None,
        };
        *self.proxied.write().unwrap_or_else(|e| e.into_inner()) = client;
        Ok(())
    }

    pub fn direct(&self) -> &Client {
        &self.direct
    }

    pub fn proxied(&self) -> Option<Client> {
        self.proxied.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Like `try_both`, but an Iranian site is asked with no proxy first, then as the PC's browser goes, then through
    /// the user's proxy (no proxy is not tried twice).
    pub async fn try_for<T, F, Fut>(&self, url: &str, fetch: F) -> Result<T, String>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        if !iranian_site(url) {
            return self.try_both(fetch).await;
        }
        let bare = match fetch(self.bare.clone()).await {
            Ok(value) => return Ok(value),
            Err(e) => e,
        };
        let direct = match fetch(self.direct.clone()).await {
            Ok(value) => return Ok(value),
            Err(e) => e,
        };
        match self.proxied() {
            Some(proxied) => fetch(proxied).await.map_err(|e| format!("Without any proxy: {bare} The PC's way: {direct} Through the proxy (VPN): {e}")),
            None => Err(format!("Without any proxy: {bare} The PC's way: {direct}")),
        }
    }

    /// Runs `fetch` straight out; when that fails, once more through the proxy (when one is set), and when that
    /// fails too, with no proxy at all.
    pub async fn try_both<T, F, Fut>(&self, fetch: F) -> Result<T, String>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        // Any failure straight out goes through the proxy (a site may refuse this address); no proxy at all only
        // when the proxy itself could not get there.
        self.try_both_if(fetch, |_| true, unreachable).await
    }

    /// Like try_both, with the choice of when to go on: `to_proxy` decides on the error straight out, `to_bare` on
    /// the proxy's own error (a site that answered "no" says the same on every route).
    pub async fn try_both_if<T, F, Fut>(&self, fetch: F, to_proxy: impl Fn(&str) -> bool, to_bare: impl Fn(&str) -> bool) -> Result<T, String>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        let direct = match fetch(self.direct.clone()).await {
            Ok(value) => return Ok(value),
            Err(e) => e,
        };
        if !to_proxy(&direct) {
            return Err(direct);
        }
        let (first, last) = match self.proxied() {
            Some(proxied) => match fetch(proxied).await {
                Ok(value) => return Ok(value),
                Err(e) => (format!("{direct} Through the proxy (VPN) too: {e}"), e),
            },
            None => (direct.clone(), direct),
        };
        // Judged on the last route's own error: the joined text always says "proxy".
        if !to_bare(&last) {
            return Err(first);
        }
        fetch(self.bare.clone()).await.map_err(|e| format!("{first} Without any proxy too: {e}"))
    }
}

/// Iranian sites answer fastest without the VPN, and some refuse foreign addresses (11-09: tgju.org 5 s with no
/// proxy, 11 s through the VPN; the assistant's read took 45 s going through the Windows proxy first).
pub fn iranian_site(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#', ':']).next().unwrap_or_default().to_lowercase();
    const SITES: &[&str] = &["tgju.org", "torob.com", "digikala.com", "aparat.com", "varzesh3.com", "namava.ir", "filimo.com", "bama.ir"];
    host.ends_with(".ir") || host == "ir" || SITES.iter().any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// The failure was getting out at all (no connection), not an answer the site gave.
fn unreachable(error: &str) -> bool {
    let lower = error.to_lowercase();
    ["error sending request", "connect", "proxy", "timed out", "dns"].iter().any(|w| lower.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_site_that_refuses_this_address_is_tried_through_the_proxy() {
        let net = Net::new();
        net.set_proxy(Some("http://127.0.0.1:9")).unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<(), String> = net
            .try_both(|_client| {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move { Err(if n == 0 { "HTTP 403 from example.com".to_string() } else { "HTTP 403 from example.com again".to_string() }) }
            })
            .await;
        assert!(result.is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2, "straight out, then through the proxy; a refusal is not 'unreachable', so no third try");
    }

    #[test]
    fn iranian_sites_are_known() {
        assert!(iranian_site("https://www.tgju.org/") && iranian_site("https://divar.ir/s/iran?q=x") && iranian_site("https://www.khabaronline.ir"));
        assert!(!iranian_site("https://www.reuters.com/") && !iranian_site("https://generalagents.com/ace/") && !iranian_site("https://irna.com.example.org"));
    }

    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn a_fetch_that_cannot_get_out_is_tried_without_any_proxy() {
        let net = Net::new();
        let calls = AtomicU32::new(0);
        let got = net
            .try_both(|_| {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                async move { if call == 1 { Err::<u32, String>("error sending request for url (https://x)".into()) } else { Ok(call) } }
            })
            .await;
        assert_eq!(got, Ok(2));
    }

    #[tokio::test]
    async fn an_answer_from_the_site_is_not_fetched_again_without_proxy() {
        let net = Net::new();
        let calls = AtomicU32::new(0);
        let got = net
            .try_both(|_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err::<u32, String>("No place called \"Xyz\" was found.".into()) }
            })
            .await;
        assert!(got.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
