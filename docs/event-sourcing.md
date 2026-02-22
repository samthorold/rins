# Event-Sourcing Architecture

This document is the authoritative reference for how event sourcing is applied in `rins`. It covers the log contract, aggregate boundaries, handler rules, mutable-state policy, incremental-replay patterns, and invariant placement. Read this before adding agents, derived views, or analytics.

---

## §1 The Log as Ground Truth

`Simulation.log: Vec<SimEvent>` is append-only and immutable during dispatch. Once an event is pushed to the log it is never modified or removed. The log is the single source of truth for what happened; all agent state is derived from it.

**What belongs in the log:** domain facts that change market state — things another agent or analyst would need to reconstruct the market's history.

| Belongs | Does not belong |
|---|---|
| `PolicyBound` | `next_policy_id` counter |
| `ClaimSettled` | Config parameters |
| `YearStart` | Transient scheduling details re-derivable from config |
| `QuoteAccepted` | RNG state |
| `PolicyExpired` | Internal cursor positions (e.g. round-robin index) |

**Implicit sequence numbers:** `log[i]` has implicit sequence number `i`. This is a stable, tested invariant (`log_is_day_ordered` test). Code that needs a stable position in the log may use the Vec index directly — do not add a `seq` field to `SimEvent` until the first `AggregateCursor` is built (see §5).

**Same-day ordering:** Within a single day, the order between events is not guaranteed and must not be relied upon. Handlers must be written so their correctness does not depend on same-day event ordering.

---

## §2 Aggregates and Aggregate Roots

An *aggregate* is a cluster of state that changes atomically in response to a single event, identified by a typed root ID. Each aggregate enforces its own invariants; no other agent modifies its state directly.

| Aggregate | Root type | State it owns |
|---|---|---|
| `Insurer` | `InsurerId` | `capital`, `rate`; capital invariants |
| Policy lifecycle | `PolicyId` | `pending_policies → policies` transition; `remaining_asset_value` per (policy, year) |
| `Broker` | *(singleton)* | `pending` submission map; round-robin cursor |
| `Insured` | `InsuredId` | acceptance decision; future GUL accumulation |
| `Market` coordinator | *(not an aggregate)* | Routes events; owns cross-aggregate policy maps |

**Boundary rule:** if two fields must be consistent after a single event, they belong to the same aggregate. If they can change independently, they may be separate aggregates.

**The coordinator is not an aggregate.** `Market`'s role is to route events and hold cross-aggregate maps (e.g. `policies`, `pending_policies`). It makes no pricing decisions and holds no state that can be modified by a single business fact. When in doubt, ask: "does this state belong to one typed entity?" If yes, it is aggregate state. If it coordinates multiple entities, it is coordinator state.

---

## §3 Handler Contracts

Every `on_*` method must satisfy all of the following:

1. **Own-aggregate mutation only.** A handler mutates only its own aggregate's state. Cross-aggregate effects are expressed by returning new events.
2. **Return, never push.** Handlers return `Vec<(Day, Event)>`; they never push directly to the simulation queue.
3. **No panics on unknown IDs.** If a handler receives an event referencing an ID it does not own or recognise, it returns `vec![]` and continues. Use `debug_assert!` for ordering invariants that should never fire in production.
4. **No side effects outside `self` and the returned list.** No I/O, no global state, no `println!` in production paths.
5. **Explicit RNG.** Accept `rng: &mut impl Rng` explicitly. Never call `thread_rng()` or any ambient source.
6. **Own the day arithmetic.** Each handler is responsible for the `day + N` offset of every event it schedules. Document those offsets in `docs/event-flow.md` whenever they change.

---

## §4 Mutable State Policy

Mutable in-agent state is acceptable when:

- Full replay at query time would be impractical (e.g. EWMA loss ratios that accumulate over thousands of events).
- The value is deterministically re-derivable from config + event slice (counters, capital balances).

**"Reconstructible from event slice"** means: given all events in `Simulation.log` where this aggregate is producer or consumer, plus the initial config, replaying them in dispatch order yields the same field values as the live agent. Two concrete examples:

- `Insurer(id).capital`: replay all `YearStart` events for this insurer (resets capital to `initial_capital`) and all `ClaimSettled { insurer_id }` events (decrements capital by `settled_amount`). The result must equal the live `capital` field.
- `Market.policies`: replay `QuoteAccepted` (create pending entry) + `PolicyBound` (move to active) + `PolicyExpired` (remove). The result must equal the live `policies` map.

**Fields that are intentionally not reconstructible** must be documented as such in the code:

- `ChaCha20Rng` state — by design. Reproducibility is achieved by replaying from the same seed, not from the log. Document this on the `rng` field.

When adding a new mutable field, ask: "could I reconstruct this by replaying the log?" If yes, write a test that does so. If no, document why reconstruction is impractical and what the recovery path is.

---

## §5 Sequence Numbers and Incremental Replay

**Today:** `log[i]` is the implicit sequence number. There is no `seq` field on `SimEvent`.

**Do not add a `seq` field yet.** The Vec index is sufficient for all current use cases. Adding it would change the NDJSON serialisation format and break analysis scripts without providing any benefit until a cursor is needed.

**Pattern to adopt at first derived view or cursor:**

```rust
struct AggregateCursor<T> {
    last_seen_seq: usize,
    state: T,
}

impl<T> AggregateCursor<T> {
    fn advance(&mut self, log: &[SimEvent], handler: impl Fn(&mut T, &SimEvent)) {
        for ev in &log[self.last_seen_seq + 1..] {
            handler(&mut self.state, ev);
            self.last_seen_seq += 1;
        }
    }
}
```

**When to adopt:** at the first `AggregateCursor` implementation, add `seq: usize` to `SimEvent` and assign it in `Simulation::run` before pushing to the log. The migration is mechanical — one field, one assignment, update NDJSON analysis scripts.

**Use cases:** year-over-year analytics, relationship score matrices, future checkpointing, test assertions on derived views.

---

## §6 Invariant Enforcement

Invariants belong in the handler that owns the aggregate, not in the caller. The coordinator delegates unconditionally — it never guards against an invariant that the aggregate should enforce itself.

| Invariant | Handler | Mechanism |
|---|---|---|
| Policy loss-eligible only after `PolicyBound` | `Market::on_policy_bound` | `pending_policies → policies` move; `on_loss_event` only iterates `policies` |
| Annual GUL cap at `sum_insured` | `Market::on_insured_loss` | `remaining_asset_value` initialised at `sum_insured`, decremented with `min` clamp |
| Single shared damage fraction per cat event | `Market::on_loss_event` | One `model.sample(rng)` before the policies iterator |
| Attritional loss strictly after `PolicyBound` day | `perils::schedule_attritional_claims_for_policy` | `(from_day, year_end]` range with `from_day = policy_bound_day` |
| Renewal zero-drift | `Simulation::dispatch` (QuoteAccepted arm) | `renewal_day = qa_day + 361 − QUOTING_CHAIN_DAYS` |
| Year-1-only batch `CoverageRequested` | `Simulation::handle_year_start` | `if year.0 == 1` guard |
| Capital reset each year | `Insurer::on_year_start` | `self.capital = self.initial_capital` |

**Rules for placing new invariants:**

- **Local to one aggregate** → enforce in that handler; emit a consequence event (e.g. `InsurerInsolvent`) rather than returning an error or panicking.
- **Use `debug_assert!`** for ordering invariants that should never fail in a correct run (e.g. "loss day must be ≥ policy bound day").
- **Use early return with `vec![]`** for defensive handling of unknown IDs (unknown `PolicyId` in a loss event, etc.).
- **Use `panic!`** only in test helpers where preconditions must hold for the test to be meaningful.

---

## §7 Cross-Reference and Maintenance

When making changes, update the corresponding reference:

| Change | Files to update |
|---|---|
| Add or rename an event variant | `src/events.rs` + `docs/event-flow.md` (mandatory per CLAUDE.md) |
| New invariant added | Add row to §6 table above |
| New aggregate added | Add row to §2 table above |
| New mutable field that is not log-reconstructible | Document on the field; note recovery path |
| First `AggregateCursor` built | Add `seq: usize` to `SimEvent`; update §5 "Today" note |

**Status badges** (`[ACTIVE]`, `[PARTIAL]`, `[PLANNED]`, `[TBD]`) live in `docs/market-mechanics.md`, not here. This document describes implementation architecture, not market feature completeness.
