# Air-gapped deployment: run XERJ without runtime internet

This is a short, Linux x86_64-musl procedure. It stages one operator-approved
release, installs the service as an unprivileged account, and keeps the node on
loopback. The base path is lexical and needs no model files. Neural embedding is
an optional, separately staged path.

## Runtime boundary

- The default embedding mode is **lexical feature hashing**. It needs no model,
  no embedding service, and no network connection.
- Neural mode is optional. Without `embedding.local_model_dir`, the first
  operation that needs an embedding downloads `config.json`, `tokenizer.json`,
  and `model.safetensors` for `sentence-transformers/all-MiniLM-L6-v2` from the
  Hugging Face Hub. With that directory set, XERJ reads those files locally.
- `proxy` calls only the configured OpenAI-compatible
  `embedding.default_endpoint`. The experimental `onnx-experimental` backend
  needs an ONNX-enabled build and explicit local assets; neither is the base
  release path.

The running node makes no telemetry, update-check, or license-activation call.
Release-download analytics are a property of the connected website/GitHub
staging surface, not of a running XERJ process. The installer also downloads a
release archive and its checksum from GitHub; an offline operator must stage
those files before entering the enclave.

## Defaults and boundaries

| Component | Default | Air-gapped meaning |
|---|---|---|
| Embedding | `auto`: lexical unless an endpoint is configured | No model or network for lexical mode |
| Neural model | Off | Set `local_model_dir` before selecting `neural` |
| External proxy | Disabled (`default_endpoint = ""`) | Only the configured endpoint is contacted |
| ONNX | Experimental and not in the standard release | Build the feature and provide local assets explicitly |
| WAL tap | Disabled | It remains inert unless enabled with a target URL; see the durable overlay note below |
| Cluster | Disabled | Single-node startup does not initialize the Raft transport |
| REST / ES-compatible listeners | `127.0.0.1` | The default is loopback-only |
| `autoindex` and MCP clients | Localhost defaults | They connect to `localhost`; they do not create a public listener |
| Runtime telemetry/update/license activation | None | No outbound call is made by the running binary for these purposes |

This is a statement of defaults, not a claim that every browser UI request is
offline. **The bundled Console has three external Google Fonts link elements
(two preconnects and one stylesheet; stylesheet may fetch additional font files).**
Blocked requests fall back to system fonts. The engine and APIs
continue to work.

This procedure does not claim that egress was measured here. If your policy
requires that boundary, apply an egress-deny rule and inspect firewall/log
counters during the acceptance checks.

## 1. Stage an approved release on a connected machine

Do this on a machine that is allowed to reach GitHub. Set `TAG` to the
operator-approved `vX.Y.Z` release tag before running; there is no moving
release alias in this procedure. The release workflow names each archive from
the version without the `v` prefix, target triple, and extension. The matching
`.sha256` is a separate per-archive asset.

The following stages the Linux x86_64 musl archive only.

```bash
set -eu

: "${TAG:?set TAG to an operator-approved vX.Y.Z release tag}"
if [[ ! "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
  echo "TAG must match vX.Y.Z with optional prerelease/build metadata: $TAG" >&2
  exit 2
fi
VERSION="${TAG#v}"
TARGET="x86_64-unknown-linux-musl"
EXT="tar.gz"
STAGE="xerj-${VERSION}-${TARGET}"
ASSET="${STAGE}.${EXT}"
BASE="https://github.com/xerj-org/xerj/releases/download/${TAG}"
OUT="${OUT:-$PWD/xerj-airgap-${VERSION}-${TARGET}}"

mkdir -p "$OUT/release"
curl -fL --retry 3 -o "$OUT/release/$ASSET" "$BASE/$ASSET"
curl -fL --retry 3 -o "$OUT/release/$ASSET.sha256" "$BASE/$ASSET.sha256"

(
  cd "$OUT/release"
  sha256sum -c "$ASSET.sha256"
  tar -xzf "$ASSET"
  test -x "$STAGE/xerj"
  "$STAGE/xerj" --version
)
```

The checksum proves that the archive matches the digest published beside that
archive in the same GitHub release. It is an integrity check, not an
independent signature, provenance statement, or attestation. If your policy
requires signed releases or an external attestation, obtain and verify that
evidence separately; `.sha256` alone does not provide it.

After transfer, set the same approved `TAG` in the enclave and verify before
extracting:

```bash
: "${TAG:?set TAG to the same operator-approved release tag}"
VERSION="${TAG#v}"
STAGE="xerj-${VERSION}-x86_64-unknown-linux-musl"
ASSET="${STAGE}.tar.gz"

(
  set -eu
  cd /opt/xerj-staging/release

  rm -rf "$STAGE" "$STAGE.verified"

  want="$(sha256sum "$ASSET" | cut -d ' ' -f 1)"
  if [[ ! "$want" =~ ^[0-9a-f]{64}$ ]]; then
    echo "could not compute a digest for $ASSET" >&2
    exit 1
  fi
  LC_ALL=C grep -qxF -e "$want  $ASSET" -e "$want *$ASSET" \
    < <(tr -d '\r' < "$ASSET.sha256")

  tar -xzf "$ASSET"
  test -x "$STAGE/xerj"
  mv "$STAGE" "$STAGE.verified"
)
```

Three details here are load-bearing, and each replaces something that looked
right and was not.

**The digest is computed here and the file is searched for it, not the other way
round.** `sha256sum -c` reads the filename out of the `.sha256` — a file that
crossed the airgap on the same medium as the archive — and reports success for
whatever it happens to name. It skips `#` comments silently and exits `0` if any
one line verified, so a `.sha256` carrying a valid digest for some other file
plus a comment mentioning the archive passes while the archive is never hashed.
Filtering that file first does not fix it either: `awk '$2 == asset'` compares
one whitespace-delimited token, while `sha256sum` takes the whole rest of the
line as the filename, so a line naming `$ASSET decoy` satisfies the filter and
hashes a decoy. Building the expected line in the shell and demanding it with
`grep -qxF` removes the disagreement — there is no field-splitting left for the
two programs to differ about, and the asset name is never interpreted. The two
patterns accept GNU text mode and `-b` binary mode; `tr -d '\r'` accepts a
checksum file written on Windows.

**`set -eu` is inside the subshell.** At the top of the block it kills the
operator's login shell on a bad digest — actively harmful over a serial console,
where the natural recovery is to re-paste without it, which is the fail-open
form. Inside, a failure ends the subshell and the operator keeps their session.

**Section 2 installs from `$STAGE.verified`, which `mv` creates only after the
digest, the extraction and `test -x` have all succeeded.** That is what makes
the block safe when its exit status is ignored — by a wrapping script, an agent,
or a reader who pastes the next section anyway. Nothing needs to check a return
code, because on any failure there is nothing at the install path.

That last sentence is only true because `rm -rf` runs FIRST. An earlier version
placed it after the digest check, where it is unreachable on exactly the failure
that matters: a bad digest exits the subshell before it, so a `$STAGE.verified`
left by an earlier run — or planted by anyone who can write to the staging
directory — survived and was installed. Clearing the install path before
anything else is what makes "nothing to install" the default rather than a
consequence.

## 2. Install the base lexical node

The account and directories below are an example of the required ownership;
use the equivalent account-management command on the target Linux image. No
model directory is created for the lexical deployment.

```bash
set -eu

: "${VERSION:?run section 1 in this shell first, or set VERSION to the verified release}"
if ! getent passwd xerj >/dev/null; then
  sudo useradd --system --user-group --home-dir /var/lib/xerj --shell /sbin/nologin xerj
fi
sudo install -d -m 0755 /opt/xerj/bin /etc/xerj
sudo install -d -o xerj -g xerj -m 0750 /var/lib/xerj
sudo install -m 0755 \
  "/opt/xerj-staging/release/xerj-${VERSION}-x86_64-unknown-linux-musl.verified/xerj" \
  /opt/xerj/bin/xerj
```

Create `/etc/xerj/xerj.toml` with the complete base configuration. Explicitly
selecting lexical mode and an empty endpoint prevents an accidental proxy or
neural dependency:

```toml
[server]
bind_address = "127.0.0.1"
data_dir = "/var/lib/xerj"
es_compat_port = 9200

[auth]
enabled = true

[embedding]
mode = "lexical"
default_endpoint = ""
default_model = ""
```

Start it as the service user. On first start, the authenticated admin key is
written to `/var/lib/xerj/admin.key` with restrictive permissions.

```bash
sudo -u xerj /opt/xerj/bin/xerj --config /etc/xerj/xerj.toml
```

Keep the listener on loopback. A network-facing listener requires the separate
production TLS/auth procedure; this recipe does not teach a cleartext opt-out.

## 3. Optional neural model

Do this only when the enclave needs neural semantics. On the connected staging
machine, download the three files consumed by the built-in loader and checksum
them for the transfer:

```bash
set -eu

: "${OUT:?run section 1 in this shell first, or set OUT to the staging directory}"
MODEL="$OUT/model/all-MiniLM-L6-v2"
mkdir -p "$MODEL"
for FILE in config.json tokenizer.json model.safetensors; do
  curl -fL --retry 3 -o "$MODEL/$FILE" \
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/$FILE"
done
(
  cd "$MODEL"
  sha256sum config.json tokenizer.json model.safetensors > ../model.sha256
)
```

Transfer `OUT/model` with the release. Verify it inside the enclave before
installing anything — the same fail-closed rule as section 1, and the reason
the check is a command here rather than a sentence:

```bash
set -eu

(
  cd /opt/xerj-staging/model/all-MiniLM-L6-v2
  sha256sum -c ../model.sha256
)
```

`model.sha256` names all three files, so a transfer that dropped or truncated
one fails this check rather than installing a partial model. Then install the
files and change only the embedding block:

```bash
set -eu

sudo install -d -o xerj -g xerj -m 0750 /opt/xerj/models/all-MiniLM-L6-v2
sudo install -o xerj -g xerj -m 0640 \
  /opt/xerj-staging/model/all-MiniLM-L6-v2/{config.json,tokenizer.json,model.safetensors} \
  /opt/xerj/models/all-MiniLM-L6-v2/
```

```toml
[embedding]
mode = "neural"
local_model_dir = "/opt/xerj/models/all-MiniLM-L6-v2"
```

XERJ does not independently pin or verify model checksums; the transferred
digest is an operator supply-chain check, not a XERJ signature. If you only
need lexical search, omit this section and all model assets.

## 4. Authenticate local clients and verify

These checks are intentionally operator-run checks, not a claim that this
documentation was live-tested against a disconnected firewall. They verify the
actual binary and local files in your enclave.

```bash
set -eu

XERJ=/opt/xerj/bin/xerj
DATA=/var/lib/xerj
KEY="$(sudo cat "$DATA/admin.key")"
export XERJ_API_KEY="$KEY"

"$XERJ" --version
curl -fsS -H "Authorization: ApiKey $KEY" \
  http://127.0.0.1:9200/_cluster/health
```

With the lexical node running, index a small semantic document and query it
through the authenticated ES-compatible surface. This semantic query uses the
local lexical embedder and needs zero model files; assert that it returns
`_id` `1`.

```bash
set -eu

curl -fsS -X PUT "http://127.0.0.1:9200/offline-demo" \
  -H "Authorization: ApiKey $KEY" -H 'Content-Type: application/json' \
  -d '{"mappings":{"properties":{"body":{"type":"semantic_text"}}}}'
curl -fsS -X POST "http://127.0.0.1:9200/offline-demo/_doc/1?refresh=true" \
  -H "Authorization: ApiKey $KEY" -H 'Content-Type: application/json' \
  -d '{"body":"A local lexical node can answer this without a network service."}'
curl -fsS -X POST "http://127.0.0.1:9200/offline-demo/_search" \
  -H "Authorization: ApiKey $KEY" -H 'Content-Type: application/json' \
  -d '{"query":{"semantic":{"field":"body","query":"local lexical"}}}'
```

Repeat the health and semantic query after restarting the same data directory.
For the stronger boundary check, run the sequence with host egress disabled
and inspect the firewall/log counters; this recipe does not claim that test was
run here.

For MCP, `XERJ_AUTH` and `--auth` are complete Authorization-header values,
including the scheme:

```bash
XERJ_URL=http://127.0.0.1:9200 XERJ_AUTH="ApiKey $KEY" "$XERJ" mcp
"$XERJ" mcp --url http://127.0.0.1:9200 --auth "ApiKey $KEY"
```

For `xerj autoindex`, `XERJ_API_KEY` and `--api-key` take the raw key; the
client adds `Authorization: ApiKey` itself:

```bash
XERJ_API_KEY="$KEY" "$XERJ" autoindex /path/to/folder --url http://127.0.0.1:9200
"$XERJ" autoindex /path/to/folder --url http://127.0.0.1:9200 --api-key "$KEY"
```

## Existing data directories and local clients

The WAL tap and cluster are disabled by default. A runtime `PUT
/_xerj/wal_tap` persists an overlay and can re-enable a tap on restart when a
data directory is reused. Inspect and remove that overlay before an offline run
if the directory is not fresh:

```bash
set -eu

curl -fsS -H "Authorization: ApiKey $KEY" \
  http://127.0.0.1:9200/_xerj/wal_tap
curl -fsS -X DELETE -H "Authorization: ApiKey $KEY" \
  http://127.0.0.1:9200/_xerj/wal_tap
```

Cluster mode stays off unless explicitly enabled with peers and a shared
`cluster.auth_secret`. `xerj mcp` is a local stdio client that proxies
`XERJ_URL`; `xerj autoindex` walks files visible to the invoking machine. Neither
creates an external listener or silently uploads corpus data.

## What this page does not promise

- An offline release is not a signed or attested software supply chain. Verify
  the archive checksum and apply your organization's release-signing process.
- XERJ does not independently checksum the Hugging Face model files today.
  Stage and verify them according to your organization's model-supply-chain
  policy.
- The bundled Console falls back to system fonts, but its three external Google
  Fonts link elements remain browser egress attempts unless your deployment
  removes or rewrites that HTML; stylesheet may fetch additional font files.
- This page does not claim a live disconnected-firewall run or measured browser
  egress. The procedure is traced to the config, neural loader, installer, and
  release workflow; execute those checks in your target environment.

Related references:

- [Production deployment](./production-deployment.md) — TLS, API-key auth,
  listener boundaries, health probes, and the broader air-gapped model recipe.
- [Configuration reference](../../landing/docs/config.html) — all TOML keys and
  defaults.
- [Experimental ONNX backend](../EXPERIMENTAL_ONNX.md) — explicit feature/build
  and local asset requirements; not the standard release path.
- [Metrics privacy posture](../../metrics/README.md) — connected distribution
  analytics and what they do not measure.
