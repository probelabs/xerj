# Reading-first core audit: what the detectors couldn't see

The structural detectors ([authz graph](wordpress-authz-agentic-audit.md),
check-vs-use, double-prepare) concluded core is clean — because they only match
*shapes*. Logic bugs live in *semantics*: a defense that is present but
**incomplete**. This pass switched mode — the agent used XERJ only to *navigate*
to logic-heavy security functions, then **read them in full and reasoned about
exploitability**, the way a human auditor does. Then each finding was turned back
into a XERJ detector.

## Confirmed solid on close reading (negative results, stated honestly)

- **`wp_validate_redirect`** — the allowlist regex strips backslashes (kills the
  `\`→`/` browser open-redirect bypass), `_deep_replace` strips CRLF recursively
  (kills `%0%0a0a`), and `@`-userinfo tricks resolve to the real host, which is
  allow-list-checked. Robust.
- **`maybe_unserialize` / `maybe_serialize`** — symmetric for objects (both use
  `/^O:[0-9]+:/`), so a string that *looks* serialized is double-serialized on
  write and returns as a string on read. String-meta PHP Object Injection is
  defended for data written through the pair.
- **Widget instance `unserialize`** (customizer) — gated by
  `hash_equals( wp_hash($decoded), $instance_hash_key )` before `unserialize`.
  An attacker can't forge the secret-key HMAC, so POI is defended.

## The finding: SSRF to cloud metadata via `wp_http_validate_url`

`wp_http_validate_url()` is core's SSRF gatekeeper for `wp_safe_remote_*` and
pingbacks. Its private-IP blocklist:

```php
if ( 127 === $parts[0] || 10 === $parts[0] || 0 === $parts[0]
    || ( 172 === $parts[0] && 16 <= $parts[1] && 31 >= $parts[1] )
    || ( 192 === $parts[0] && 168 === $parts[1] )
) { /* reject unless http_request_host_is_external allows */ }
```

Reading the *coverage* (not the shape) shows the gap: it blocks loopback, 10/8,
0/8, 172.16/12, 192.168/16 — but **not `169.254.0.0/16` (link-local)**, which
contains **`169.254.169.254`, the AWS / GCP / Azure cloud-metadata endpoint**
(and `100.100.100.200`, Alibaba). Verified against the exact logic:

| target | verdict |
|---|---|
| 127.0.0.1, 10.x, 172.16.x, 192.168.x | BLOCKED |
| **169.254.169.254 (cloud metadata)** | **ALLOWED** |
| 100.64/10 (CGNAT) | ALLOWED |

**Impact.** Any core feature or plugin that passes a user-supplied URL through
`wp_safe_remote_get()` / the pingback path can be steered at the metadata service
→ theft of cloud IAM credentials. Also missing: CGNAT `100.64/10` and IPv6
(loopback `::1`, ULA `fc00::/7`) — though IPv6 host *literals* are separately
rejected by the `strpbrk(host, ':#?[]')` check.

**Honest status.** This is a real, verifiable gap, but a *known-class* one: core
historically treats `wp_http_validate_url` as best-effort and points hardening at
the `http_request_host_is_external` filter, and exploitation needs the SSRF
feature reachable in a cloud environment. It is not claimed as a novel 0-day. It
is exactly the kind of semantic-completeness weakness that **only a read** finds —
and that is the point.

## Improving XERJ from the finding

The bug class is *an incomplete allow/deny list in a security validator*.
`docs/examples/ast-vuln-graph/wp_ssrf_ranges.py` encodes it: locate IP-range SSRF
validators (host resolution + octet comparisons), then check their coverage
against the set of ranges every validator **should** block, and report what's
**missing**. Run against core it independently reproduces the finding:

```
### wp_http_validate_url  (wp-includes/http.php)
    [x] 127.0.0.0/8   [x] 10/8   [x] 172.16/12   [x] 192.168/16
    [ ] 169.254.0.0/16 link-local/METADATA   <<< MISSING
    [ ] 100.64.0.0/10 CGNAT                   <<< MISSING
    [ ] IPv6 ::1 / ULA fc00::/7               <<< MISSING
    !! SSRF TO CLOUD METADATA POSSIBLE
```

This is the loop worth building for XERJ as an audit substrate: a human (or an
AI) reads and finds a semantic gap once; the finding is encoded as a
completeness check; XERJ then carries it across every future audit — of core
upgrades and, more usefully, of the plugin ecosystem, where SSRF validators are
frequently hand-rolled and far more incomplete than core's.

### The general lesson for XERJ

Structural detectors answer "is the defense *present*?" The bugs that survive
audit are "is the defense *complete*?" — a missing IP range, an unescaped context,
an un-re-verified object relationship. XERJ's contribution is to make each such
semantic invariant, once discovered by reading, a **stored, queryable
completeness check** over the whole indexed codebase. Precision comes from the
read; durability and scale come from XERJ.
