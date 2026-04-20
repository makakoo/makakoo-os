# ACKNOWLEDGEMENTS

brain-anchors is a composition layer, not a rewrite. ~90% of the
infrastructure it sits on was already built inside Harvey (`brain_docs`,
`memory_summarizer.py`, `entity_graph`, Qdrant, switchAILocal). The
architectural ideas are borrowed from published work and open-source
projects, credited below.

## Primary influences

- **HippoRAG** — [OSU-NLP-Group/HippoRAG](https://github.com/OSU-NLP-Group/HippoRAG),
  arXiv [2405.14831](https://arxiv.org/abs/2405.14831),
  arXiv [2502.14802](https://arxiv.org/abs/2502.14802).
  The triples-as-anchor + passages-as-expansion split, and the idea
  of using Personalized PageRank over an entity graph to re-rank
  retrieved items. brain-anchors' read path is essentially HippoRAG's
  read path with Letta-style explicit expansion bolted on.

- **Mem0** — [mem0ai/mem0](https://github.com/mem0ai/mem0),
  arXiv [2504.19413](https://arxiv.org/abs/2504.19413).
  The write-time state machine — `ADD` / `UPDATE` / `DELETE` / `NOOP`
  — that prevents Harvey's Brain from turning into an append-only
  log of near-duplicates. Mem0 showed that the dedup decision belongs
  before the vector write, not after.

- **RAPTOR** — [parthsarthi03/raptor](https://github.com/parthsarthi03/raptor),
  arXiv [2401.18059](https://arxiv.org/abs/2401.18059).
  The collapsed-tree multi-level anchor index (`atomic` / `summary` /
  `root`). Phase F of the brain-anchors migration is a direct
  RAPTOR-style clustering pass over atomic anchors.

- **MemGPT / Letta** — [letta-ai/letta](https://github.com/letta-ai/letta),
  arXiv [2310.08560](https://arxiv.org/abs/2310.08560).
  The function-gated expansion pattern — the LLM explicitly calls
  `expand_anchor(id)` rather than having the retrieval layer shove
  full passages into the prompt by default. This is the single
  biggest token-saver in the read path.

## Adapted patterns

- **[JuliusBrussee/caveman](https://github.com/JuliusBrussee/caveman)** (MIT).
  The preservation-rule + validator-diff idea in caveman's
  output-compression mode. brain-anchors adapts the validator pattern
  for fact preservation (5W1H round-trip) instead of code/URL
  preservation. The shape of the "round-trip check" is caveman's.

## Conceptual grounding

- **Pentti Kanerva — Sparse Distributed Memory (1988).** The
  hippocampal intuition: many noisy anchors, aggregate at read, never
  trust any single anchor alone. Kanerva's SDM is the reason
  brain-anchors uses top-K over multiple retrieval signals (FTS +
  vector + PPR) instead of betting on one.

## Harvey-internal prior art

- **`harvey-os/core/superbrain/memory_summarizer.py`** — already
  produced human-readable page summaries and already wrote to
  `entity_graph`. brain-anchors reuses ~60% of its extraction
  scaffolding; the new extractor is a tighter, JSON-only sibling.
- **`harvey-os/core/superbrain/entity_graph/`** — the triple store and
  PPR implementation already existed. brain-anchors just seeds PPR
  with entities pulled from anchors instead of from raw passages.
- **switchAILocal** — makes the "route everything through one local
  gateway" rule trivial. Without it, brain-anchors would need its
  own provider fallback logic.

## License

Harvey's adaptation is MIT-licensed, matching Harvey-OS. Upstream
projects retain their own licenses — see each repo linked above.
