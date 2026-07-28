# A small technical-docs corpus (the shape a RAG chatbot indexes): each item is a
# CHUNK with a doc source + heading. Deliberately mixes exact identifiers
# (error codes, function names, config keys, versions) with conceptual prose, so
# we can show where pure-cosine retrieval fails and hybrid fixes it.
import json
CHUNKS = [
 ("billing/errors","Error E4304 — card declined by issuer","Error E4304 is returned when the customer's card is declined by the issuing bank. Retrying immediately will fail; prompt the customer to contact their bank or use a different card. E4304 is not retryable and does not count against your retry budget."),
 ("billing/errors","Error E4210 — insufficient funds","Error E4210 indicates the card has insufficient funds. Unlike E4304 this IS retryable after a delay; we recommend retrying after 24 hours with exponential backoff."),
 ("billing/errors","Error E5012 — gateway timeout","Error E5012 is a transient gateway timeout. Retry with idempotency keys to avoid double charges."),
 ("api/checkout","createCheckoutSession","Call createCheckoutSession to start a payment. It returns a session URL; redirect the customer there. Pass line_items, success_url, and cancel_url. The session expires after 24 hours."),
 ("api/checkout","cancelCheckoutSession","cancelCheckoutSession voids an open session before payment. It is a no-op if the session already completed."),
 ("api/webhooks","Verifying webhook signatures","Every webhook includes a signature header. Verify it using your WEBHOOK_SIGNING_SECRET before trusting the payload. Reject requests whose signature does not match to prevent forgery."),
 ("api/webhooks","Webhook retry policy","Failed webhook deliveries are retried with exponential backoff for up to 72 hours. Return a 2xx quickly; do heavy work asynchronously."),
 ("security","Keeping API keys safe","Never commit secret keys to source control or ship them in client-side code. Store them in environment variables or a secrets manager, rotate them regularly, and use restricted keys with least privilege."),
 ("security","Rotating a compromised key","If a key is exposed, roll it immediately from the dashboard; the old key is revoked within seconds. Update WEBHOOK_SIGNING_SECRET and any deployed services."),
 ("guides","Idempotency keys","Send an Idempotency-Key header on write requests so retries don't create duplicate charges. Keys are remembered for 24 hours."),
 ("guides","Testing with sandbox mode","Use sandbox keys (prefix sk_test_) to simulate payments without moving money. Card 4242 4242 4242 4242 always succeeds; card 4000 0000 0000 0002 triggers E4304."),
 ("guides","Migrating to API v2.11","Version v2.11 changes the checkout response shape: session.url replaces session.redirect_url. Pin your version header to 2.11 and update your redirect code."),
 ("guides","Rate limits","The API allows 100 requests/second per account. Bursts above that return 429; back off and retry. Contact support to raise limits."),
 ("account","Setting up two-factor auth","Enable 2FA from account settings to protect your dashboard login. We support authenticator apps and hardware security keys."),
 ("account","Team roles and permissions","Owner, Admin, and Developer roles control dashboard access. Only Owners can rotate live keys or change billing."),
 ("guides","Handling declined payments","When a payment is declined, show the customer a clear message and offer to retry with another method. Distinguish retryable declines (insufficient funds) from hard declines (card reported lost)."),
 ("api/refunds","createRefund","createRefund issues a full or partial refund on a completed charge. Refunds settle to the original method in 5–10 business days."),
 ("api/refunds","Refund failures","A refund can fail if the original charge is too old (over 180 days) or already fully refunded."),
]
docs=[dict(id=f"c{i}",doc=d,heading=h,text=f"{h}. {t}",body=t) for i,(d,h,t) in enumerate(CHUNKS)]
json.dump(docs, open("/tmp/claude-1001/-home-claude-ai-xerj/09b4eaff-87bf-4097-b72f-3907d1ef6cd4/scratchpad/rag/chunks.json","w"))
print(f"{len(docs)} document chunks")
