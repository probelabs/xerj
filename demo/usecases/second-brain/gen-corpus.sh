#!/usr/bin/env bash
# gen-corpus.sh [DEST] — write the second-brain demo vault: a realistic
# personal knowledge base (bread baking + a bakery-launch project) that
# exercises every edge detector:
#
#   notes/                  15 zettels, [[wikilink]]-rich        → wikilink@1
#   projects/bakery-launch/  6 files, relative markdown links    → mdlink@1
#   clippings/               4 saved HTML pages, <a href>        → href@1
#   journal/                12 dated entries, NO authored links  → structural only
#   reading/                 1 long note (>2 sections)           → sequence@1
#   (every directory with 2+ files also chains)                  → samedir@1
#
# Determinism: file bytes are fixed and every mtime is pinned with `touch -d`.
# Detector edges take `valid_at` from mtime, so re-running this generator and
# re-running `xerj brain` reproduces the SAME edge_ids every time (only
# `created_at` — when the brain learned it — is wall clock, by contract).
#
# One wikilink is deliberately dangling ([[banana-bread]] has no note): the
# run summary must count it as unresolved, never invent an edge for it.
#
# Idempotent: wipes and rewrites DEST on every run.
set -euo pipefail

DEMO_ROOT="${XERJ_BRAIN_DEMO_ROOT:-${TMPDIR:-/tmp}/xerj-second-brain-demo}"
DEST="${1:-$DEMO_ROOT/vault}"

rm -rf "$DEST"
mkdir -p "$DEST/notes" "$DEST/projects/bakery-launch" "$DEST/clippings" \
         "$DEST/journal" "$DEST/reading"

# ── notes/ — the zettel cluster (wikilinks) ──────────────────────────────
cat > "$DEST/notes/sourdough-starter.md" <<'EOF'
# Sourdough starter

A starter is a stable culture of wild yeast and lactobacilli in a flour-water
paste. Mine is 100% hydration, fed daily — see [[starter-maintenance]] for the
schedule and [[hydration]] for what the percentage means.

A young, sweet starter builds a milder [[levain]]; an older, more acidic one
brings tang but weakens gluten. Starting a new culture goes fastest with some
[[rye-flour]] in the first feeds — more wild yeast lives on the bran.
EOF

cat > "$DEST/notes/hydration.md" <<'EOF'
# Hydration

Hydration is water as a percentage of flour weight, the core of
[[baker-percentages]]. 65% is a tight sandwich loaf; 75-80% is where open
[[crumb-structure]] lives; past 85% the dough demands real skill.

Higher hydration = looser dough, longer bulk, bigger holes. It is the single
most consequential number in the formula.
EOF

cat > "$DEST/notes/autolyse.md" <<'EOF'
# Autolyse

Mix just flour and water and rest 20-60 minutes before adding salt and
starter. Enzymes begin [[gluten-development]] with zero kneading, and the
flour hydrates fully — see [[hydration]].

Skip it for rye-heavy doughs; rye has little gluten to develop anyway.
EOF

cat > "$DEST/notes/bulk-fermentation.md" <<'EOF'
# Bulk fermentation

The first rise, from mixing until divide. This is where flavour and strength
happen: stretch-and-folds every 30-45 minutes build [[gluten-development]]
without a mixer.

Judge by the dough, not the clock — 50% growth, domed edges, a few surface
bubbles. What follows is shaping and final [[proofing]].
EOF

cat > "$DEST/notes/gluten-development.md" <<'EOF'
# Gluten development

Glutenin and gliadin cross-link into the network that traps gas. Windowpane
test: stretch a piece thin enough to read through without tearing.

Strong development plus good fermentation gives the open, glossy
[[crumb-structure]] everyone photographs.
EOF

cat > "$DEST/notes/levain.md" <<'EOF'
# Levain

An offshoot of the [[sourdough-starter]] built for one specific bake, so the
mother culture is never depleted. Build it 4-12 hours ahead, use it at peak.

The levain's ripeness sets the tempo of [[bulk-fermentation]]: a ripe, active
levain can cut bulk time by a third.
EOF

cat > "$DEST/notes/scoring.md" <<'EOF'
# Scoring

The lame cut that tells the loaf where to open. Shallow angled cuts make ears;
deep vertical cuts make width. Score decisively — hesitation drags.

Scoring is the release valve for [[oven-spring]]: an unscored high-hydration
loaf bursts wherever the crust is weakest.
EOF

cat > "$DEST/notes/oven-spring.md" <<'EOF'
# Oven spring

The dramatic rise in the first 15 minutes of the bake: trapped CO2 expands,
water flashes to steam, and yeast has one last party before 60°C.

Steam is everything — it keeps the crust soft long enough to expand, which is
the whole point of the [[dutch-oven-method]]. Good [[scoring]] directs it.
EOF

cat > "$DEST/notes/baker-percentages.md" <<'EOF'
# Baker's percentages

Everything is expressed relative to total flour = 100%. Water 75%, salt 2%,
starter 20% — a formula, not a recipe, so it scales to any batch size.

[[hydration]] is the percentage that changes the bread most.
EOF

cat > "$DEST/notes/dutch-oven-method.md" <<'EOF'
# Dutch oven method

A preheated cast-iron pot is a home oven's steam injection: the loaf's own
moisture stays trapped for the first 20 minutes, maximising [[oven-spring]].

Lid on 20 min at 250°C, lid off 20-25 min at 230°C. One day I should try the
same trick for [[banana-bread]], though that is a quick bread and probably
does not care.
EOF

cat > "$DEST/notes/rye-flour.md" <<'EOF'
# Rye flour

Low gluten, high enzymes, ferments fast. Even 10% rye darkens flavour
noticeably; 100% rye is a different craft entirely (dense, pentosan-bound).

Compared with [[whole-wheat]], rye brings more sourness and stickier dough.
It is the best fuel for a young [[sourdough-starter]].
EOF

cat > "$DEST/notes/whole-wheat.md" <<'EOF'
# Whole wheat

Bran and germ included: more flavour, more nutrition, and sharp bran edges
that cut gluten strands. Compensate with more water — add 5% [[hydration]]
for every 20% whole grain — and gentler handling than white flour.

See [[rye-flour]] for the other whole-grain regular in my rotation.
EOF

cat > "$DEST/notes/crumb-structure.md" <<'EOF'
# Crumb structure

The pattern of holes in the slice — the bake's report card. Even, open,
glossy alveoli need three things at once: enough [[hydration]], full
[[gluten-development]], and fermentation stopped at the right moment.

Dense streaks near the bottom usually mean underproofing, not underbaking.
EOF

cat > "$DEST/notes/proofing.md" <<'EOF'
# Proofing

The final rise after shaping, seam-side up in a banneton. Poke test: a slow,
partial spring-back means ready.

Overproofed dough has spent its gas — it flattens when turned out. When in
doubt, cold-retard overnight; the fridge slows [[bulk-fermentation]] chemistry
to a crawl and makes [[scoring]] far easier on the firm surface.
EOF

cat > "$DEST/notes/starter-maintenance.md" <<'EOF'
# Starter maintenance

Daily at room temperature: discard to 20g, feed 1:4:4. Weekly if refrigerated;
revive with two feeds before baking. Hooch on top just means hungry.

The mother [[sourdough-starter]] never goes into dough directly — build a
[[levain]] instead and keep the culture safe.
EOF

# ── vault root — the entry note ──────────────────────────────────────────
cat > "$DEST/README.md" <<'EOF'
# Baking vault

My second brain for bread. The science lives in the zettels — start from
[[sourdough-starter]] and [[bulk-fermentation]] — the bakery plan lives in
[the project folder](projects/bakery-launch/01-business-plan.md), and every
bake gets a line in the journal.
EOF

# ── projects/bakery-launch/ — relative markdown links ────────────────────
cat > "$DEST/projects/bakery-launch/01-business-plan.md" <<'EOF'
# Micro-bakery: business plan

Weekend micro-bakery, 40 loaves a batch, subscription-first. The product is
the naturally-leavened loaf my [starter culture](../../notes/sourdough-starter.md)
already makes — the plan is only about repeating it at scale, which is worked
out in [recipes at scale](03-recipes-at-scale.md).

Revenue target: 40 loaves × 2 bakes × €6.50 = €520/week before costs.
EOF

cat > "$DEST/projects/bakery-launch/02-equipment.md" <<'EOF'
# Equipment list

The deck oven replaces the [dutch oven method](../../notes/dutch-oven-method.md)
with real steam injection. Until it arrives, batch-baking continues in three
cast-iron pots.

- Rofco B40 deck oven — quoted €1,850 (see https://www.rofco.be for specs)
- Spiral mixer, 20L
- Proofing retarder (a converted fridge for now)
EOF

cat > "$DEST/projects/bakery-launch/03-recipes-at-scale.md" <<'EOF'
# Recipes at scale

Everything converts through [baker's percentages](../../notes/baker-percentages.md):
the formula is batch-size-free by construction. The house loaf stays at 78%
[hydration](../../notes/hydration.md) — customers buy the open crumb.

Scale test log: 4kg flour batch ferments ~20% faster than 1kg (thermal mass);
pull bulk earlier.
EOF

cat > "$DEST/projects/bakery-launch/04-permits.md" <<'EOF'
# Permits and registration

Cottage food registration first, full food-business licence only if the
weekly volume passes the exemption threshold. Kitchen inspection checklist
requested from the municipality (https://example.gov/food-business).

Insurance quote pending. No links to the science here on purpose — this file
is pure bureaucracy.
EOF

cat > "$DEST/projects/bakery-launch/05-launch-checklist.md" <<'EOF'
# Launch checklist

- [ ] Sign off the [business plan](01-business-plan.md)
- [ ] Close out [permits](04-permits.md)
- [ ] Deck oven delivered and cured
- [ ] Subscription page live
- [ ] Dry run: two full batches back to back
EOF

cat > "$DEST/projects/bakery-launch/notes-from-mentor.md" <<'EOF'
# Notes from the mentor call

Talked an hour with Ana (ex-Poilâne). Her rules, verbatim:

"Volume changes fermentation more than any recipe tweak — re-learn
[bulk fermentation](../../notes/bulk-fermentation.md) at every batch size."

"Sell the schedule, not the bread. People subscribe to Saturday."
EOF

# ── clippings/ — saved HTML pages (href) ─────────────────────────────────
cat > "$DEST/clippings/maillard-reaction.html" <<'EOF'
<!DOCTYPE html>
<html>
<head><title>The Maillard reaction in bread crust</title></head>
<body>
<h1>The Maillard reaction in bread crust</h1>
<p>Browning is chemistry between amino acids and reducing sugars above
140&deg;C. In a lean dough it is concentrated at the crust, which is why
steam timing &mdash; and with it <a href="../notes/oven-spring.md">oven
spring</a> &mdash; decides both colour and flavour.</p>
<p>Source: <a href="https://en.wikipedia.org/wiki/Maillard_reaction">the
Wikipedia article</a> this clip summarises.</p>
</body>
</html>
EOF

cat > "$DEST/clippings/flour-protein-guide.html" <<'EOF'
<!DOCTYPE html>
<html>
<head><title>Flour protein content guide</title></head>
<body>
<h1>Flour protein, by the numbers</h1>
<p>Bread flour 12-14%, all-purpose 10-12%, pastry 8-9%. Whole-grain flours
report high protein but behave weaker &mdash; my notes on
<a href="../notes/whole-wheat.md">whole wheat</a> and
<a href="../notes/rye-flour.md">rye</a> cover why the bran matters more than
the number.</p>
</body>
</html>
EOF

cat > "$DEST/clippings/steam-injection.html" <<'EOF'
<!DOCTYPE html>
<html>
<head><title>Steam in professional deck ovens</title></head>
<body>
<h1>Steam in professional deck ovens</h1>
<p>Commercial deck ovens inject steam for the first minutes of the bake; the
home equivalent is the <a href="../notes/dutch-oven-method.md">dutch oven
method</a>. Overdo steam and the crust turns leathery instead of crisp.</p>
</body>
</html>
EOF

cat > "$DEST/clippings/banneton-care.html" <<'EOF'
<!DOCTYPE html>
<html>
<head><title>Banneton care</title></head>
<body>
<h1>Banneton care</h1>
<p>Flour the basket with rice flour (it does not hydrate into glue), never
wash it, dry it after every use. A well-kept banneton releases the loaf
cleanly at the end of <a href="../notes/proofing.md">proofing</a>.</p>
</body>
</html>
EOF

# ── journal/ — dated entries, deliberately link-free ─────────────────────
# The structural-only corner of the vault: every edge here comes from
# samedir@1 (files sitting together), none from authored links — this is
# what the dashboard's AUTHORED vs STRUCTURAL split makes visible.
cat > "$DEST/journal/2026-05-03.md" <<'EOF'
Saturday bake #31. 75% hydration, 20% levain, 2% salt. Bulk 4h30 at 24C.
Best ear so far — the new blade angle works. Crumb slightly tight at the
bottom, probably shaped too cold.
EOF
cat > "$DEST/journal/2026-05-10.md" <<'EOF'
Bake #32. Pushed hydration to 78%. Dough was a puddle after mix but three
folds saved it. Open crumb, glossy, the best slice photo yet. Neighbour
claimed a whole loaf.
EOF
cat > "$DEST/journal/2026-05-17.md" <<'EOF'
Bake #33. Tried 15% rye. Fermentation went visibly faster — pulled bulk at
3h45 and it was already at the edge. Tangier than usual, crumb tighter.
Would do 10% next time.
EOF
cat > "$DEST/journal/2026-05-24.md" <<'EOF'
Bake #34. Cold-retarded overnight for the first time in weeks. Scoring on
fridge-cold dough is a different sport — clean single slash, textbook ear.
Flavour deeper. This is the schedule now.
EOF
cat > "$DEST/journal/2026-05-31.md" <<'EOF'
Bake #35. Disaster bake. Forgot salt until the second fold, dough never
tightened, loaf spread flat. Still tasted fine toasted. Lesson: mise en
place, always.
EOF
cat > "$DEST/journal/2026-06-07.md" <<'EOF'
Bake #36. Back to the standard formula to recalibrate. Solid, unremarkable,
exactly what a control bake should be. Starter smells great since the rye
feeds.
EOF
cat > "$DEST/journal/2026-06-14.md" <<'EOF'
Bake #37. Doubled the batch to four loaves as a scale test for the bakery
idea. The mixing bowl is now officially too small. Fermentation ran hotter
with the bigger dough mass — pulled bulk 30 min early.
EOF
cat > "$DEST/journal/2026-06-21.md" <<'EOF'
Bake #38. Four loaves again, staggered bakes. Second pair overproofed while
waiting for the oven — need a retarder, added it to the equipment list.
Subscribers (mum, two colleagues) unbothered.
EOF
cat > "$DEST/journal/2026-06-28.md" <<'EOF'
Bake #39. Whole-wheat at 30%, water up 7%. Nutty, moist, keeps better than
the white loaf. A keeper formula for the winter menu.
EOF
cat > "$DEST/journal/2026-07-05.md" <<'EOF'
Bake #40. Milestone bake, gave loaves away at the park run. Two people asked
if they could order. Wrote both emails down. This might actually be a
business.
EOF
cat > "$DEST/journal/2026-07-12.md" <<'EOF'
Bake #41. First dry run of the Saturday production schedule: levain at 06:00,
mix at 07:30, bake windows 13:00 and 14:00. Made it with 20 minutes to
spare. Feet hurt.
EOF
cat > "$DEST/journal/2026-07-19.md" <<'EOF'
Bake #42. Second dry run, smoother. Timed every step for the checklist.
Best batch consistency yet — all eight loaves within 40g of each other.
Ready to sign off the plan.
EOF

# ── reading/ — one long note that splits into sections (sequence) ────────
# ~3.3KB of prose — past the 2KB section target, so autoindex splits it,
# and each section s_{i} gets a sequence@1 edge from s_{i-1} — the
# "document's own narrative order survives the split" signal.
cat > "$DEST/reading/bread-science-notes.md" <<'EOF'
# Reading notes: bread science, one long braid

Working notes from three books read back to back — Hamelman's "Bread",
Forkish's "Flour Water Salt Yeast", and Robertson's "Tartine Bread" — folded
together into one narrative so future-me stops re-reading the same chapters.
The through-line of all three: bread is four ingredients and three control
knobs (time, temperature, hydration), and every technique in every book is
just a different grip on those knobs.

Hamelman is the engineer. His chapters on fermentation read like process
control: the dough is a bioreactor, the baker's job is to keep it inside an
envelope, and every visible sign — doming, jiggle, surface bubbles — is
telemetry, not folklore. His preferments taxonomy (poolish, biga, levain,
pate fermentee) finally made the terminology stick: they differ in hydration
and yeast source, nothing else. The rest is marketing by tradition. What I
took into my own process: read the dough at fixed checkpoints instead of
checking the clock, and log what the checkpoints looked like. That is the
whole reason the journal folder in this vault exists.

Forkish is the schedule. His entire system is built so that a person with a
job can bake serious bread: overnight bulk at cool temperatures, tiny levain
percentages, everything tuned so the dough's slow chemistry does the work
while you sleep. The insight that transfers: levain percentage is a throttle.
Five percent ripe levain and a 78-degree kitchen gives the same fermentation
arc as twenty percent in a cold one — pick the percentage that makes the
timeline fit your life, not the recipe's. His iron discipline about dough
temperature (he measures water to the degree) looked fussy until the
four-loaf scale test proved him right: mass changes everything, and the only
way to keep the arc constant across batch sizes is to hit the same dough
temperature every time.

Robertson is the palate. Tartine's method is the least precise of the three
and the most opinionated about what the loaf should BE: custardy open crumb,
blistered mahogany crust, acidity in balance rather than in charge. His young
levain doctrine — use the starter hours before it peaks, when it smells of
yogurt rather than vinegar — directly contradicts the older sour-is-authentic
school, and my own side-by-side bakes came down firmly on Robertson's side.
The same bake also taught me that his shaping technique carries more of the
open-crumb result than his formula does: surface tension built during
shaping is what lets a wet dough stand tall through oven spring instead of
relaxing into a pancake.

Where the three books disagree, the disagreement is itself the lesson.
Hamelman wants strong development early (improved mix), Forkish wants almost
none (folds only), Robertson is in between — and all three produce great
bread, because development and fermentation time trade off against each
other. Strong early gluten tolerates shorter bulk; lazy mixing demands the
long slow build that also happens to develop more flavour. Pick the pair
that fits the schedule. There is no single correct process, only internally
consistent ones — which is the most liberating sentence in any of the three
books, and the reason this note exists as one braid instead of three
summaries.
EOF

# ── pin every mtime (valid_at = mtime; determinism depends on this) ──────
t() { touch -d "$1" "$DEST/$2"; }

t "2026-05-01 09:00:00 UTC" README.md
t "2026-05-02 10:00:00 UTC" notes/sourdough-starter.md
t "2026-05-04 10:00:00 UTC" notes/starter-maintenance.md
t "2026-05-06 10:00:00 UTC" notes/hydration.md
t "2026-05-08 10:00:00 UTC" notes/baker-percentages.md
t "2026-05-11 10:00:00 UTC" notes/levain.md
t "2026-05-14 10:00:00 UTC" notes/autolyse.md
t "2026-05-18 10:00:00 UTC" notes/bulk-fermentation.md
t "2026-05-21 10:00:00 UTC" notes/gluten-development.md
t "2026-05-25 10:00:00 UTC" notes/crumb-structure.md
t "2026-05-28 10:00:00 UTC" notes/proofing.md
t "2026-06-01 10:00:00 UTC" notes/scoring.md
t "2026-06-04 10:00:00 UTC" notes/oven-spring.md
t "2026-06-08 10:00:00 UTC" notes/dutch-oven-method.md
t "2026-06-10 10:00:00 UTC" notes/rye-flour.md
t "2026-06-11 10:00:00 UTC" notes/whole-wheat.md

t "2026-06-12 12:00:00 UTC" clippings/maillard-reaction.html
t "2026-06-14 12:00:00 UTC" clippings/flour-protein-guide.html
t "2026-06-16 12:00:00 UTC" clippings/steam-injection.html
t "2026-06-18 12:00:00 UTC" clippings/banneton-care.html

for d in 2026-05-03 2026-05-10 2026-05-17 2026-05-24 2026-05-31 \
         2026-06-07 2026-06-14 2026-06-21 2026-06-28 \
         2026-07-05 2026-07-12 2026-07-19; do
  t "$d 20:00:00 UTC" "journal/$d.md"
done

t "2026-06-20 09:00:00 UTC" projects/bakery-launch/01-business-plan.md
t "2026-06-23 09:00:00 UTC" projects/bakery-launch/02-equipment.md
t "2026-06-26 09:00:00 UTC" projects/bakery-launch/03-recipes-at-scale.md
t "2026-06-29 09:00:00 UTC" projects/bakery-launch/04-permits.md
t "2026-07-02 09:00:00 UTC" projects/bakery-launch/05-launch-checklist.md
t "2026-07-03 09:00:00 UTC" projects/bakery-launch/notes-from-mentor.md

t "2026-07-06 21:00:00 UTC" reading/bread-science-notes.md

n=$(find "$DEST" -type f | wc -l)
echo "corpus: $n files under $DEST (bytes and mtimes pinned — re-runs are identical)"
