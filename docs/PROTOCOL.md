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
5. Download the file from each source's copy in turn (`<files>/<name>`, `{version}` filled in), keep it
   only if its SHA-256 is the signed one, then install it.

The first source whose manifest verifies decides; a source that is down, stale or lying costs a log
line and the next source. A stale source can only make an update appear later.

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
   `SHA256SUMS.sig` and publishes. `--key-file` signs with the offline backup instead.
3. Apps find it at their next check.

## File names

`<Product>-<version>-<platform…>`: `Vergence-0.2.0-macos-universal.zip`. The version sits between
dashes (or before a dot), because the signed names are what bind the version. One release has one
version: a manifest naming two is refused.

## Keys

Two keys pinned from the first release that ships a `release-signers` file: the everyday hardware key
and an offline backup. A backup added later would be worthless, since nothing installed would trust
it. The namespace is per product, so a signature made for one app can't pass as another's.
Each app compiles in its own `release-signers` (ssh-keygen's allowed_signers format).

## What this does not protect against

A host that withholds updates (it can keep serving an old genuine release, or none), and a stolen
signing key. Code signing and notarization are separate and not done: the downloaded file is trusted
because it is the signed one, not because the OS vouches for it.
