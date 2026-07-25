# Findings

## Summary

| # | finding | class | severity | status |
|---|---|---|---|---|
| 1 | `wp_http_validate_url` allows `169.254.0.0/16` (cloud metadata) | SSRF (incomplete deny-list) | Medium (known-class) | **Real, verified, reachable** |
| — | AJAX / REST / admin authorization | IDOR / privesc | — | Verified **clean** (object-scoped) |
| — | SQL double-`prepare` de-escape | SQLi | — | Verified **safe** (placeholder_escape) |
| — | sanitizer composition (escape→de-escape) | XSS/SQLi | — | Verified **safe** (escaper-last) |

Only finding #1 is a real weakness. The rest are documented negatives — core is
hardened — and each was confirmed by reading the real implementation, not by a
pattern match.

---

## Finding 1 — SSRF to cloud metadata via `wp_http_validate_url`

**Component:** `wp-includes/http.php` → `wp_http_validate_url()` — core's SSRF
gatekeeper for `wp_safe_remote_get/post/request()` and XML-RPC pingbacks.

**The gap.** The private-IP deny-list blocks loopback, `10/8`, `0/8`,
`172.16/12`, and `192.168/16` — but **not `169.254.0.0/16` (link-local)**, which
contains the cloud-metadata endpoint `169.254.169.254` (AWS/GCP/Azure) and
`100.100.100.200` (Alibaba). Also missing: CGNAT `100.64/10` and IPv6.

```php
if ( 127 === $parts[0] || 10 === $parts[0] || 0 === $parts[0]
    || ( 172 === $parts[0] && 16 <= $parts[1] && 31 >= $parts[1] )
    || ( 192 === $parts[0] && 168 === $parts[1] )
) { /* reject unless the http_request_host_is_external filter allows */ }
```

**How it was found — reading, not pattern-matching.** A structural scanner sees
"there is an IP check" and moves on. The bug is that the allow/deny list is
*incomplete*. The agent read the range coverage and noticed `169.254` was absent.
This became a reusable XERJ detector (`wp_ssrf_ranges.py`) that flags any SSRF
validator missing dangerous ranges — it reproduces the finding independently.

**Verified by executing the flow.** With no PHP runtime available, the function
was ported line-for-line to Python (same scheme filter, same
`strpbrk(host, ':#?[]')`, same octet test) and executed with real
`gethostbyname`:

| payload | executed verdict |
|---|---|
| `http://127.0.0.1/`, `http://10.1.2.3/`, `http://192.168.0.1/` | REJECTED |
| `http://[::1]/` | REJECTED (`strpbrk` catches `:`) |
| **`http://169.254.169.254/latest/meta-data/iam/security-credentials/`** | **ALLOWED** |

**Reachability traced to user input.** A gap only matters if input reaches it.
Callers of the safe-remote path that pass a caller-supplied URL:

- `WP_XMLRPC_Server::pingback_ping` — the pingback **source URL is
  attacker-supplied** (unauthenticated where XML-RPC is enabled). Classic SSRF
  vector; the `169.254` gap turns it into metadata access.
- `WP_REST_URL_Details_Controller::get_remote_url` — REST endpoint, user `url`
  parameter (author-level).
- oEmbed discovery, `download_url`, pingback discovery — all fetch caller URLs.

**Impact.** A request steered at `169.254.169.254` can read cloud IAM
credentials/tokens → account compromise, on hosts where the SSRF path is reachable
and a metadata service is present.

**Honest severity.** This is a **known-class** limitation. Core historically treats
`wp_http_validate_url` as best-effort and points hardening at the
`http_request_host_is_external` filter; exploitation needs the SSRF path enabled
in a cloud environment. It is **not** claimed as a novel 0-day. It is exactly the
kind of semantic-completeness weakness that only a *read* (not a pattern) finds,
which is the case-study point.

**Suggested remediation (defense in depth).** Add `169.254.0.0/16`,
`100.64.0.0/10`, and IPv6 loopback/ULA to the default deny-list; resolve and
re-check the IP at request time (mitigates DNS-rebinding TOCTOU).

---

## Verified-clean negatives (the honest majority)

### Authorization / IDOR — clean
- **AJAX (95 handlers), REST (33 mutating + 57 read):** every state change and
  private-object read is gated by an **object-scoped meta-capability**
  (`current_user_can('edit_post', $post->ID)`, `read_app_password` with
  `$user->ID`+`uuid`, `edit_comment` with `$comment->comment_ID`).
- **check-vs-use IDOR** (cap checked on object A, operation on object B): the only
  risky shape (`WP_REST_Revisions_Controller`: cap on `parent`, returns `id`) is
  explicitly guarded — `if ($parent->ID !== $revision->post_parent) return 404`.
- **`map_meta_cap`** fails closed on missing object / missing arg.

### SQL de-escape (double `prepare`) — safe
`WP_List_Table::months_dropdown` re-feeds a `prepare()`'d value (from
`$_GET['post_status']`) into another `prepare()`. Safe **only** because
`placeholder_escape()` converts value-`%` to an unguessable `{hmac}` token,
restored at execution. Plugins that self-escape bypass this one defense and the
same pattern becomes SQLi.

### Sanitizer composition — safe
Core holds the **escaper-last-before-sink** invariant everywhere
(`get_terms`: `stripslashes` → `esc_sql` last; `wp_update_term`: `wp_unslash`
before the self-escaping `$wpdb->update`). 23 order-flagged candidates all cleared
on reading.

---

## Meta-finding: a real XERJ engine bug the audit surfaced

The reverse call-graph query "who calls `wp_safe_remote_get`" returned **1** from
XERJ's structured `term` index vs **9** (full-text) / **14 files** (grep). Root
cause: `term`/`terms` on a keyword **array** matched only element `[0]` (single-
valued keyword storage). Proven, fixed (memtable half), and written up as a PR:
[`../../research/xerj-keyword-array-term-fix.md`](../../research/xerj-keyword-array-term-fix.md).
A false "not reachable" is the worst error in an audit — surfacing it is part of
what makes this method trustworthy.
