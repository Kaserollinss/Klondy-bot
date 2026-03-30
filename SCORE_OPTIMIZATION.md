# Score Optimization: Solve First, Otherwise Get as Close as Possible

## Objective

The repo should prioritize **finding a full solution first**. If a game cannot be solved within the current search budget, it should return the **best legal fallback line** it can find: a sequence of moves that makes the strongest progress toward a win and can be followed by a human in standard Klondike terms.

In short:

1. Try to solve the game exactly.
2. If an exact solution is found, output that line.
3. If no exact solution is found within budget, output the best fallback line.
4. Always emit moves in a **human-followable order**.

## Important Clarification

We should not treat these cases as the same thing:

- **Solved**: an exact winning line was found.
- **Proved unsolvable**: the exact solver exhausted the search space and proved there is no win.
- **Not solved within budget**: the search stopped because of time/visit limits, so we need a best-effort fallback.

That distinction matters for both correctness and UX. Most hard positions should probably be described as "no full solution found within budget" unless the exact solver actually proves unsolvability.

## Why The Current Binary Reward Is Not Enough

Right now the HOP/MCTS path is binary:

`is_win()` -> `SearchResult::Solved` -> `wins += 1` -> `wins / played`

That is useful for estimating "does this move ever win?", but it is not good enough for "what should I do if I cannot fully solve this game?". A rollout that reaches 48 foundation cards and a rollout that stalls immediately both count as 0 if neither fully wins.

For the fallback objective, we need a reward that values **partial progress**, not just final victory.

## Proposed Product Behavior

The desired high-level flow should be:

### Phase 1: Exact solve attempt

- Run the existing exact solver from the current position.
- If it returns `Solved`, output the exact winning line and stop.
- If it returns `Unsolvable`, report that the game appears unwinnable and optionally still offer the best progress line.
- If it returns `Terminated` or otherwise fails to finish within budget, switch to fallback search.

### Phase 2: Best-effort fallback

- Use score-guided search to choose a line that gets as close as possible to winning.
- Score candidates by **progress toward a win**, not just binary solve rate.
- Keep the selected line legal and convertible into standard human-playable moves.

### Output requirement

- The final output should be an ordered move list.
- The output should use **standard Klondike moves**, not just internal optimized moves.
- A human should be able to replay the line from the initial position.

## Recommended Fallback Objective

Use **foundation card count measured at rollout terminal states** as the reward signal.

Each rollout already plays to a terminal state (win or dead end). Instead of recording only binary win/loss, read `stack.len()` at the terminal state and use that as the rollout reward. This naturally handles the "foundation count alone can be shortsighted" problem: if stacking a card early leads to a dead end at 12 foundation cards, but not stacking it leads to a dead end at 30, the terminal-state measurement captures that difference without any heuristic tuning.

This is sufficient for the first implementation. Additional signals (hidden cards revealed, mobility, line length) can be added later as tiebreakers if needed, but terminal-state foundation count is the right starting point because:

- It requires minimal code changes (read `stack.len()` at rollout end).
- It naturally penalizes trap positions via the rollout playing them out.
- It avoids fragile weight tuning between multiple heuristic signals.
- It is much better than binary win/loss, and it matches the current architecture well.

## What Already Exists In The Repo

- The exact solver already exists in `src/solver.rs`.
- The HOP/MCTS-style move picker already exists in `src/hop_solver.rs`, `src/mcts_solver.rs`, and `lonecli/src/main.rs`.
- The repo already has a conversion layer from internal optimized moves to standard human-followable moves in `src/convert.rs`.

That means this project does **not** need a brand-new solver architecture. It needs:

- a clearer solve-first execution flow,
- better fallback scoring,
- and a better final output contract.

## Implementation Work Needed

### 1. Define the control flow explicitly

The current HOP flow repeatedly picks moves until the game is won or lost. To support the desired behavior cleanly, the repo should make the decision pipeline explicit:

1. Attempt exact solve from the current state.
2. If solved, return the exact line.
3. If not solved within budget, call fallback move selection.
4. Continue fallback selection until no more useful progress is available or a win is reached.
5. Return the accumulated line.

This should likely live behind a dedicated top-level entry point instead of being implicit inside the current `hop` loop.

### 2. Add a budgeted exact-solve entry point

The repo already has exact solve logic in `src/solver.rs`, including `solve_with_tracking`, but the new behavior needs a version that can stop cleanly under a configured budget and report that outcome upstream.

Needed changes:

- Add or expose a solve path that accepts a terminate signal or explicit budget.
- Use that path for the "solve first" phase.
- Distinguish between:
  - exact solve finished and found a win,
  - exact solve finished and proved no win,
  - exact solve stopped because the budget expired.

Without this, "try to solve it first" risks meaning "run an unbounded exact search", which is not practical for difficult games.

### 3. Replace binary rollout scoring with progress scoring

This is the core optimization.

### `src/stack.rs`

- Expose foundation count cleanly if needed.
- `Stack::len()` already exists and is enough for internal use.

### `src/hop_solver.rs`

- Extend `HOPSolverCallback` so each rollout records the **best foundation count reached**.
- On `on_win`, record `52`.
- On `on_visit`, when traversal terminates due to visit budget, retain the current foundation count instead of throwing it away.
- Update `HopResult` to store total score, not just total wins.
- Aggregate score for **every** rollout, including partial ones.

Possible shape:

```rust
pub struct HopResult {
    pub total_score: usize,
    pub skips: usize,
    pub played: usize,
}
```

### `src/mcts_solver.rs`

- Update move selection to use `total_score / played` instead of `wins / played`.
- Keep exploration, but rebalance exploitation around average progress.

### `lonecli/src/main.rs`

- Update `ucb1` so the exploitation term reflects average foundation cards, ideally normalized to `[0, 1]`.
- Re-tune the exploration constant after normalization.

### 4. Preserve exact solving as the top priority

The score optimization should be used for the fallback path, not as a replacement for exact solving.

Needed changes:

- Add a top-level mode that calls `solve` first.
- Only fall back to MCTS/HOP guidance when exact solve does not produce a full line within budget.
- Avoid re-running the full exact solver before every single fallback move unless benchmarks prove it is cheap enough.

Recommended first version:

- Run exact solve once from the current state.
- If it fails to produce a full line within budget, switch to fallback mode.
- Optionally retry exact solve after major progress events, such as a newly revealed card or a large jump in foundation count.

This avoids the worst runtime blow-up while still keeping "solve first" as the real objective.

### 5. Track and return the best fallback line, not just the next move

The current move picker is optimized around choosing a move sequence to apply next. The new objective requires the system to return the **best line discovered so far** when a full solve is unavailable.

Needed changes:

- Keep the best-scoring state/line found during fallback search.
- Define what score belongs to a line:
  - primary: foundation count
  - secondary: revealed cards, mobility, etc. later if needed
- Make sure the caller can retrieve the best line even when no win is found.

This is a product requirement, not just a scoring tweak.

### 6. Output standard human-followable moves

This repo already has the key building block in `src/convert.rs`.

Needed changes:

- Decide that the public output contract is `StandardMove` sequence, not internal `Move` sequence.
- Convert the chosen internal line before printing or saving it.
- If conversion fails, treat that as a bug in the output pipeline.

This is important because the README already notes that some optimized internal actions can look unusual from a human perspective.

### 7. Add budgets and result types that match the real behavior

To make the feature usable, the solver should expose result types that reflect what actually happened.

Recommended result categories:

- `Solved(line)`
- `ProvedUnsolvable(best_line_optional)`
- `BestEffort(line, score)`
- `Terminated(best_line_optional)`

Even if the exact enum names differ, the behavior should distinguish:

- proven no-win,
- no-win-found-yet,
- and best-effort progress output.

## Runtime / Complexity Risks

The idea is good, but there are real cost tradeoffs.

### Main risk: solve-first can be expensive

If "solve first" means "run a full exact search before every fallback decision", runtime may become unacceptable on hard seeds.

Safer approach:

- exact solve once up front,
- fallback thereafter,
- optional exact re-checks only at milestones.

### Main risk: foundation count alone can be shortsighted

Foundation progress is a strong signal, but Klondike has traps where pushing cards up too early can hurt future mobility.

Mitigation: measure foundation count at rollout **terminal states**, not mid-search. Rollouts already play to a dead end or a win. If an early stack move creates a trap, the rollout will dead-end at a low foundation count, naturally penalizing that path. This avoids the need for separate heuristics to detect traps. Additional tiebreaker signals (hidden cards revealed, mobility) can be added later if terminal-state foundation count proves insufficient.

### Main risk: output quality vs. search cost

Returning the "best line" is more expensive than returning the "best next move", because the search must preserve and compare entire candidate paths.

This is still worth doing, but it should be acknowledged as a scope increase.

## Suggested Implementation Order

1. Keep exact solve as the first attempt from the starting state.
2. Replace binary rollout reward with foundation count measured at rollout terminal states.
3. Return the best fallback line found, not just a local best move.
4. Convert fallback output into `StandardMove` form.
5. Benchmark on easy, medium, and hard seeds.
6. Add secondary tiebreaker signals (hidden cards revealed, mobility) only if terminal-state foundation count alone is not good enough.

## Success Criteria

This work is successful if:

- exact wins are still found and returned when available,
- hard or time-limited games return a strong fallback line instead of just "Lost",
- the fallback line is legal and human-followable,
- and runtime remains practical for CLI use.

## Non-Goals For The First Iteration

- Perfectly modeling all aspects of Klondike position strength
- Replacing the exact solver
- Proving optimality of the fallback line
- Designing a fully general heuristic evaluator

The first goal is simpler: keep exact solving intact, and make the failure mode dramatically more useful.
