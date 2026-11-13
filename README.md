# signed-update

Self-updates for apps that ship outside a store (Tauri desktop apps, mostly), verified against SSH
release keys pinned in the app. One crate, one release flow, shared by Apparition, Vergence and
whatever comes next. MIT.

- **Verify:** SSHSIG (ed25519) over a release's `SHA256SUMS`; hashes checked before anything is used.
- **Find:** asks apps.illixion.com (redirects only, no files) then GitHub Releases for the newest
  signed manifest; the first that verifies wins. Hosting can move (a self-hosted copy while GitHub is
  down) by editing a redirect, with no new release and no app update.
- **Hide:** `Privacy::Ech` (feature `ech`) reaches apps.illixion.com only over DNS-over-HTTPS and
  Encrypted Client Hello, so a network log doesn't show the name, and skips it (never goes plain) when
  that is blocked.
- **Connect:** the operating system's trust store for every request, no compiled-in roots, or the
  app's own verifier (`Config::verifier`). TLS decides only who can see or block a check, never what
  is installed.
- **Install:** the macOS bundle swap (`install::install_bundle`: unzip beside the app, check the
  code-signing seal, `renamex_np` swap). The app keeps its own UI and scheduling.
- **Sign:** `scripts/bless-release.sh` signs a GitHub draft release with your key and publishes it.

```rust
let config = Config::new("Vergence", "vergence-release", include_str!("../release-signers"))
    .source(Source::apps("vergence"))
    .source(Source::github("illixion/vergence"));
if let Some(update) = check(&config, env!("CARGO_PKG_VERSION"), "Vergence-{version}-macos-universal.zip").await? {
    let package = update.download(&dir, |bytes| { /* progress */ }).await?;
    signed_update::install::install_bundle(&package, &bundle)?;
}
```

How it works and why: [docs/PROTOCOL.md](docs/PROTOCOL.md). `cargo test` runs the whole protocol
against a local server that redirects, 404s and lies (needs `ssh-keygen`, which signs the test
manifests; `--features ech -- --ignored` adds live tests against Cloudflare and GitHub, including a wire
capture of the handshake and a verifier that is asked about every server). `--no-default-features` is the verification half without an HTTP client.
