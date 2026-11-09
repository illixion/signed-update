//! What `Privacy::Ech` promises: a concealed source is reached privately or not at all.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use signed_update::{Config, Privacy, Source, check};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SUMS: &str = "SHA256SUMS";

/// A server that counts every request it gets, whatever the path. Replies 404.
async fn counting_server() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            });
        }
    });
    (base, hits)
}

fn config(concealed: &str, other: &str, privacy: Privacy) -> Config {
    Config::new("Prod", "prod-release", "test@example ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPDFLEggFzedj9nvz0wl9AoPGXFFbT9XfgEWrAY0l7pa")
        .source(Source::mirror(concealed, "prod").concealed())
        .source(Source::mirror(other, "prod"))
        .privacy(privacy)
        // Nothing listens here: the private route is unavailable.
        .doh(vec!["http://127.0.0.1:1/dns-query".into()])
}

#[tokio::test]
async fn a_concealed_source_is_never_reached_in_the_clear() {
    let (concealed, concealed_hits) = counting_server().await;
    let (other, other_hits) = counting_server().await;
    let error = check(&config(&concealed, &other, Privacy::Ech), "1.0.0", "Prod-{version}-macos.zip").await.unwrap_err().to_string();
    assert_eq!(concealed_hits.load(Ordering::SeqCst), 0, "the concealed host received a connection: {error}");
    assert!(other_hits.load(Ordering::SeqCst) > 0, "the next source is asked instead");
    assert!(error.contains(&concealed), "the failure names the skipped source: {error}");
    assert!(error.contains("privately") || error.contains("DNS-over-HTTPS"), "and says why: {error}");
}

#[tokio::test]
async fn without_privacy_a_concealed_source_is_an_ordinary_one() {
    let (concealed, concealed_hits) = counting_server().await;
    let (other, _) = counting_server().await;
    let _ = check(&config(&concealed, &other, Privacy::Off), "1.0.0", "Prod-{version}-macos.zip").await;
    assert!(concealed_hits.load(Ordering::SeqCst) > 0);
    let _ = SUMS;
}
