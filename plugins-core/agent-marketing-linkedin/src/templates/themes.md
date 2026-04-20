# LinkedIn 10-Theme Wheel

Canonical order. One post per theme. Don't reorder — the cognitive progression is intentional.

1. **launch-day** — What shipped today. Zero buildup, straight facts. "Shipped: X. Open source. MIT."
2. **single-insight** — The sharpest idea in the product as a standalone essay. No product pitch — just the idea. Pitch comes in the last line.
3. **killer-feature** — One feature that embodies the whole product. Shows, not tells.
4. **personality** — The humor, design choice, or stance that makes the product feel alive.
5. **use-case-expansion** — "Not just X — also Y and Z." Expands the perceived market.
6. **lore** — The weird detail that makes people screenshot. An easter egg, a naming story, a hidden flag.
7. **origin** — Why this exists. Personal. First person. One specific moment that started it.
8. **dogfood-receipt** — Proof. Numbers, commit hashes, test counts, bug lists. "Here's the receipt."
9. **install-cta** — Zero friction copy-paste. One line to install. One link to the repo.
10. **community-call** — Ask for contribution, validation, feedback. Never "please like this post" — always a real ask with a real action.

## Theme prompt pattern

For theme N, build the prompt as:

```
System: marketing-linkedin SKILL.md + theme wheel definition
Context: campaign brief + reference example for theme N (from examples/)
Task: Write a LinkedIn post for theme "{theme_name}" about product "{product}".
       Target 150-300 words. First line is the hook. End with CTA + 4-5 hashtags.
       Match the voice of the reference example exactly.
Output: pure post text, no preamble, no JSON wrapper.
```

The product + brief change. The wheel and tone are constants.
