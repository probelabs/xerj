# The cost model

Prices are Anthropic first-party API list rates, per million tokens. Verify
against `platform.claude.com/docs/en/pricing` before quoting them anywhere —
they change, and a stale price makes every downstream figure wrong.

| Model | Input | Output | Output ÷ Input |
|---|---:|---:|---:|
| Claude Opus 5 | $5.00 | $25.00 | **5×** |
| Claude Sonnet 5 | $3.00 | $15.00 | 5× |
| Claude Fable 5 | $10.00 | $50.00 | 5× |
| Claude Haiku 4.5 | $1.00 | $5.00 | 5× |

Cache reads cost **~0.1×** base input. Cache writes cost **1.25×** (5-minute TTL)
or **2×** (1-hour TTL). On Opus 5 the minimum cacheable prefix is **512 tokens**.

## Why retrieval can pay

On Opus 5, cached input runs about **$0.50/MTok** against **$25/MTok** for
output — a factor of **50**. So the arithmetic is:

> Retrieval is worth it when the context it adds costs less than the output it
> prevents. Because output is 50× cached input, a retrieved passage has a large
> budget to work with — but only if it actually removes work.

That last clause is the whole game. **Retrieval that does not reduce iterations
is a pure loss**: you paid for the passage and still ran every lap.

## Break-even

Let `C` be the retrieved context tokens added per task, and `L` the output
tokens of one avoided retry lap. Retrieval pays when:

```
C × input_price  <  L × output_price
C × 5            <  L × 25          (Opus 5, uncached)
C                <  5L              (uncached)
C                <  50L             (cached — the prefix survives across turns)
```

A retry lap that produces 800 output tokens justifies up to **4,000** uncached
or **40,000** cached context tokens. That is a wide margin, which is why this
approach tends to win — and exactly why it must still be measured rather than
assumed.

## Where it loses

State these plainly; a skill that only describes its wins is marketing.

1. **Irrelevant retrieval.** Wrong corpus → passages add tokens, mislead the
   agent, and can *increase* laps. This is the most common failure.
2. **Tasks that never loop.** If the agent would have got it first try, every
   retrieved token is waste.
3. **Cache-hostile placement.** Injecting retrieved passages *above* stable
   content invalidates the cached prefix and re-bills the whole conversation at
   full input price. Retrieved context belongs **after** the last cache
   breakpoint. Getting this backwards turns a saving into a large loss.
4. **Setup amortisation.** Cloning and indexing a corpus costs wall-clock and
   disk. On a one-off task it never repays.
5. **Stale index.** Returns code that no longer exists, with provenance that
   looks authoritative. Worse than no retrieval, because it is believed.

## Counting tokens

Use the API — `client.messages.count_tokens(model=..., messages=[...])`. Token
counts are model-specific.

**Do not use `tiktoken`.** It is OpenAI's tokenizer; it undercounts Claude
tokens by roughly 15–20% on prose and considerably more on code.

Where an environment has no API credentials, `ab.py` measures **bytes** and says
so. A byte count is an honest proxy for comparing two arms of the same
experiment; it is **not** a token count and must never be reported as one.
