# Performance experiments

This is a log of the speed hypotheses tried on the CPU pixel loop, how they
were measured, and what was kept. The CUDA engine (`fracst-cuda`) is a
line-by-line port of the loop described here; see [remote.md](remote.md). Each hypothesis was developed on its own branch
in its own git worktree, then measured against the same baseline.

## Method

- **Workload.** The `bench` example renders six generated locations at
  1920×1080 on all 16 threads of a Ryzen 7 8845HS: an overview, a shallow
  minibrot, spirals at 1e238, 1e397 and 1e1000, and the inside of a minibrot
  at 1e400 with a 1M iteration limit.
- **Interleaving.** Timings on this machine swing by up to ±30% with clocks
  and temperature. Every comparison therefore ran all variants round-robin,
  alternating the order each round, and kept each variant's best time per
  location over 3–4 rounds. Variants that ran identical code still differed
  by up to ±10%, which is the noise floor.
- **No overlapping runs.** Nothing else heavy ran during measurements. The
  agents that wrote some variants did not benchmark; they only built and
  tested.
- **Correctness.** Every variant had to pass the accuracy suite, which
  checks against brute-force iteration. Where a change promised identical
  output, a test compares frames bit for bit.

## Results

Speed-ups are against the engine at v0.1.0, as geometric means over the six
locations, with notable rows called out.

| Hypothesis | Idea | Result | Kept |
|---|---|---|---|
| **A** FMA, CPU-specific builds | Fused multiply-adds shorten each step's dependency chain | FMA on x86-64-v3: +12–14% on `f64` frames, −5% on deep frames. `target-cpu=native` alone: +2% | Yes, as runtime dispatch for the `f64` loop only |
| **B** Pixel interleaving | Advance 2–4 pixels per thread to hide latency | 25–50% *slower*. The loop is limited by how many instructions it issues, not by waiting on results, and SMT already pairs pixels per core | No |
| **C** Extended-range overhead | Precomputed CExp orbit; an exact fast path that skips escape and rebase tests where they provably can't fire; longest-first BLA search | ×1.24–1.63 on deep frames, bit-identical | Yes |
| **D** BLA table layout | Structure of arrays, fewer levels, other search orders | SoA is a net regression (×0.87 with the old search order). Top-down search is +4–7% on BLA-heavy frames but −7% on frames where lookups fail | Only the idea, as a combined lookup |
| **D′** Combined lookup | Test the shortest step first (failures stop after one compare), then search down from the longest | +19% on the 1e238 spiral, same results | Yes |
| **E** AVX2 SIMD | Four `f64` pixels per step. Lanes with anything to decide take that step through the scalar loop, so output is bit-identical | ×1.40 over scalar on the long shallow frame. First version was 2× slower on short-orbit and BLA-heavy frames; fixed with a scalar "drain" mode for event lanes and a per-frame gauge that turns SIMD off when events cost more than they save | Yes |

All kept changes together, against v0.1.0:

| Location | Speed-up |
|---|---:|
| Overview | 1.00× |
| Seahorse minibrot 3e8 | 1.79× |
| Spiral 1e238 | 1.23× |
| Spiral 1e397 | 1.19× |
| Inside a minibrot 1e400 | 1.49× |
| Spiral 1e1000 | 1.38× |
| **Geometric mean** | **1.33×** |

## What's left

- **Deep frames** are now bounded by the BLA steps themselves and the `f64`
  phase. A SIMD version of the extended-range phase is possible, but most
  deep pixels take only a few dozen steps.
- **AVX-512** would allow eight lanes on CPUs that have it. It is worth
  trying, using the same event design.
- **Interior detection** costs roughly 10–45% per step on exterior-heavy
  frames that don't need it. A cheaper check, or enabling it only when the
  previous frame had interior pixels, could win that back.
