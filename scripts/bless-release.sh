#!/usr/bin/env bash
#
# Signs a release with a pinned release key, for apps that update with the signed-update crate.
# One signature covers the whole release: every file on it is hashed into one SHA256SUMS, that is
# signed (SSHSIG, `ssh-keygen -Y sign`), and the manifest and its signature are attached back.
# An app accepts a release only when that signature checks out against the keys compiled into it.
#
#   bless-release.sh --repo OWNER/REPO --namespace NS --signers FILE [--product NAME] [--tag TAG]
#                    [--publish] [--no-attestations] [--signer X | --key-file F] [--gpg-agent]
#                    [--dry-run]
#   bless-release.sh --namespace NS --signers FILE --manifest SHA256SUMS [key options]
#
# Release mode reads the GitHub release TAG (the newest one, drafts included, when --tag is left
# out), downloads its files, writes SHA256SUMS and SHA256SUMS.sig, checks the result with
# ssh-keygen's own verifier against the signers file, and uploads both to the release. --publish
# then publishes a draft: GitHub's `releases/latest` (which apps.illixion.com redirects to, and the
# updater asks for) only ever names a published release, so a draft isn't offered to anyone yet.
# CI builds the files into a draft release; this is the step that needs you and your key.
#
# Before signing, release mode refuses what every app would refuse after it: with --product NAME
# (the app's Config::new product, `Vergence`), a tag other than v<version>, or a file that isn't
# NAME-…, or doesn't carry that version (`Vergence-0.2.0-macos-universal.zip`). A stray upload then
# stops here instead of publishing a release no app accepts. And each file's build provenance is
# checked (`gh attestation verify`): this is the human gate over whatever CI, or anyone holding a
# token, put on the draft. --no-attestations skips that, for a release built by hand.
#
# Manifest mode signs a SHA256SUMS you already have, writing SHA256SUMS.sig beside it.
#
# NS is the product's SSHSIG namespace (`vergence-release`), not "file", so a signature made for
# one app can't pass as another's. FILE is the app's `release-signers` (ssh-keygen's allowed_signers
# format), the same file it compiles in.
#
# THE KEYS. The first signer is the everyday hardware key (a PIN and a touch, never on disk or in
# CI), so run this in the foreground: a backgrounded signer can't raise the prompt and fails as a
# timeout. --signer picks another by comment or position; --key-file signs with the offline backup
# (a private key file, no agent); --gpg-agent looks for the key in gpg-agent's SSH socket. Nothing
# here reads a private key except the one you name with --key-file.
#
# Adapted from Longwave's and Apparition's release signing scripts (MIT, same author).

set -euo pipefail

REPO=""
NAMESPACE=""
SIGNERS=""
TAG=""
MANIFEST_FILE=""
PRODUCT=""
ATTESTATIONS=1
PUBLISH=0
DRY_RUN=0
SIGNER=""
KEY_FILE=""
USE_GPG_AGENT=0
MANIFEST=SHA256SUMS
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)      REPO="${2:?--repo needs OWNER/REPO}"; shift ;;
    --namespace) NAMESPACE="${2:?--namespace needs a value}"; shift ;;
    --signers)   SIGNERS="${2:?--signers needs a file}"; shift ;;
    --tag)       TAG="${2:?--tag needs a value}"; shift ;;
    --manifest)  MANIFEST_FILE="${2:?--manifest needs a file}"; shift ;;
    --product)   PRODUCT="${2:?--product needs a name}"; shift ;;
    --no-attestations) ATTESTATIONS=0 ;;
    --publish)   PUBLISH=1 ;;
    --signer)    SIGNER="${2:?--signer needs a value}"; shift ;;
    --key-file)  KEY_FILE="${2:?--key-file needs a value}"; shift ;;
    --gpg-agent) USE_GPG_AGENT=1 ;;
    --dry-run)   DRY_RUN=1 ;;
    -h|--help)   sed -n '3,46p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

[[ -n "$NAMESPACE" ]] || { echo "error: --namespace is required" >&2; exit 2; }
[[ -n "$SIGNERS" && -f "$SIGNERS" ]] || { echo "error: --signers needs the app's release-signers file" >&2; exit 2; }
[[ -n "$REPO" || -n "$MANIFEST_FILE" ]] || { echo "error: give --repo (a GitHub release) or --manifest (a local SHA256SUMS)" >&2; exit 2; }

STAGE="$(mktemp -d -t signed-update-bless)"
trap 'rm -rf -- "$STAGE"' EXIT

# A pinned signer line by comment substring or 1-based index; the first by default. Prints
# "algo blob comment".
pick_signer() {
  local want="$1" n=0 principal algo b64 comment
  while read -r principal algo b64 comment; do
    [[ -z "${algo:-}" || "$principal" == \#* ]] && continue
    # A line with options (namespaces=…) pins nothing in the apps either.
    [[ "$algo" == ssh-* || "$algo" == sk-* ]] || continue
    n=$((n + 1))
    if [[ -z "$want" && "$n" == 1 ]] || [[ "$want" == "$n" ]] \
       || { [[ -n "$want" ]] && [[ "$comment" == *"$want"* ]]; }; then
      printf '%s %s %s\n' "$algo" "$b64" "$comment"
      return 0
    fi
  done < "$SIGNERS"
  return 1
}

# The principal of the first signer, which ssh-keygen -Y verify wants as the identity.
identity() {
  local principal rest
  while read -r principal rest; do
    [[ -z "${rest:-}" || "$principal" == \#* ]] && continue
    echo "$principal"
    return 0
  done < "$SIGNERS"
  return 1
}

agent_has_key() { SSH_AUTH_SOCK="$1" ssh-add -L 2>/dev/null | grep -qF "$2"; }

# Signs $1, leaving $1.sig, then checks it. `ssh-keygen -Y sign` leaves an existing .sig alone and
# still exits 0, so a stale signature would survive silently: it is removed first.
sign_and_check() {
  local file="$1" sigfile="$1.sig" line algo blob name gpgsock pub
  rm -f -- "$sigfile"
  if [[ -n "$KEY_FILE" ]]; then
    [[ -f "$KEY_FILE" ]] || { echo "error: key file not found: $KEY_FILE" >&2; exit 1; }
    echo "==> signing with the key file $KEY_FILE"
    ssh-keygen -Y sign -n "$NAMESPACE" -f "$KEY_FILE" "$file"
  else
    line="$(pick_signer "$SIGNER")" \
      || { echo "error: no pinned signer matches '${SIGNER:-<first>}' in $SIGNERS" >&2; exit 1; }
    read -r algo blob name <<<"$line"
    echo "==> signing as $name (watch for a PIN/touch prompt)"
    # An agent that holds this key: --gpg-agent, else SSH_AUTH_SOCK, else gpg-agent's SSH socket
    # (where a hardware key's OpenPGP key shows up). Checked, because signing through an agent
    # without the key fails with an error that reads like a bad key file.
    if [[ "$USE_GPG_AGENT" -eq 1 ]]; then
      SSH_AUTH_SOCK="$(gpgconf --list-dirs agent-ssh-socket)"; export SSH_AUTH_SOCK
      gpgconf --launch gpg-agent || true
    elif ! agent_has_key "${SSH_AUTH_SOCK:-}" "$blob"; then
      gpgsock="$(gpgconf --list-dirs agent-ssh-socket 2>/dev/null || true)"
      if [[ -n "$gpgsock" ]] && { gpgconf --launch gpg-agent 2>/dev/null; agent_has_key "$gpgsock" "$blob"; }; then
        SSH_AUTH_SOCK="$gpgsock"; export SSH_AUTH_SOCK
      fi
    fi
    agent_has_key "${SSH_AUTH_SOCK:-}" "$blob" || {
      echo "error: no reachable ssh-agent holds $name." >&2
      echo "       Plug in the hardware key, try --gpg-agent, or sign with the offline key: --key-file." >&2
      exit 1
    }
    # -f takes the PUBLIC key; ssh-keygen asks the agent to sign with its private half.
    pub="$STAGE/signer.pub"
    printf '%s %s %s\n' "$algo" "$blob" "$name" > "$pub"
    ssh-keygen -Y sign -n "$NAMESPACE" -f "$pub" "$file"
  fi
  [[ -f "$sigfile" ]] || { echo "error: ssh-keygen made no signature" >&2; exit 1; }
  # Catches signing with a key that isn't pinned: a valid signature every client refuses, which
  # only shows up on someone else's machine.
  echo "==> checking it against $SIGNERS"
  ssh-keygen -Y verify -f "$SIGNERS" -I "$(identity)" -n "$NAMESPACE" -s "$sigfile" < "$file" | sed 's/^/    /'
}

if [[ -n "$MANIFEST_FILE" ]]; then
  [[ -f "$MANIFEST_FILE" ]] || { echo "error: no manifest at $MANIFEST_FILE" >&2; exit 1; }
  sign_and_check "$MANIFEST_FILE"
  exit 0
fi

# ------------------------------------------------------------------ release mode
if [[ -z "$TAG" ]]; then
  # --json tagName of the newest release, drafts included (a draft is the point of this step).
  TAG="$(gh release list -R "$REPO" --limit 1 --json tagName -q '.[0].tagName')"
  [[ -n "$TAG" ]] || { echo "error: $REPO has no releases and no --tag was given" >&2; exit 1; }
  echo "==> no --tag given, using the newest release: $TAG"
fi

echo "==> reading $REPO $TAG"
ASSET_NAMES=()
while IFS= read -r line; do
  [[ -n "$line" ]] && ASSET_NAMES+=("$line")
done < <(gh release view "$TAG" -R "$REPO" --json assets -q '.assets[].name' | sort)

# The manifest and its signature are this script's output, never its input.
COVERED=()
for name in "${ASSET_NAMES[@]+"${ASSET_NAMES[@]}"}"; do
  case "$name" in
    "$MANIFEST"|"$MANIFEST.sig") continue ;;
    *) COVERED+=("$name") ;;
  esac
done
[[ ${#COVERED[@]} -gt 0 ]] || { echo "error: $TAG has no files to sign" >&2; exit 1; }

# What the apps check after the signature, checked before it (the version is bound to the file
# names, and the download URLs are built from it as v<version>).
if [[ -n "$PRODUCT" ]]; then
  VERSION="${TAG#v}"
  [[ "$TAG" == "v$VERSION" && "$VERSION" =~ ^[0-9]+(\.[0-9]+)+$ ]] \
    || { echo "error: the tag $TAG isn't v<version>; apps download release files from v<version>" >&2; exit 1; }
  bad=0
  for name in "${COVERED[@]}"; do
    if [[ "$name" != "$PRODUCT-"* ]] || [[ "$name" != *"-$VERSION-"* && "$name" != *"-$VERSION."* ]]; then
      echo "error: $name isn't a $PRODUCT $VERSION file (apps refuse a manifest that lists it); remove it from $TAG" >&2
      bad=1
    fi
  done
  [[ "$bad" == 0 ]] || exit 1
fi

# Blessed before? Say what moved: "3 new files" is the difference between a late arrival and
# someone else having uploaded something.
if printf '%s\n' "${ASSET_NAMES[@]}" | grep -qx "$MANIFEST"; then
  if gh release download "$TAG" -R "$REPO" --pattern "$MANIFEST" --dir "$STAGE" --clobber >/dev/null 2>&1; then
    mv "$STAGE/$MANIFEST" "$STAGE/previous"
    echo "==> $TAG is already blessed; comparing"
    covered_sorted="$(printf '%s\n' "${COVERED[@]}" | sort)"
    previous_names="$(awk '{ $1=""; sub(/^ +\**/, ""); print }' "$STAGE/previous" | sort)"
    comm -23 <(echo "$covered_sorted") <(echo "$previous_names") | sed 's/^/    + (new since the last blessing) /'
    comm -13 <(echo "$covered_sorted") <(echo "$previous_names") | sed 's/^/    - (no longer on the release) /'
  fi
fi

echo "==> downloading ${#COVERED[@]} file(s)"
mkdir -p "$STAGE/files"
for name in "${COVERED[@]}"; do
  gh release download "$TAG" -R "$REPO" --pattern "$name" --dir "$STAGE/files" --clobber
done

if [[ "$ATTESTATIONS" == 1 ]]; then
  echo "==> checking each file's build provenance"
  for name in "${COVERED[@]}"; do
    gh attestation verify "$STAGE/files/$name" -R "$REPO" >/dev/null \
      || { echo "error: $name has no valid attestation from $REPO's workflows (--no-attestations for a hand-built release)" >&2; exit 1; }
    echo "    $name"
  done
fi

# Hashed from inside the folder so the manifest carries bare names, which is what the app matches
# its file against; sorted for a stable, reviewable diff between blessings.
echo "==> hashing"
( cd "$STAGE/files" && shasum -a 256 -- * | sort -k2 ) > "$STAGE/$MANIFEST"
sed 's/^/    /' "$STAGE/$MANIFEST"
HASHED="$(grep -c '' "$STAGE/$MANIFEST")"
[[ "$HASHED" -eq "${#COVERED[@]}" ]] \
  || { echo "error: hashed $HASHED file(s) but the release has ${#COVERED[@]}: refusing to sign a partial manifest" >&2; exit 1; }

sign_and_check "$STAGE/$MANIFEST"

if [[ "$DRY_RUN" == 1 ]]; then
  OUT="$PWD/.release-manifest-out"
  mkdir -p "$OUT"
  cp "$STAGE/$MANIFEST" "$STAGE/$MANIFEST.sig" "$OUT/"
  echo "==> --dry-run: nothing uploaded. Manifest and signature are in $OUT/"
  exit 0
fi

echo "==> uploading $MANIFEST and $MANIFEST.sig to $TAG"
gh release upload "$TAG" -R "$REPO" "$STAGE/$MANIFEST" "$STAGE/$MANIFEST.sig" --clobber

if [[ "$PUBLISH" == 1 ]]; then
  echo "==> publishing $TAG"
  gh release edit "$TAG" -R "$REPO" --draft=false --latest
else
  echo "    (a draft stays unlisted until you publish it: re-run with --publish, or gh release edit $TAG -R $REPO --draft=false --latest)"
fi

echo
echo "==> $TAG is blessed: ${#COVERED[@]} file(s) under one signature."
