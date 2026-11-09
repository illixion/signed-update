//! What `Privacy::Ech` promises: a concealed source is reached privately or not at all.
#![cfg(feature = "http")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use signed_update::{Config, Privacy, Source, check};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SUMS: &str = "SHA256SUMS";

/// A server that counts every connection it gets, whatever the path, and answers each request with
/// `reply` (a 404 by default). `host` names it in the returned base URL: the concealed server is
/// `localhost`, the other `127.0.0.1`, two hosts as far as the concealed list is concerned.
async fn counting_server(host: &str, reply: Option<String>) -> (String, u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://{host}:{port}");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let reply = reply.unwrap_or_else(|| "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into());
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            let reply = reply.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    (base, port, hits)
}

fn redirect_to(location: &str) -> Option<String> {
    Some(format!("HTTP/1.1 302 Found\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"))
}

fn config(concealed: &str, other: &str, privacy: Privacy) -> Config {
    Config::new("Prod", "prod-release", "test@example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJ8qs+pOpJxYWyxifDvk2+iCeK08uT/YjckUSHXtTrge")
        .source(Source::mirror(concealed, "prod").concealed())
        .source(Source::mirror(other, "prod"))
        .privacy(privacy)
        // Nothing listens here: the private route is unavailable.
        .doh(vec!["http://127.0.0.1:1/dns-query".into()])
}

#[tokio::test]
async fn a_concealed_source_is_never_reached_in_the_clear() {
    let (concealed, _, concealed_hits) = counting_server("localhost", None).await;
    let (other, _, other_hits) = counting_server("127.0.0.1", None).await;
    let error = check(&config(&concealed, &other, Privacy::Ech), "1.0.0", "Prod-{version}-macos.zip").await.unwrap_err().to_string();
    assert_eq!(concealed_hits.load(Ordering::SeqCst), 0, "the concealed host received a connection: {error}");
    assert!(other_hits.load(Ordering::SeqCst) > 0, "the next source is asked instead");
    assert!(error.contains(&concealed), "the failure names the skipped source: {error}");
    assert!(error.contains("privately") || error.contains("DNS-over-HTTPS"), "and says why: {error}");
}

#[tokio::test]
async fn without_privacy_a_concealed_source_is_an_ordinary_one() {
    let (concealed, _, concealed_hits) = counting_server("localhost", None).await;
    let (other, _, _) = counting_server("127.0.0.1", None).await;
    let _ = check(&config(&concealed, &other, Privacy::Off), "1.0.0", "Prod-{version}-macos.zip").await;
    assert!(concealed_hits.load(Ordering::SeqCst) > 0);
}

/// Another source (or the front door itself) redirecting to the concealed host spelled another
/// way, on another port or over plain http doesn't get it reached in the clear.
#[tokio::test]
async fn a_redirect_to_the_concealed_host_in_any_spelling_is_not_followed_in_the_clear() {
    let (concealed, port, concealed_hits) = counting_server("localhost", None).await;
    for location in [
        format!("http://LOCALHOST.:{port}/prod/latest/{SUMS}"),
        format!("http://localhost:{port}/prod/latest/{SUMS}"),
        format!("http://localhost:{}/prod/latest/{SUMS}", port.wrapping_add(1)),
    ] {
        let (other, _, other_hits) = counting_server("127.0.0.1", redirect_to(&location)).await;
        let error = check(&config(&concealed, &other, Privacy::Ech), "1.0.0", "Prod-{version}-macos.zip").await.unwrap_err().to_string();
        assert!(other_hits.load(Ordering::SeqCst) > 0, "{location}: the other source was asked");
        assert_eq!(concealed_hits.load(Ordering::SeqCst), 0, "{location} reached the concealed host: {error}");
    }
}
