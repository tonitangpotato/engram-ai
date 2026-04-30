# Discussion: Spreading Activation as the Multi-Hop Retrieval Model

> Status: **discussion / exploration** — written 2026-04-29 by potato + rustclaw.
> This is not a design doc yet. It captures the conversation that led to
> abandoning beam-search-based multi-hop (ISS-070 V1) in favor of a
> spreading-activation model. A formal design doc will follow once a
> prototype validates the approach on LoCoMo conv-26.

## Table of contents

1. Origin: why this discussion happened
2. The 6 standard multi-hop approaches and why none felt right
3. How human memory actually handles "multi-hop"
4. Why engram v0.2 didn't have a multi-hop problem
5. Why v0.3 created one — and what we did right anyway
6. Substrate vs traversal: the framing that resolves the confusion
7. KG as substrate: necessary, but not the same as "modeling the brain"
8. Mapping brain → engram: the operational analogy
9. Spreading activation: the algorithm
10. Open questions answered in this discussion
    - 10.1 Do we read node/edge semantics during spreading?
    - 10.2 Convergence vs fixed iterations — what does the brain do?
    - 10.3 Inhibition, normalization, sparsity in the brain vs in code
    - 10.4 Can we replicate "structure is meaning"?
    - 10.5 Why must we discretize?
11. Decisions taken
12. Next step: prototype scope
13. What this means for ISS-070

---

## 1. Origin: why this discussion happened

ISS-070 was filed to fix the LoCoMo `multi-hop` category 0/3 hit@5 result
in RUN-0006. The first design pass (`.gid/issues/ISS-070/design.md`,
preserved as V1) proposed a **beam search** over typed edges: BFS with
top-K pruning per hop, max depth 3, predicate filter to exclude
`RelatedTo` / `MentionedIn` / `Contradicts`.

After review, potato pushed back: the 6 standard multi-hop approaches
(graph DB query, PathRAG, beam search, GNN embedding, agent-style LLM
exploration, subgraph extraction) all felt wrong. The unease was
specific: **none of them resemble how human memory actually works**, and
engram's stated goal is to learn from how the brain handles memory.

This document records the discussion that followed. The conclusion was
that beam search applies a symbolic-KG-traversal paradigm to a substrate
(v0.3 typed graph) that should instead be supporting an
**activation-spreading** paradigm. The fix is not a new plan — it is a
re-framing of what retrieval *is*.

## 2. The 6 standard multi-hop approaches and why none felt right

| # | Approach | Walk | Judge | Fits engram? |
|---|---|---|---|---|
| 1 | Graph DB query (Cypher / SPARQL) | Pattern-match user-written queries | Query result is the answer | No — needs NL2Cypher, fragile |
| 2 | PathRAG / random walk | Sample N random/DFS paths | LLM scores or fuses paths | No — non-deterministic, LLM in hot path |
| 3 | Beam search over typed edges | BFS with top-K pruning per hop | Path score | Workable but symbolic-KG-style |
| 4 | GNN / pre-computed embeddings | Train GNN, embed neighborhood | Cosine similarity | No — graph is dynamic, no train data |
| 5 | Agent-style LLM exploration | LLM picks next edge each hop | LLM declares "done, answer is X" | No — non-deterministic, expensive |
| 6 | Subgraph extraction → LLM | Pull k-hop neighborhood, feed to LLM | LLM reads and answers | No — explodes context, non-deterministic |

All 6 share a hidden assumption: **retrieval is a search over a static
read-only graph**. Query in, paths out, ranking applied. None of them
treat the graph as a substrate on which something dynamic happens.

This is the assumption potato's intuition rejected. Not because the
algorithms are bad — beam search is the standard answer in classical
KG-QA and works fine for that — but because it does not match the goal
of engram, which is to model **how brains do recall**, not how
graph databases do query optimization.

## 3. How human memory actually handles "multi-hop"

Worked example: "what was the name of the TA in Pan-laoshi's class?"

What happens in a human brain (simplified, but neuroscientifically
grounded):

1. **"Pan-laoshi" activates directly.** This is recognition, not search.
   No traversal of "all professors I know followed by filtering." The
   neural assembly that *is* Pan-laoshi's representation lights up.
2. **Activation spreads to neighboring concepts.** The course, the
   semester, the classroom, assignments, classmates — all get weakly
   activated as a side effect of Pan-laoshi being active. This is
   *spreading activation*, well-documented in cognitive psychology
   (Collins & Loftus 1975, ACT-R, etc.).
3. **The query cue "TA" also activates.** Concepts associated with
   "teaching assistant role" light up in parallel.
4. **Intersection emerges by superposition.** Where Pan-laoshi's
   activation cloud overlaps with the TA-concept cloud, some specific
   memory becomes super-activated and "pops up" into consciousness.
5. **If nothing pops up, strategic search begins.** "Who sat next to me
   that semester?" "Who graded the homework?" — querying related
   contexts to inject more activation from a different angle.
6. **If still nothing, tip-of-the-tongue state.** You know it exists,
   you know its semantic neighborhood, but the name itself fails to
   activate above threshold.

Key properties of this process:

- **There is no "decide how many hops"**. Activation decays naturally;
  weak nodes drop below threshold and are forgotten this round.
- **There is no "list of candidate paths"**. What surfaces, surfaces.
  What doesn't, doesn't exist as far as conscious recall is concerned.
- **Judgment is not "score the path"**. It is **convergence of multiple
  cues on a common neighbor** — a resonance, a "yes, that's it" signal.
- **The process is asynchronous, parallel, continuous**. Not BFS-style
  layered traversal.
- **Failure has structure**. Tip-of-the-tongue is not "no result"; it
  is partial activation succeeding for context but failing for the
  target. The brain *knows it doesn't know*.

The crucial insight: **the brain does not have a "multi-hop" concept**.
What we call multi-hop is just activation that, after several synaptic
steps of decay, still reaches threshold at the target. If synaptic
weights are strong and the target cue is also contributing activation,
even a 5-step chain feels instant. If a link is missing, even a 1-hop
recall fails. **Hop count is not the control variable; activation
strength is.**


## 4. Why engram v0.2 didn't have a multi-hop problem

v0.2 architecture:

- Memory unit = opaque text blob
- Recall = embedding similarity + ACT-R activation decay
- Intent classifier routes to single-step plans
- Hebbian links connect co-activated memories

v0.2 was already an **activation-field model**. Recall succeeded when
some memory's ACT-R activation crossed threshold; failed otherwise. There
was no "path" between memories — just activation strength. So
"multi-hop" was structurally absent: there were no edges to walk.

The problem with v0.2 was not the activation model. It was that the
**substrate was too thin**. Embedding similarity over text blobs gave
ACT-R weak signals to work with:

- No entity concept — "Alice" and "her husband" did not resolve to a
  shared anchor
- Hebbian links were noisy because any two co-occurring memories got
  connected, drowning real signal in correlation
- No way to distinguish "X is married to Y" (strong, durable) from
  "X mentioned Y once" (weak, accidental)

Recall was bad because the substrate was too poor for the activation
model to do useful work, **not because the activation model was wrong**.

## 5. Why v0.3 created the multi-hop problem — and what we did right anyway

v0.3 introduced a **typed graph**: entities, typed predicates,
bi-temporal edges, entity resolution. This was the right move:

| v0.2 weakness | v0.3 fix |
|---|---|
| "Alice" and "her" not unified | Entity resolution → canonical entity |
| Relations only implicit in embeddings | Typed predicates (MarriedTo, WorksAt, ...) |
| Weak vs strong association indistinguishable | Edge confidence + predicate semantics |
| No temporal validity | Bi-temporal edges |

But v0.3 retrieval was structured as a **collection of plans, each of
which queries the graph as a database**. Factual plan does an entity
anchor + 1-hop edge fetch. Episodic plan does a time-window scan.
Affective plan does a sentiment filter. Each plan is independently
written, treats the graph as a SQL-like data source, and produces a
ranked list of candidate memories.

This works for single-step recall. It does **not** work for multi-hop,
because there is no plan whose job is "follow a chain of edges." Adding
one (the original ISS-070 V1 beam search proposal) was the obvious next
step **within this paradigm**.

But the paradigm itself is the issue. v0.3's plan-as-database-query
structure threw away v0.2's activation-field model. The multi-hop gap
is a symptom of the bigger issue: **v0.3 has the right substrate but
the wrong traversal model**. Beam search would patch the symptom
without fixing the underlying mismatch.

**What v0.3 did right and we keep:**

- Graph schema (entity, edge, predicate, bi-temporal)
- Entity resolver
- Storage layer (graph_entities, graph_edges, FTS)
- Resolution pipeline
- Retrieval orchestrator + plan dispatcher framework
- Existing single-step plans (Factual, Episodic, Affective, Bitemporal,
  Abstract, Associative, Hybrid)

**What we re-think:**

- Multi-hop, which never had a working plan
- The internal implementation of existing plans (probably becomes
  "activation injection strategies" rather than "independent SQL-like
  queries"), but the plan dispatcher + orchestrator framework stays
  and is the right level of abstraction

This is **not a v0.3 rewrite**. It is a re-framing of one layer
(retrieval traversal) within v0.3.

## 6. Substrate vs traversal: the framing that resolves the confusion

The confusion that made the 6 standard approaches feel wrong came from
collapsing two orthogonal dimensions:

**Substrate** = the data structure that stores the relational memory:

- Blob memory (v0.2) — text blocks with embeddings
- Typed graph (v0.3, current) — entities, typed edges, bi-temporal
- Vector index — points in embedding space
- Document store — nested JSON
- Dense neural network — knowledge in weights

**Traversal** = the algorithm that uses the substrate to answer a query:

- Similarity search (kNN over embeddings)
- Graph walk (BFS / DFS / beam search over edges)
- Spreading activation (energy injection + decay-driven diffusion)
- Pattern matching (Cypher-style declarative)
- LLM-driven exploration (model picks next step)

Substrate and traversal are **independent choices**. Any substrate can
host multiple traversals; any traversal applies across multiple
substrates with adjustments.

The 6 "wrong" approaches in §2 are wrong not because they pick a bad
substrate (they all use some kind of graph) but because they pick a
**graph-walk traversal** when the engram goal calls for a
**spreading-activation traversal**.

v0.2 did spreading activation correctly but on a thin substrate.
v0.3 upgraded the substrate but switched the traversal to graph-walk
(plan-as-query). The right combination is **v0.3 substrate + v0.2-style
activation traversal** — taking the strengths of both.

This framing is what unblocked the design conversation. It is also why
"just add a multi-hop plan" is the wrong question to ask.

## 7. KG as substrate: necessary, but not the same as "modeling the brain"

A separate confusion potato raised: "we want to learn from the brain,
so we have to use a knowledge graph, right?"

The answer requires precision because mixing these up causes
over-engineering or under-engineering later.

**True**: any associative memory system, mathematically, is a graph.
"A is related to B" is an edge; concepts are nodes. Every alternative
(vector index, SQL tables, document store, dense neural network) is
either a special case of a graph or an information-theoretic
substitute that loses something:

- Vector index = complete graph with cosine-similarity weights, no
  edge typing
- SQL = relational graph (foreign keys are edges), schema-rigid
- Document store = tree (graph special case), no cross-references
- Dense neural network = continuous relational structure, but
  black-box and not incrementally updatable

So the substrate **must be a graph**. That part is forced by the
mathematics of associative memory.

**False, or imprecise**: "the substrate must be a knowledge graph
specifically." Knowledge Graph (KG) in the industry sense means
something stronger:

- Explicit schema (Wikidata, Freebase)
- Hand-curated or semi-automatic construction
- Strongly typed entities and relations
- Used for declarative query and reasoning

The brain is **not** a KG in this sense:

- No explicit schema
- Nodes are activation patterns, not discrete entities
- Edges are synaptic weight distributions, not labeled relations
- The same "concept" recruits different neuron sets in different
  contexts
- Edge "types" are functionally emergent, not pre-defined

So the precise statement is:

- ✓ Substrate must be a graph (nodes + edges) — mathematical necessity
- ✗ Substrate need not be KG-style — KG is one engineering instance
- ✓ engram using KG-style as a starting point is a sound engineering
  compromise — we need serializability, queryability, debuggability,
  and a discrete schema fits SQLite, version control, and
  introspection

**The brain achieves "structure is meaning" because it had hundreds of
millions of years of evolution to grow a substrate where activation
patterns directly encode semantics. We do not have that time, and we
do not need it for the agent-memory use case.** What we need is to
model the dynamics of activation spreading on a discrete substrate.
The substrate being KG-shaped is an engineering choice, not a
biological claim.

## 8. Mapping brain → engram: the operational analogy

This table captures the engineering-level analogy between brain
machinery and engram constructs. It is not a claim that engram *is*
brain-like in any neuroscientific sense. It is an analogy that drives
implementation decisions.

| Brain | engram v0.3 |
|---|---|
| Neuron | entity / memory node |
| Synapse | typed edge |
| Synaptic weight | edge confidence × predicate conductance |
| Neuron firing | node activation level (ACT-R) |
| Activation propagation | spreading activation along edges |
| Excitatory vs inhibitory synapse | predicate type (MarriedTo positive, Contradicts negative) |
| Short-term memory | working set / recent activation state |
| Long-term memory | persisted graph |
| Concept formation | entity resolution (multiple mentions → canonical entity) |
| Association | Hebbian link / Proposed predicate |
| Tip-of-the-tongue | sub-threshold activation reported as partial recall |
| Priming | residual activation carried across queries |
| Attention | query-conditional conductance modulation (future) |

This mapping is what justifies calling the resulting system
"brain-inspired." Not the substrate, not the implementation language,
but the **dynamics** — how activation enters, propagates, decays, and
surfaces as recall.

## 9. Spreading activation: the algorithm

### 9.1 Mathematical core

State: each node has activation `a_i ∈ [-1, 1]`. The graph state is
vector `a ∈ R^N` (N = node count, in practice held as a sparse map).

Edge weight matrix `W`, where `W[i][j]` is the conductance from j to i:

```
W[i][j] = predicate_conductance(edge.predicate)
       × edge.confidence
       × recency_factor(edge.created_at, query_time)
```

One step of diffusion:

```
a_new = decay_self * a + decay_propagate * (W * a)
a_new = clamp(a_new, -1.0, 1.0)
```

Iterate K times (with early-stop on convergence). Inject query anchors
at step 0 by setting their activation values directly. After K steps,
extract memories whose associated entity nodes have activation above
threshold; rank by activation value.

This is mathematically the same family as PageRank. The differences
from PageRank are query-driven injection (vs uniform) and edge typing
(vs unweighted hyperlinks).

### 9.2 Why this naturally handles multi-hop

There is no `max_depth` parameter in spirit. After K iterations:

- Step 1: activation reaches 1-hop neighbors of anchors
- Step 2: 2-hop neighbors get activation
- Step k: k-hop neighbors get some, weakened by `decay_propagate^k`

If `decay_propagate = 0.5`, a 3-hop neighbor receives `0.125 ×
edge_weights_along_path` of an anchor's activation. If the path edges
are strong (high confidence × high conductance), this can still cross
threshold. If the path is weak, it doesn't. **Hop count is not a
parameter — activation decay decides automatically.**

Multi-anchor queries (e.g. "Alice and Bob's common friend") are
handled by simultaneous injection. Both anchors' activation clouds
expand; common neighbors receive activation from both and become
super-activated. Set-intersection emerges from superposition without
special-case code.

### 9.3 Shape of the engineering implementation

```
ActivationEngine::run(anchors, query_time, config) -> ActivationState

  state = sparse map {node_id: activation}
  for each anchor: state[anchor.entity_id] = anchor.strength
  for step in 0..K:
    new_state = self_decay(state)
    for (node, activation) in state:
      if |activation| < pruning_threshold: skip
      for edge in graph.outgoing_edges(node, query_time):
        flow = activation
             × decay_propagate
             × predicate_conductance(edge.predicate)
             × edge.confidence
             × recency_factor(edge.created_at, query_time)
        new_state[edge.target] += flow
    clamp new_state to [-1, 1]
    if max(|new_state - state|) < epsilon: break (early stop)
    state = new_state
  return state

ActivationEngine::extract_memories(state, top_k) -> Vec<(memory_id, score)>

  for entity in state with activation > threshold:
    for memory linked via Mentions edge:
      memory_score[memory] += entity_activation × edge_weight
  return top_k by memory_score
```

Estimated size: ~150 lines core + ~50 lines extraction. Compared to
~600 LoC for the original ISS-070 V1 beam-search proposal, this is
both simpler and more general.

### 9.4 Why this subsumes the existing single-step plans (eventually)

Factual recall = inject one anchor, run 1-2 steps, extract memories.
Episodic recall = inject time-window context as anchors, run 1 step.
Affective recall = inject sentiment-tagged anchors, weight by valence.
Multi-hop = inject multiple anchors, run K steps.

All of these become **configurations of the same engine**, not
independent plans. The plan dispatcher becomes a dispatcher of
*activation injection strategies* rather than independent retrieval
algorithms. This is a longer-term cleanup, not part of the immediate
prototype.

## 10. Open questions answered in this discussion

This section captures the substantive design questions raised during
the conversation and the answers reached. Future reviewers reading
this document should treat the answers as the working position, not as
final decisions — the prototype may invalidate any of them.

### 10.1 Do we read node/edge semantics during spreading?

**Two layers, two answers:**

**Layer A — diffusion runtime: no.** The inner loop is pure numeric
computation. activation × conductance × confidence × recency. No
string comparison, no parsing of predicate labels, no understanding of
"what MarriedTo means." The semantic interpretation is encoded
*statically* in the conductance table.

**Layer B — conductance table construction: yes, offline.** Building
the predicate-conductance map (e.g. `MarriedTo → 0.8`, `Contradicts →
-0.5`, `RelatedTo → 0.2`) requires semantic judgment about which
predicates conduct activation strongly. This is done once, persisted
as a constant, and never re-evaluated at query time.

**Why this matters:** the brain analogy holds — neurons don't read
synapse labels at firing time. Synapses just have weights. The
"meaning" is in *which* synapses exist where, set during development
and learning. Our offline conductance table is the engineering
equivalent of "developmental wiring."

There is a deeper question about query-conditional conductance —
whether a query about marriage should boost MarriedTo edges
specifically. This is the brain phenomenon called *priming*. Two
implementation choices:

- **Approach 1 (recommended for now):** ignore query content at the
  edge level. Inject activation at multiple anchors and let
  superposition handle it. Simpler, fully deterministic, no NLU
  dependency.
- **Approach 2 (future):** parse query intent, modulate conductance
  per query. Closer to brain-like priming but adds NLU as a hot-path
  dependency.

The prototype will use Approach 1. If LoCoMo results expose specific
multi-hop questions where the right edge type is being drowned out by
generic activation flow, revisit.

### 10.2 Convergence vs fixed iterations — what does the brain do?

**The brain does neither.** The brain runs **continuous-time dynamics**:

```
dV/dt = -V/τ + Σ synaptic_input
```

Each neuron's membrane potential evolves continuously. Spreading does
not "iterate." It just happens, at the speed of synaptic transmission
(milliseconds per hop).

Spreading does not "stop" by reaching a halt condition. It stops
because:

1. **Energy depletion** — neurons can't fire indefinitely (ATP limits)
2. **Inhibitory feedback** — excitatory activity recruits inhibitory
   interneurons that suppress the network
3. **Spike-frequency adaptation** — sustained firing raises the
   threshold for further firing
4. **Loss of input** — without ongoing cue, the initial activation
   dissipates through leak

A typical recall episode lasts a few hundred milliseconds — roughly
tens of synaptic steps.

**Our engineering choice:** discretize. Why we *can*:

- We are doing ranking, not dynamics research
- Continuous simulation (small dt over real time) and discrete
  iteration (large dt as steps) give nearly identical relative
  rankings, even when absolute activation values differ
- The performance gap is large: dt = 0.001 sim of 1 second = 1000
  steps, vs K = 10 discrete steps. ~100× cost

**Our scheme:**

- Fixed upper bound K = 10
- Early stop if `max(|Δa|) < ε` (typically converges in 3-5)
- Anytime output: caller can stop at any iteration, get current
  ranking — uses our existing `BudgetController`

This is not "discretization because computers are discrete" (a lazy
answer). It is "discretization because for our use case the continuous
dynamics give us no extra value at large extra cost."

### 10.3 Inhibition, normalization, and sparsity in the brain vs in code

**Brain inhibition:** the brain has dedicated **inhibitory neurons**
(GABAergic, ~10-20% of neurons), whose activation reduces target
neurons' membrane potential. Two roles:

- **Feedback inhibition:** excitatory firing recruits inhibitory
  neurons that suppress the same population. Effect: automatic
  normalization. Total activation cannot run away to infinity.
- **Feedforward inhibition:** input simultaneously drives both
  excitatory and inhibitory pathways, with inhibition suppressing
  weaker downstream targets. Effect: contrast enhancement /
  winner-take-all.

The brain's inhibition is **structural, not algorithmic** — certain
neurons are biologically inhibitory by type.

**Our equivalents:**

- **Negative-weight edges:** `Contradicts` predicate gets conductance
  = -0.5. Activation flowing through these edges becomes negative,
  suppressing the contradicted target. This corresponds to
  inhibitory synapses.
- **Self-decay (`decay_self < 1`):** activation naturally fades
  unless replenished. Approximates leak / energy depletion.
- **Clamp [-1, 1]:** prevents runaway. Approximates physiological
  saturation.
- **Pruning threshold:** drop nodes with `|activation| < ε` from the
  active set. Approximates sub-threshold neurons being
  computationally absent.

**Choices we did not pick and why:**

- **Softmax normalization (per step):** prevents runaway and
  enhances contrast, but loses absolute activation magnitude. We need
  absolute magnitude to detect "tip-of-the-tongue" (low overall
  activation = recall failure, return empty + partial-recall
  signal). Softmax would mask this.
- **Sum normalization:** same problem, different math.

**Sparsity:**

The brain has ~86 billion neurons, each connected to ~7,000 others.
Connection density ≈ 10⁻⁹. Extremely sparse.

Spreading is therefore inherently local — activation can only flow to
direct neighbors, of which there are thousands at most, not billions.

**engram graph is similarly sparse**: each entity has at most a few
dozen edges in practice. Implementation must reflect this:

- **Never instantiate W as N×N**. Memory blows up at N≈10⁴.
- **Sparse map representation:** active set held as
  `HashMap<NodeId, f32>`. At step 0 the set has the anchors only
  (1-3 nodes). After K steps it has at most all K-hop neighbors
  reached above threshold (typically hundreds).
- **Iterate over active set, fetch outgoing edges per node.** Time
  complexity per step: O(active × avg_edges_per_node). Active is
  small, edges per node is small, so each query is milliseconds.

Sparsity also makes the search space self-limiting. Beam search
imposes a hard width K to keep search bounded. Spreading activation
gets the same bound *for free* via decay-driven natural pruning.

### 10.4 Can we replicate "structure is meaning" in code?

This was the deepest question raised. The brain has no separate "label
storage" for concepts — the activation pattern of a specific neural
assembly *is* the concept. There is no field saying `name = "Alice"`;
the concept is the firing pattern.

**Theoretically, yes — modern neural networks already do this.**
Transformer-based LLMs encode concepts as activation patterns over
millions of neurons. There is no "Alice" symbol anywhere; the model
"knows" Alice because certain attention heads and feed-forward layers
respond to Alice-related context in characteristic ways. This is
exactly "structure is meaning."

**Engineering-wise, we should not.** Four hard constraints push us
toward explicit symbolic storage:

1. **Traceability and explainability.** Agent memory needs to answer
   "why do I think Alice is married to Bob?" with "this March 2024
   conversation said so." Black-box weight-encoded knowledge cannot
   be sourced. Every fact must be locatable.
2. **Incremental learning.** Agent memory must absorb a new fact in
   seconds and use it in seconds. Neural-net knowledge requires
   re-training (expensive) or in-context provision (non-persistent).
   Symbolic storage allows insert-and-use.
3. **Serializability.** engram persists to SQLite, dumps for
   inspection, version-controls graph snapshots. Symbolic storage
   serializes trivially. Neural-net weights are gigabytes of
   black-box numbers tied to one specific model.
4. **Cost and latency.** Symbolic graph queries run in milliseconds
   on CPU. Neural-net inference needs GPUs and incurs much higher
   latency.

**So engram occupies a deliberate middle ground:**

| dimension | pure neural | engram | pure symbolic KG |
|---|---|---|---|
| where is meaning | implicit in weights | partially explicit (labels + activation) | fully explicit (in labels) |
| explainable | barely | reasonably | fully |
| incremental update | hard | easy | easy |
| brain-similar in representation | yes | partially | no |
| brain-similar in dynamics | only if architected for it | yes (spreading activation) | no |
| engineering cost | very high | medium | low |

The engram position is not a "compromise" in the negative sense; it is
the **best fit for the use case**. Full neural is overkill and
unworkable for agent-memory constraints. Full symbolic is too thin to
host brain-like dynamics.

A meta-observation: engram is not trying to *be* a brain. It is trying
to **mimic the functional properties of brain memory** that matter for
agents:

- Associativity (concepts pull related concepts)
- Decay (unused things fade)
- Co-activation strengthening (Hebbian)
- Multi-cue convergence (multiple signals stack)
- Failure with structure (you know when you don't know)
- Concept formation (multiple mentions merge)

These six functional properties can be achieved on a symbolic
substrate via spreading activation, ACT-R decay, Hebbian links, and
multi-anchor injection. We don't need biological accuracy; we need
functional equivalence.

### 10.5 Why must we discretize?

This was already touched in §10.2 but is worth stating directly:
**we are not forced to discretize by hardware**. Modern neuron
simulators (NEURON, Brian2) discretize at dt = 0.0001 to 0.001 seconds
to faithfully simulate continuous dynamics. We *could* do that.

We choose not to because:

1. **The use case is ranking, not dynamics study.** We care about
   "which memories surface," not "what is the firing rate of node X
   at time t."
2. **For ranking, large discrete steps and small continuous steps
   give the same answer.** The relative order of activated nodes is
   stable across step sizes; only the absolute values change.
3. **Performance gap is large.** Continuous simulation at dt=0.001
   for 1 second of biological time = 1000 steps. Discrete K=10 is
   100× cheaper.
4. **Continuous simulation's complexity is real.** ODE solver choice,
   numerical stability, stiffness, integration method — these are
   research-grade engineering problems that buy us nothing.

**One caveat for the future:** if engram ever wants to model
short-term-memory holding multiple concurrent concepts via gamma /
theta oscillation binding, or attention switching via dynamic
inhibition rebalancing, *those phenomena require continuous time*.
They do not arise in discrete iteration. We are not doing this now,
but the door should be left open.

For ISS-070 / multi-hop, K=10 with early stop is correct.

## 11. Decisions taken

Synthesizing the above into actionable positions:

1. **ISS-070 V1 (beam search) is superseded.** The design.md V1
   remains in place as a historical artifact, marked superseded.
2. **Multi-hop is not a new plan.** It is what the spreading
   activation engine produces naturally when activation propagates K
   steps from anchors.
3. **Spreading activation is the unifying retrieval primitive.**
   Existing single-step plans become activation-injection strategies
   over time — but this migration is *not* part of the prototype scope.
4. **Substrate stays as v0.3 typed graph.** No schema changes, no
   storage rework, no migration.
5. **Algorithm parameters (initial defaults, subject to prototype):**
   - K = 10 (max iterations)
   - early-stop ε = 0.01 (max change threshold)
   - decay_self = 0.7
   - decay_propagate = 0.5
   - pruning_threshold = 0.05
   - activation range: [-1, 1] (allow negative for inhibition)
6. **predicate_conductance defaults (initial guess):**
   - `MarriedTo`, `WorksAt`, `BornIn`, `LivesIn`, `Authored`,
     `MemberOf`, strong typed predicates: 0.8
   - `Mentions`: 0.6 (entity ↔ memory bridging)
   - `Knows`, `Met`, weaker typed predicates: 0.5
   - `RelatedTo`: 0.2 (deliberately weak — generic fallback)
   - `Proposed(_)`: 0.3 (uncertain — discovered, not canonical)
   - `Contradicts`: -0.5 (inhibitory)
   - `MentionedIn` (entity → episode): 0.1 (provenance, not semantics)
7. **Anchor injection is multi-source.** EntityResolver returns N
   anchors, all are injected with their match strengths. Multi-hop
   query intent is detected by `anchors.len() ≥ 2` plus textual
   heuristics (deferred — prototype only handles LoCoMo
   `multi-hop`-tagged queries).
8. **Bench-mode hint routing** (`locomo_category == "multi-hop"`)
   should be gated behind a feature flag `bench-routing-hints` so
   production builds do not carry bench logic.
9. **Failure mode is a first-class output.** If no node exceeds
   activation threshold at the end, return empty result + partial
   recall signal containing the top-K sub-threshold candidates. This
   corresponds to tip-of-the-tongue.
10. **Continuous-time dynamics are out of scope** for this iteration
    but recognized as relevant for future short-term-memory work.

## 12. Next step: prototype scope

A standalone Rust binary, **not integrated into the retrieval
pipeline yet**, that implements the activation engine and runs it
against LoCoMo conv-26's multi-hop questions.

**Goal:** validate the approach on real benchmark data before
committing to integration work.

**Success criteria:**

- Hit@5 on the 3 multi-hop questions of conv-26: **≥ 1/3 (33%)**
  minimum, **≥ 2/3 (67%)** target
- Activation traces are inspectable per query: which anchors injected,
  which nodes activated at each step, final ranking
- Runtime per query < 100ms on the conv-26 graph subset

**Non-goals for the prototype:**

- Integration with `RetrievalOrchestrator`
- Refactoring existing plans to use the engine
- Production-grade configuration plumbing
- Unit tests beyond the algorithm core (integration tests deferred)
- Live-agent path (no `current_self_state`, no priming, no priming
  decay across queries)

**Deliverables:**

- `crates/engram-bench/examples/spreading_activation_prototype.rs`
  (or a similar binary path — final location TBD)
- ~150 LOC core algorithm + ~50 LOC graph loader + ~50 LOC
  scoring/reporting = ~250 LOC total
- A short report (could be appended to this discussion doc as §14)
  summarizing hit@5 results, activation traces for each question,
  observations, and parameter sensitivities

**Time estimate:** 1-2 days of focused work.

**Decision gate after prototype:**

- If hit@5 ≥ 33%: proceed to write a formal design doc for the
  activation engine and supersede ISS-070 V1 with a V2 design doc
  using this paradigm.
- If hit@5 < 33%: investigate whether failure is in algorithm,
  parameters, or substrate (edges missing from the graph). Do not
  pivot to a different retrieval paradigm without first explaining
  the failure.

## 13. What this means for ISS-070

ISS-070 stays open. Its scope changes:

- **V1 design.md** stays in place, marked superseded. Useful as a
  reference for what was rejected and why.
- **V2 design.md** will be written *after* the prototype validates
  the spreading-activation approach. It will describe the integration
  path: how `ActivationEngine` becomes a retrieval primitive, how the
  classifier routes multi-hop queries to it, how it interoperates
  with existing single-step plans during the transition period.
- **The acceptance criterion stays the same:** LoCoMo conv-26
  multi-hop hit@5 ≥ 33% minimum, ≥ 67% target. The implementation
  path changes from "beam search plan" to "activation engine
  integrated as a new plan or as a routing target."
- **The wider implication** — that existing plans should eventually
  be re-expressed as activation-injection strategies — is *not* part
  of ISS-070. It is a follow-on discussion that should result in a
  separate issue once the prototype proves the paradigm.

This document is the source of truth for the rationale. ISS-070 V2
design and the eventual feature-level architectural change should
both link back here.

