//! `Config::verifier` decides every connection a check makes: the DNS-over-HTTPS lookup, the ECH
//! handshake and the ordinary ones. Needs the network: `cargo test --features ech -- --ignored`.
#![cfg(feature = "ech")]

use std::sync::{Arc, Mutex};

use signed_update::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use signed_update::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use signed_update::rustls::{self, DigitallySignedStruct, SignatureScheme};
use signed_update::{Config, Privacy, Source, check};

/// Refuses every server, and remembers the names it was asked about.
#[derive(Debug, Default)]
struct RefuseAll {
    asked: Mutex<Vec<String>>,
}

impl ServerCertVerifier for RefuseAll {
    fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], name: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        self.asked.lock().unwrap().push(name.to_str().into_owned());
        Err(rustls::Error::General("refused by the test".into()))
    }

    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

fn config() -> Config {
    Config::new("Apparition", "apparition-release", "")
        .source(Source::apps("apparition"))
        .source(Source::github("illixion/Apparition"))
}

#[tokio::test]
#[ignore = "needs the network"]
async fn live_the_apps_verifier_is_asked_about_every_server() {
    let refuse = Arc::new(RefuseAll::default());
    let config = config().privacy(Privacy::Ech).verifier(refuse.clone());
    assert!(check(&config, "0.0.1", "Apparition-{version}.zip").await.is_err(), "every source was refused");
    let asked = refuse.asked.lock().unwrap().clone();
    assert!(asked.iter().any(|name| name == "1.1.1.1"), "the DNS-over-HTTPS resolver: {asked:?}");
    assert!(asked.iter().any(|name| name == "github.com"), "GitHub: {asked:?}");
}

/// The control: without a verifier of its own, the operating system's trust store reaches GitHub.
#[tokio::test]
#[ignore = "needs the network"]
async fn live_the_systems_trust_reaches_github() {
    let config = Config::new("Apparition", "apparition-release", "").source(Source::github("illixion/Apparition"));
    let error = check(&config, "0.0.1", "Apparition-{version}.zip").await.err().map(|e| e.to_string()).unwrap_or_default();
    // No signers, so nothing verifies: getting as far as the signature means the manifest came.
    assert!(error.contains("isn't a pinned release signer"), "{error}");
}
