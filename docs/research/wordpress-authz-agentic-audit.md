# Thinking with XERJ: an agent reasons about WordPress authorization

This is a different exercise from [the grep comparison](wordpress-audit-with-xerj.md).
There, XERJ narrowed candidates and I read them. Here, XERJ is the agent's
**second brain** — external memory holding all 11,990 functions of real
WordPress — and the agent *reasons against it*: locate the auth primitives, read
how they actually work, build a model of correct-vs-buggy authorization, then
hunt the codebase for the buggy shape. Logic bugs live in the gap between "there
is a check" and "the check is correct," and that gap is only visible if you read
the real implementation.

No core vulnerability is claimed — core is the most-audited PHP alive, and it is
correctly gated. The deliverable is the **reasoning loop** and the **authz model
it produced**, which is exactly what transfers to plugin auditing where the real
bugs live.

## The loop (each step is a query the agent made, then reasoning)

### Step 1 — locate the gatekeepers

```
GET wpaudit  terms func:[current_user_can, wp_verify_nonce, check_ajax_referer,
                         map_meta_cap, wp_validate_auth_cookie]
```

The returned `calls` lists alone were a finding: `check_ajax_referer`'s callees
contain `wp_verify_nonce` but **no capability function**, and `wp_verify_nonce`'s
callees contain `wp_get_current_user`. Before reading a line of a body, the
second brain says: *nonces are user-bound; the ajax-referer check does no authz.*

### Step 2 — read the real implementation

`wp_verify_nonce` (wp-includes/pluggable.php): the token is

```php
substr( wp_hash( $tick . '|' . $action . '|' . $uid . '|' . $token, 'nonce' ), -12, 10 )
```

Bound to the 12-hour tick, the **action string**, the **user ID**, and the
**session token**. A valid nonce proves *this logged-in user deliberately made
this request*. It is CSRF defense — nothing more.

`check_ajax_referer` (same file): pulls the nonce from `$_REQUEST` and calls
`wp_verify_nonce`. **That is the whole function.** Zero authorization.

**Model fact #1: `check_ajax_referer()` authenticates the request's origin; it
never authorizes the action.** A Subscriber holds valid nonces. This single fact
defines a whole class of WP privilege-escalation bugs.

### Step 3 — learn what "correct" looks like

Reading the strong handlers taught the real pattern:

- `wp_ajax_trash_post`: `check_ajax_referer("{$action}_$id")` **and**
  `current_user_can('delete_post', $id)` — the nonce action and the capability
  are both **bound to the specific post `$id`**.
- `wp_edit_theme_plugin_file` (the RCE-class file writer):
  `current_user_can('edit_plugins')` **then**
  `wp_verify_nonce($nonce, 'edit-plugin_' . $file)` — the nonce action **includes
  the specific file**, so it cannot be replayed against a different file. Plus
  `validate_file()` for traversal.

**Model fact #2: WordPress's real defense is *object-scoped* meta-capabilities +
*object-scoped* nonces.** So "does the handler check a capability?" is the wrong
question. The right one is: **"is the capability bound to the object being acted
on?"** A generic `current_user_can('edit_posts')` plus a generic nonce is the
*buggy* shape — it lets a user act on objects they don't own. (This is the
pattern that recurs in plugin CVEs.)

### Step 4 — hunt the buggy shape across all 95 handlers

Core's authenticated AJAX handlers are all named `wp_ajax_<action>`:

```
GET wpaudit  prefix func:"wp_ajax_"   →  95 authenticated handlers
classify by direct callees:  nonce? capability?
```

| bucket | count |
|---|--:|
| nonce **and** capability (proper, direct) | 56 |
| nonce, **no** direct capability | 15 |
| **neither**, direct | 13 |

The 28 non-proper ones are the interesting set. The agent read the sensitive
ones and followed delegation edges:

- `wp_ajax_edit_theme_plugin_file` (in "neither") → delegates the entire check to
  `wp_edit_theme_plugin_file()`, which is the comprehensive gate above. **Cleared,
  and it taught the model.**
- `wp_ajax_untrash_post` (in "neither") → calls `wp_ajax_trash_post()`, the
  object-scoped gate. **Cleared.**
- `wp_ajax_closed_postboxes`, `hidden_columns`, `save_user_color_scheme`, … (in
  "nonce, no cap") → operate only on the **acting user's own preferences**. You
  are always allowed to edit your own state, so no capability is *needed*; the
  nonce (anti-CSRF) is the correct and sufficient control. **Cleared by
  understanding the operation, not by a rule.**

**A false-negative honesty note:** the join in Step 4 first returned *zero*
authenticated hooks, because core registers ajax actions dynamically
(`add_action("wp_ajax_$action", …)` in a loop) and the substrate's hook
extractor only captured *literal* hook strings. The agent noticed the impossible
result, reasoned about why, and pivoted to the naming convention. A silent
substrate gap is the real risk in this kind of work — it has to be caught by
sanity-checking results, exactly as here.

## Every "sink" in the flagged handlers was a false positive

Consistent with the [earlier run](wordpress-audit-with-xerj.md), and now with a
second cause identified:

- `require ABSPATH . WPINC . '/class-wp-editor.php'` → flagged **LFI**, but the
  path is a **constant**, not attacker input. (`wp_ajax_wp_link_ajax`,
  `wp_ajax_get_community_events`, and most core `require`s.)
- `echo wp_json_encode($results)` → flagged **XSS**, but it emits safe JSON as an
  ajax response, not HTML. (`wp_ajax_wp_link_ajax`.)
- `WP_Query->query()` / `WP_User_Query->get_results()` / `DOMXPath->query()` →
  flagged **SQL**, but none is `$wpdb`. (receiver-type blindness, from the earlier
  run.)

So the two concrete precision levers for the extractor are now clear:
**(a) resolve the receiver type of a `->method()` sink**, and **(b) treat
`require`/`include` of a constant/`ABSPATH`-rooted path as non-LFI.** Both are
mechanical; both would remove essentially all of the false positives seen across
both runs.

## Why this is the second-brain, not grep

Grep can find `check_ajax_referer` call sites. It cannot *read* `wp_verify_nonce`,
conclude "nonces don't authorize," derive that the correct pattern is
object-scoped meta-caps, and then use that derived model to triage 95 handlers
and follow each delegation edge to its real gate — all while holding a 619k-line
codebase in external memory and spending a few thousand tokens. The value isn't
retrieval; it's that **the agent can build and apply a model of the system's
security logic** because XERJ makes the whole system cheap to think against.

## Deepening: the interprocedural authz graph, and the true core is zero

The flat "does the handler check a capability?" classifier over-flags. The next
build (`wp_authz_graph.py`) enriches each function with the **argument shape** of
its cap/nonce checks and its state-change **call sites**, then propagates along
call edges. Three refinements — each forced by reading a real handler — took the
suspicious set from 7 to **0**:

1. **Self-scoped writes are decided at the call site.** `update_user_meta($user->ID, …)`
   where `$user = wp_get_current_user()` mutates the *session user's own* state —
   no capability required. (Fixes `closed_postboxes`, `hidden_columns`.)
2. **Polymorphic authorization counts.** `$wp_list_table->ajax_user_can()` is a
   real gate invisible to name-based cap detection. (Fixes `fetch_list`.)
3. **Trusted plumbing is an analysis boundary.** `check_ajax_referer → wp_verify_nonce
   → wp_hash → wp_salt` writes `update_site_option` (salt caching); `wp_generate_password
   → wp_rand → set_transient` (RNG seeding). These benign infra side-effects must
   not be attributed to the caller's authz surface — treat nonce/hash/RNG/cache
   primitives as leaves, exactly like the state primitives. (Fixes `generate_password`,
   `rest_nonce`, `dashboard_widgets`.)

Final sweep of all 95 authenticated AJAX handlers: **0** reach a non-self state
change or a request-identified object without an object-scoped/polymorphic cap.
Core's authenticated AJAX surface has no IDOR. Honest scope limit: this covers
`wp_ajax_*`; **REST controllers and `admin_post_*` are a separate surface** the
same graph must still sweep.

## A real XERJ finding: the code analyzer splits identifiers

Hunting the SQL flows surfaced a genuine limitation. XERJ's default `text`
analyzer tokenizes on underscore and punctuation: `esc_like` → `esc`+`like`,
`wp_unslash` → `wp`+`unslash`, `$wpdb->get_results(` → `wpdb`+`get`+`results`. So
`term`/`terms` queries for a multi-token identifier return **nothing**, and
`match` matches loosely (any sub-token). For code search this matters — you
cannot exactly distinguish `esc_like` from `esc … like`. The working pattern is
`match` for a coarse superset, then **exact regex over the returned `code`
field** for precision. A code-aware analyzer (keep `_`, keep `->`) is a concrete
autoindex improvement for the code use-case, and belongs on the same list as the
receiver-typed sinks.

## Verifying a real security flow: the SQL "escape-after-escape" de-escape

The dangerous class: a value escaped by one `prepare()`, then re-fed into another
`prepare()` — double-processing can turn a `%` in the value into a format
placeholder (placeholder injection / de-escape). The detector (prepared var
re-used inside another `prepare(`) found **one** instance in core:
`WP_List_Table::months_dropdown`, reachable from `$_GET['post_status']`:

```php
$extra_checks = $wpdb->prepare( ' AND post_status = %s', $_GET['post_status'] ); // inner
$wpdb->get_results( $wpdb->prepare(                                              // outer, interpolates $extra_checks
    "SELECT … WHERE post_type = %s  $extra_checks  ORDER BY …", $post_type ) );
```

Verified **safe**, by reading `prepare()` and `placeholder_escape()`:

- Inside `prepare()`, every literal `%` in a *value* is replaced with an
  unguessable `{hmac-sha256}` token (`placeholder_escape()`), not a `%`.
- So the inner prepare turns `$_GET['post_status'] = "X%s"` into
  `AND post_status = 'X{hmac}s'` — **no `%` survives**.
- The outer prepare's stray-`%` escaper and placeholder extractor therefore see
  nothing to misread; only its own `%s` (for `$post_type`) is a placeholder.
- At execution, a `query`-filter (`remove_placeholder_escape`, priority 0) swaps
  `{hmac}` back to a literal `%` inside the quoted string. No injection.

**The honest conclusion is the valuable one:** the double-prepare pattern is real
and *present in core*, and core is safe **only** because of the
`placeholder_escape()` hardening. Any plugin that escapes its own way —
`esc_sql` + manual quoting, `sprintf` in a loop, or `allow_unsafe_unquoted_parameters`
— bypasses that single defense and the identical pattern **de-escapes into
SQLi**. That is a flow the agent now understands end-to-end: the call site, the
`prepare()` internals, and the exact condition under which the defense is absent.

## What's next

Core's `wp_ajax_*` authz is clean and one core SQL de-escape flow is verified
safe. Remaining surfaces the same reasoning loop must still cover, to honestly
claim "all flows": **REST `permission_callback` + `admin_post_*` for IDOR**, the
**capability-map (`map_meta_cap`) edge cases**, and the **plugin ecosystem**,
where the placeholder-escape and object-scoped-cap defenses are routinely absent.
