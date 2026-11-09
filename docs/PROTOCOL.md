# The signed-update protocol

A release is a set of files and one `SHA256SUMS` listing them, signed with `ssh-keygen -Y sign -n
<product>-release` by a key the app has compiled in. That signature is the only thing an app
believes. Everything else (which host, which redirect, which mirror) is a hint, so hosting can move
without a new release and without trusting any host.

## What the app does

1. For each source in order, fetch `<latest>/SHA256SUMS` and `<latest>/SHA256SUMS.sig`
   (`Source::apps`, then `Source::github`).
2. Verify the signature (namespace, pinned key), then that every listed file is `<Product>-…`.
3. Find this platform's file by its signed name (`Vergence-{version}-macos-universal.zip`). The
   version is read from that name, so it is signed too: a host can't relabel an old release as a
   new one. No such file in the release: no update.
4. Not newer than the running version: no update (so a stale or replayed manifest can't downgrade).
   Versions compare numerically (`0.10.0` > `0.9.2`); a release is newer than its own pre-release
   (`0.3.0` > `0.3.0-beta.1`), and pre-releases of one version aren't ordered.
5. Download the file from each source's copy in turn (`<files>/<name>`, `{version}` filled in), keep it
   only if its SHA-256 is the signed one, then install it.

The first source whose manifest verifies decides; a source that is down or lying costs a log line and
the next source. A stale source can hold an app on any genuine release newer than the one it runs
(including one later found to be bad) until it is fixed, but never offer a downgrade or a forgery.
Each manifest or signature request gets 30 seconds in all, redirects and body included, and a
download is dropped for the next copy when it moves less than 64 KiB a minute, so a host that
trickles bytes can't hold a check or a download forever.

## Where things are

| | `latest` (manifest, signature) | `files` |
|---|---|---|
| GitHub | `https://github.com/<repo>/releases/latest/download/` | `https://github.com/<repo>/releases/download/v<version>/` |
| apps.illixion.com | `https://apps.illixion.com/<app>/latest/` | `https://apps.illixion.com/<app>/releases/<version>/` |

GitHub's `releases/latest` redirects to the newest *published, non-prerelease* release: a draft isn't
offered to anyone, which is what makes a draft the place to build and bless a release in.

**apps.illixion.com holds no files.** It is Cloudflare static hosting with a redirect per app
(`Website/_redirects`), so it is the front door and GitHub is what it points at:

```
/vergence/latest/*             https://github.com/illixion/vergence/releases/latest/download/:splat          302
/vergence/releases/:version/*  https://github.com/illixion/vergence/releases/download/v:version/:splat       302
```

If GitHub goes down, move every client by editing those two lines to a self-hosted copy laid out the
same way (`<host>/latest/SHA256SUMS`, `<host>/releases/<version>/<file>`), and push. The apps need no
update: they already ask the front door first. If the front door is down, the apps go to GitHub directly.

## Shipping a release

1. Bump the version, push a `v<version>` tag. CI builds the files and creates a **draft** release.
2. `scripts/bless-release.sh --repo OWNER/REPO --namespace <product>-release --signers release-signers
   --publish`, in the foreground (the hardware key wants a PIN and a touch). It hashes the draft's
   files, signs, checks the signature against the signers file, uploads `SHA256SUMS` and
   `SHA256SUMS.sig` and publishes. `--key-file` signs with the offline backup instead. Before it signs
   it checks each file's build attestation, and with `--product <Product>` that the tag is
   `v<version>` and every file is `<Product>-<version>…`, which is what the apps require after it.
3. Apps find it at their next check.

## Privacy: not showing apps.illixion.com to a network

`Config::privacy(Privacy::Ech)` (feature `ech`) reaches a *concealed* source, `Source::apps`, only
over DNS-over-HTTPS and Encrypted Client Hello, so on a hotel or office network a log shows neither
the lookup nor the name. github.com stays ordinary: it identifies no one on its own.

1. The HTTPS record of `apps.illixion.com` (its ECH config and address hints) is read from
   `https://1.1.1.1/dns-query`, then `1.0.0.1`, **by IP**: reaching a resolver by IP sends no name,
   and the system's DNS is never asked. Cloudflare is already the CDN, so no new party learns it.
2. The connection goes to those addresses with ECH enforced (rustls fails a handshake the server
   doesn't accept instead of carrying on in the clear). The network sees a handshake naming
   Cloudflare's shared `cloudflare-ech.com`.
3. The redirect to GitHub is followed by the ordinary client: each hop picks its own connection.
4. If the resolver, the ECH config or the handshake is blocked, the source is **skipped, never reached
   in the clear**, and the next source (GitHub directly) is asked. Without the `ech` feature a
   concealed source is skipped the same way.
5. A redirect back to the concealed host, from any hop, is held to the same rule however it spells
   the host (case, a trailing dot, another port): only https, only privately. The private client
   ignores proxies from the environment (a proxy is told the name in its `CONNECT`), and so does the
   DNS-over-HTTPS client.

What a log still shows: a connection to 1.1.1.1 (no name), a TLS handshake naming `cloudflare-ech.com`,
and github.com. Tested on the wire (`cargo test --features ech -- --ignored`): a proxy between the client
and Cloudflare captures the first flight of the handshake, which contains `cloudflare-ech.com` and not
`illixion`, while the same proxy shows an ordinary client's `apps.illixion.com`.

`ech` compiles rustls's aws-lc-rs provider (C code; ring has no HPKE), so each shipped target has to be
built and tried.

## File names

`<Product>-<version>-<platform…>`: `Vergence-0.2.0-macos-universal.zip`. The version sits between
dashes (or before a dot), because the signed names are what bind the version. One release has one
version: a manifest naming two is refused.

## Keys

Two keys pinned from the first release that ships a `release-signers` file: the everyday hardware key
and an offline backup. A backup added later would be worthless, since nothing installed would trust
it. The namespace is per product, so a signature made for one app can't pass as another's.
Each app compiles in its own `release-signers` (ssh-keygen's allowed_signers format), one
`principal ssh-ed25519 <key>` per line. A line with options (`namespaces=`, `valid-before=`,
`cert-authority`) pins nothing: those limits aren't implemented, and ignoring them would trust the
key more widely than the file says.

## What this does not protect against

A host that withholds updates (it can keep serving an old genuine release, or none), and a stolen
signing key. Code signing and notarization are separate and not done: the downloaded file is trusted
because it is the signed one, not because the OS vouches for it.
