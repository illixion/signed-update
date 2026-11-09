//! The update protocol end to end against a local server that redirects, 404s and lies, with
//! manifests signed by a real `ssh-keygen` (the tests skip where there is none).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use signed_update::{Config, Source, check};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const NAMESPACE: &str = "prod-release";
const PATTERN: &str = "Prod-{version}-macos.zip";

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    Redirect(String),
}

type Routes = Arc<Mutex<HashMap<String, Reply>>>;

/// A server on a free port; `routes` can be edited while it runs. Unknown paths are 404.
async fn serve() -> (String, Routes) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let routes: Routes = Arc::default();
    let shared = routes.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let routes = shared.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 4096];
                let n = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let reply = routes.lock().unwrap().get(&path).cloned();
                let response = match reply {
                    Some(Reply::Body(body)) => {
                        let mut out = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).into_bytes();
                        out.extend(body);
                        out
                    }
                    Some(Reply::Redirect(to)) => format!("HTTP/1.1 302 Found\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").into_bytes(),
                    None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (base, routes)
}

struct Signer {
    dir: tempfile::TempDir,
}

impl Signer {
    /// A throwaway key; None without ssh-keygen.
    fn new() -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        let made = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"]).arg(dir.path().join("key")).status().ok()?;
        made.success().then_some(Self { dir })
    }

    fn signers(&self) -> String {
        format!("releases@test {}", std::fs::read_to_string(self.dir.path().join("key.pub")).unwrap())
    }

    fn sign(&self, data: &[u8], namespace: &str) -> String {
        let file = self.dir.path().join("SHA256SUMS");
        std::fs::write(&file, data).unwrap();
        let _ = std::fs::remove_file(self.dir.path().join("SHA256SUMS.sig"));
        let status = Command::new("ssh-keygen").args(["-Y", "sign", "-q", "-n", namespace, "-f"]).arg(self.dir.path().join("key")).arg(&file).status().unwrap();
        assert!(status.success());
        std::fs::read_to_string(self.dir.path().join("SHA256SUMS.sig")).unwrap()
    }
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// A release 1.2.0's manifest, signature and file, served under `<prefix>/latest/` and
/// `<prefix>/releases/1.2.0/` of `routes`.
fn publish(routes: &Routes, prefix: &str, signer: &Signer, version: &str, file: &[u8]) {
    let name = format!("Prod-{version}-macos.zip");
    let sums = format!("{}  {name}\n", sha(file));
    let signature = signer.sign(sums.as_bytes(), NAMESPACE);
    let mut routes = routes.lock().unwrap();
    routes.insert(format!("{prefix}/latest/SHA256SUMS"), Reply::Body(sums.into_bytes()));
    routes.insert(format!("{prefix}/latest/SHA256SUMS.sig"), Reply::Body(signature.into_bytes()));
    routes.insert(format!("{prefix}/releases/{version}/{name}"), Reply::Body(file.to_vec()));
}

fn config(signer: &Signer, bases: &[&str]) -> Config {
    let mut config = Config::new("Prod", NAMESPACE, &signer.signers());
    for base in bases {
        config = config.source(Source::mirror(base, "prod"));
    }
    config
}

macro_rules! signer_or_skip {
    () => {
        match Signer::new() {
            Some(signer) => signer,
            None => {
                eprintln!("no ssh-keygen; skipped");
                return;
            }
        }
    };
}

#[tokio::test]
async fn finds_verifies_and_downloads_an_update() {
    let signer = signer_or_skip!();
    let (base, routes) = serve().await;
    publish(&routes, "/prod", &signer, "1.2.0", b"the new app");
    let update = check(&config(&signer, &[&base]), "1.1.0", PATTERN).await.unwrap().expect("an update");
    assert_eq!((update.version.as_str(), update.name.as_str()), ("1.2.0", "Prod-1.2.0-macos.zip"));
    assert_eq!(update.signer, "releases@test");
    let dir = tempfile::tempdir().unwrap();
    let file = update.download(dir.path(), |_| {}).await.unwrap();
    assert_eq!(std::fs::read(file).unwrap(), b"the new app");
    assert!(!dir.path().join("Prod-1.2.0-macos.zip.partial").exists());
}

#[tokio::test]
async fn up_to_date_and_older_releases_are_no_update() {
    let signer = signer_or_skip!();
    let (base, routes) = serve().await;
    publish(&routes, "/prod", &signer, "1.2.0", b"x");
    let config = config(&signer, &[&base]);
    assert!(check(&config, "1.2.0", PATTERN).await.unwrap().is_none());
    assert!(check(&config, "1.3.0", PATTERN).await.unwrap().is_none(), "a genuine older release is not a downgrade");
    assert!(check(&config, "1.1.0", "Prod-{version}-windows.zip").await.unwrap().is_none(), "a release with no file for this platform");
}

/// apps.illixion.com's shape: nothing but redirects to wherever the release is.
#[tokio::test]
async fn follows_redirects_from_the_front_door() {
    let signer = signer_or_skip!();
    let (host, hosted) = serve().await;
    publish(&hosted, "/real", &signer, "1.2.0", b"hosted elsewhere");
    let (front, doors) = serve().await;
    {
        let mut doors = doors.lock().unwrap();
        for file in ["SHA256SUMS", "SHA256SUMS.sig"] {
            doors.insert(format!("/prod/latest/{file}"), Reply::Redirect(format!("{host}/real/latest/{file}")));
        }
        doors.insert("/prod/releases/1.2.0/Prod-1.2.0-macos.zip".into(), Reply::Redirect(format!("{host}/real/releases/1.2.0/Prod-1.2.0-macos.zip")));
    }
    let update = check(&config(&signer, &[&front]), "1.0.0", PATTERN).await.unwrap().unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(std::fs::read(update.download(dir.path(), |_| {}).await.unwrap()).unwrap(), b"hosted elsewhere");
}

#[tokio::test]
async fn a_source_that_is_down_costs_the_next_source() {
    let signer = signer_or_skip!();
    let (down, _) = serve().await; // serves nothing: every path 404s
    let (up, routes) = serve().await;
    publish(&routes, "/prod", &signer, "1.2.0", b"from the second");
    let update = check(&config(&signer, &[&down, &up]), "1.0.0", PATTERN).await.unwrap().unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(std::fs::read(update.download(dir.path(), |_| {}).await.unwrap()).unwrap(), b"from the second");
}

#[tokio::test]
async fn a_forged_manifest_is_refused_and_the_next_source_is_asked() {
    let signer = signer_or_skip!();
    let attacker = signer_or_skip!();
    let (evil, evil_routes) = serve().await;
    publish(&evil_routes, "/prod", &attacker, "9.9.9", b"malware");
    let (good, good_routes) = serve().await;
    publish(&good_routes, "/prod", &signer, "1.2.0", b"genuine");
    let update = check(&config(&signer, &[&evil, &good]), "1.0.0", PATTERN).await.unwrap().unwrap();
    assert_eq!(update.version, "1.2.0");
    // With only the forger there is no update, and the reason is said.
    let error = check(&config(&signer, &[&evil]), "1.0.0", PATTERN).await.unwrap_err().to_string();
    assert!(error.contains("pinned release signer"), "{error}");
}

#[tokio::test]
async fn a_manifest_signed_for_another_product_is_refused() {
    let signer = signer_or_skip!();
    let (base, routes) = serve().await;
    let sums = format!("{}  Prod-1.2.0-macos.zip\n", sha(b"x"));
    let signature = signer.sign(sums.as_bytes(), "other-release");
    routes.lock().unwrap().insert("/prod/latest/SHA256SUMS".into(), Reply::Body(sums.into_bytes()));
    routes.lock().unwrap().insert("/prod/latest/SHA256SUMS.sig".into(), Reply::Body(signature.into_bytes()));
    let error = check(&config(&signer, &[&base]), "1.0.0", PATTERN).await.unwrap_err().to_string();
    assert!(error.contains("not \"prod-release\""), "{error}");
}

#[tokio::test]
async fn a_tampered_download_is_refused_and_another_copy_used() {
    let signer = signer_or_skip!();
    let (bad, bad_routes) = serve().await;
    let (good, good_routes) = serve().await;
    publish(&good_routes, "/prod", &signer, "1.2.0", b"genuine");
    // The first source serves the genuine manifest but a swapped file.
    publish(&bad_routes, "/prod", &signer, "1.2.0", b"genuine");
    bad_routes.lock().unwrap().insert("/prod/releases/1.2.0/Prod-1.2.0-macos.zip".into(), Reply::Body(b"swapped".to_vec()));
    let update = check(&config(&signer, &[&bad, &good]), "1.0.0", PATTERN).await.unwrap().unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(std::fs::read(update.download(dir.path(), |_| {}).await.unwrap()).unwrap(), b"genuine");
    // Swapped everywhere: nothing is kept.
    good_routes.lock().unwrap().insert("/prod/releases/1.2.0/Prod-1.2.0-macos.zip".into(), Reply::Body(b"swapped".to_vec()));
    let dir = tempfile::tempdir().unwrap();
    let error = update.download(dir.path(), |_| {}).await.unwrap_err().to_string();
    assert!(error.contains("SHA-256"), "{error}");
    assert!(!Path::new(&dir.path().join("Prod-1.2.0-macos.zip")).exists());
    assert!(!dir.path().join("Prod-1.2.0-macos.zip.partial").exists());
}

#[tokio::test]
async fn no_sources_is_an_error() {
    let signer = signer_or_skip!();
    assert!(check(&config(&signer, &[]), "1.0.0", PATTERN).await.is_err());
}
