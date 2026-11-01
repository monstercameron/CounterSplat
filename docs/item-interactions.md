# Proposed ECS Item and Interaction API

> **Status: proposed design, not implemented.** The current ECS has no item catalog, inventory, equipment, or item operations. This document describes a possible extension grounded in the existing `World`, component, command, event, and Lua bridge APIs.

## Goals and boundaries

The API should scale across many item kinds without adding an enum variant or bespoke system for every kind. It should give Rust and Lua clear ways to create instances, move and combine them, and request interactions. It should preserve the ECS crate's current separation: the core owns neutral entity/component state and validated structural operations; the game or application owns the meaning of using, equipping, crafting, consuming, damaging, and otherwise interacting with items.

Existing primitives this proposal builds on are `Entity` (world-scoped and generational, process-local), typed Rust components, named dynamic JSON components, opt-in `ComponentAdapter`s, typed and dynamic resources, ordered events, `Schedule`, and owned `WorldOperation`/`WorldOutcome` messages. None of the item-specific types or operations below exist yet.

## Definitions versus instances

Definitions describe reusable item kinds. Instances are entities representing individual things in the current world. Do not spawn one entity per item kind, and do not make item kinds variants in a central enum.

```rust
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemDefId(String); // validate namespaced syntax, e.g. "game:iron_sword"

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemDefinition {
    pub id: ItemDefId,
    pub display_name: String,
    pub stack_limit: u32,
    pub tags: BTreeSet<String>,
    pub defaults: BTreeMap<String, serde_json::Value>,
}

#[derive(Default)]
pub struct ItemCatalog {
    definitions: BTreeMap<ItemDefId, ItemDefinition>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemInstance {
    pub definition: ItemDefId,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Stack {
    pub count: NonZeroU32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Durability {
    pub current: u32,
    pub maximum: u32,
}
```

The code is illustrative: validation and privacy details need to be chosen when implemented. Use stable, namespaced definition IDs such as `base:healing_potion` or `mod.frost:ice_axe`. These IDs can be used in saves and network messages. An `Entity` remains a runtime handle only; its current `world:index.generation` string must not be treated as a persistent save ID. Persistent instance identity and save/load remapping are a separate feature.

Definitions contain shared metadata and serializable data defaults, not closures or trait objects. Optional capabilities are modeled by independent data/components and tags. For example, a sword might have `Durability`, an apple might have `Stack`, and a key might have a `KeyCode` component. Avoid a single `Item { durability: Option<_>, equip_slot: Option<_>, ... }` that accumulates fields for unrelated item kinds.

The proposed `ItemDomain` owns its catalog alongside the `World` on the simulation thread. This lets a handler borrow the domain and world independently instead of trying to borrow a resource out of the world while also mutating that world. If scripts or other systems need to inspect definitions, expose an explicit read-only catalog view; do not expose mutable catalog internals by default. Definitions can initialize dynamic components or registered serde components, but only explicitly exposed components become script-visible, matching the current ECS policy.

## Instances and locations

A unique item instance is an entity with `ItemInstance` and only the components it needs. A stack is a single item entity with `Stack { count }`; splitting creates another entity. This supports per-instance durability, affixes, ownership, and other state without forcing every item to have those fields.

Inventory/equipment should use explicit item-location semantics rather than assuming the generic entity hierarchy is inventory. The current parent/child API is a general hierarchy with detach semantics and should not silently become an ownership mechanism.

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ItemLocation {
    World { position: [f32; 3] },
    Inventory { owner: Entity, slot: u32 },
    Equipment { owner: Entity, slot: String },
}
```

A concrete storage representation might be `Inventory { slots: Vec<Option<Entity>> }`, `Equipment { slots: BTreeMap<String, Entity> }`, and an `ItemLocation` component on the item. These are examples, not settled layouts. Keep a single authoritative representation where possible; if both container slots and item location are stored, every operation must update them atomically and validate their agreement.

Required invariants:

- A live item instance references a registered `ItemDefId`.
- Stack count is nonzero and no greater than the definition's stack limit.
- An item is in at most one location at a time.
- Inventory/equipment references point to live item entities and agree with the item's location.
- A move, split, merge, or future transfer either fully succeeds or leaves all involved entities and containers unchanged.
- Despawning an item clears its container/equipment reference; despawning an owner follows an explicit game policy (drop, transfer, or destroy), not an accidental hierarchy side effect.
- Ownership and destination handles are checked for current world and generation before mutation.

## Typed Rust API

Make typed item-domain handlers the primary interface. They can validate against `ItemCatalog` and keep each domain operation atomic while Rust retains ergonomic static types. The signatures below are illustrative facade methods on the separate item/domain crate, taking `&mut World`; they are not additions to `World`.

```rust
let sword = items.create_item(
    &mut world,
    ItemDefId::new("base:iron_sword")?,
    NonZeroU32::new(1).unwrap(),
    ItemLocation::Inventory { owner: player, slot: 0 },
)?;

items.move_item(
    &mut world,
    sword,
    ItemLocation::Equipment { owner: player, slot: "hand.main".into() },
)?;

let split = items.split_stack(&mut world, apples, NonZeroU32::new(3).unwrap())?;
items.merge_stacks(&mut world, split.created, apples)?;
```

Potential typed signatures and result shape:

```rust
pub fn create_item(
    &mut self, world: &mut World, definition: ItemDefId, count: NonZeroU32, location: ItemLocation,
) -> Result<Entity, ItemError>;

pub fn move_item(
    &mut self, world: &mut World, item: Entity, destination: ItemLocation,
) -> Result<ItemMoveResult, ItemError>;

pub fn split_stack(
    &mut self, world: &mut World, item: Entity, count: NonZeroU32,
) -> Result<ItemSplitResult, ItemError>;

pub fn merge_stacks(
    &mut self, world: &mut World, source: Entity, target: Entity,
) -> Result<ItemMergeResult, ItemError>;
```

Possible errors include `UnknownDefinition`, `NotAnItem`, `StaleEntity`, `InvalidCount`, `StackLimitExceeded`, `IncompatibleDefinitions`, `SameEntity`, `OccupiedSlot`, `InvalidOwner`, `InvalidDestination`, and `LocationInvariantViolation`. Validate all preconditions before changing either an entity or container. Typed errors can be converted to stable, descriptive strings at the existing bridge boundary.

Structural operations should cover generic creation, moving, splitting, and merging. The ECS crate should not implement item behavior. In particular, the ECS crate should not gain item-specific methods or `WorldOperation` variants. Put the catalog, item components, domain commands/results, validation, and handlers in a separate item/domain crate that depends on ECS. The application host routes an outer item-domain command envelope to a handler running on the ECS owner thread. This preserves the existing dependency direction and keeps generic ECS usable without items. Game-specific policy such as whether a player may equip a sword, whether a potion heals, whether a door accepts a key, or whether a recipe is valid remains in the application/game layer.

## Dynamic, thread-safe commands

The current `WorldOperation` enum provides a language-neutral, serializable ECS command boundary. Do not extend it with item-specific variants. Define owned-data `ItemCommand` and `ItemOutcome` in the separate item/domain crate and route them through a host/domain envelope alongside ordinary ECS commands; do not send Rust closures, Lua values, borrows, or trait objects across threads.

```rust
pub enum ItemCommand {
    CreateItem { definition: ItemDefId, count: u32, location: ItemLocation },
    MoveItem { item: Entity, destination: ItemLocation },
    SplitStack { item: Entity, count: u32 },
    MergeStacks { source: Entity, target: Entity },
    ReadItemDefinition { definition: ItemDefId },
}

pub enum ItemOutcome {
    ItemCreated { item: Entity },
    ItemMoved { item: Entity, from: ItemLocation, to: ItemLocation },
    StackSplit { original: Entity, created: Entity, moved_count: u32 },
    StacksMerged { source: Entity, target: Entity, moved_count: u32, source_despawned: bool },
    ItemDefinition(Option<ItemDefinitionView>),
}
```

Names and payloads can change during implementation, but prefer specific result variants over overloading `Applied`, `Value`, or stringly typed event payloads. The item handler should return `Result<ItemOutcome, ItemError>` and preserve the outer host request correlation ID.

### Interactions

A universal core `use_item` operation is risky because the word “use” has game-specific semantics. Prefer a neutral interaction request that records intent and lets a game system decide the outcome:

```rust
pub struct InteractionRequest {
    pub actor: Entity,
    pub target: InteractionTarget,
    pub verb: String,
    pub payload: serde_json::Value,
}

pub enum InteractionTarget {
    Item(Entity),
    Entity(Entity),
    WorldPoint([f32; 3]),
}
```

A possible command is `RequestInteraction(InteractionRequest)`. It validates that referenced handles are live, then queues a typed event or returns an accepted request ID; it does not itself consume an item, apply damage, open a door, or equip anything. The game-side system subscribes to requests, checks rules, performs ECS mutations using typed methods, and emits game-specific completion/rejection events. If this request/event abstraction proves unnecessary for the host, existing named event queues can carry an agreed request payload instead. Do not add an opaque callback registry in the core crate.

For item-to-item interactions such as combining ingredients, prefer a dedicated game system that consumes a request with two item entity IDs and then performs validated structural operations. This keeps game-defined recipes out of the ECS while still making item identity and ownership easy to verify.

## Lua interface

Lua currently calls named component/resource/event operations through `WorldBridge`; JSON values are copied, entities are opaque strings, and bridge calls are synchronous individual requests. Mirror only the typed item verbs that the Rust side can validate. A possible ergonomic surface is:

```lua
local potion = items.create("base:healing_potion", {
    count = 2,
    at = { kind = "inventory", owner = player, slot = 4 },
})

items.move(potion, { kind = "world", position = { 10, 0, 2 } })
local remainder = items.split(potion, 1)
items.merge(potion, remainder)
local definition = items.definition("base:healing_potion") -- read-only view
items.interact(player, potion, "use", { target = enemy }) -- intent only
```

The location representations and Lua names above are proposed. Convert maps into typed DTOs at the Lua boundary and validate keys, number ranges, finite coordinates, IDs, and required fields before creating an `ItemCommand`. Return entity IDs in the current opaque string format. Definition queries should return a copied read-only JSON-compatible view. Existing `ecs.get/set/query` can already address named script-visible components; use new high-level `items.*` calls for operations that require cross-entity atomic validation.

Do not allow arbitrary runtime scripts to register definitions unless hot-reload/versioning rules are explicit. Prefer host bootstrap registration into `ItemCatalog`. If script-authored definitions become a requirement, add a validated registration command with a clear phase/authority policy and collision behavior.

Lua callbacks must not execute while the ECS is locked or while the ECS owner waits on the script worker. The current `ChannelBridge` is synchronous, so the ECS owner must keep servicing requests while Lua runs. A timeout does not cancel an already accepted operation. Keep this limitation visible in the API docs.

## Determinism, events, and tick ordering

Current `WorldOperation`s are individual requests without tick IDs. Proposed `ItemCommand`s should also be wrapped with tick/order metadata when introduced. The Lua README documents that scripts are not transactions: earlier writes persist if a later call fails and other clients may interleave. The threading design calls tick-batched commands the preferred direction for deterministic frame boundaries.

Before depending on cross-client ordering for gameplay, add a command-batch envelope with a tick and stable sequence/source order. Apply item batches serially on the world owner, preserving the documented policy for all-or-nothing behavior (per-operation atomicity is the minimum; whole-batch atomicity is a separate decision). Reject or explicitly classify commands for stale ticks. Do not imply deterministic ordering simply because one individual world method validates atomically.

If item lifecycle events are added, use typed owned payloads and deterministic emission order, for example `ItemCreated`, `ItemMoved`, `StackSplit`, `StackMerged`, and `InteractionRequested`. Include tick or sequence metadata when the host introduces it. Keep game-specific events such as `PotionConsumed`, `DoorUnlocked`, and `DamageApplied` in the game/application layer. Existing `BTreeMap` use and ordered entity queries provide a useful deterministic basis; avoid relying on `HashMap` iteration order.

## Incremental implementation sequence

1. **Definitions and simple instances.** Add a validated `ItemDefId`, `ItemDefinition`, and `ItemDomain`-owned `ItemCatalog`; add `ItemInstance` and `Stack`; implement registration/lookup and typed `create_item`. Define duplicate-ID and invalid stack-limit behavior. Document that runtime `Entity` values are not persistent IDs.
2. **Typed structural results and commands.** In the separate item/domain crate, add typed error/result types and `ItemCommand`/`ItemOutcome` variants for definition read, create, and simple item inspection; define the host envelope/dispatch route without changing ECS `WorldOperation` or `WorldOutcome`. Ensure all validations happen before mutation. Keep catalog registration initially as host bootstrap, not a runtime gameplay operation.
3. **Explicit locations.** Add inventory/equipment/world-location representation only for the requirements in scope. Make movement atomic and enforce one-location invariants. Do not reuse `set_parent` as an implicit inventory implementation.
4. **Stack operations.** Add split/merge with compatibility checks, count and limit validation, full preflight, and deterministic results. Decide whether a fully merged source is despawned or left at count zero; the proposed invariant recommends despawning it because zero stacks are invalid.
5. **Interaction request path.** Add an optional neutral request/event type in the item/domain crate and demonstrate how an application `Schedule` system handles a request. Keep example policy in a game/app crate or host, never in ECS core.
6. **Lua mirror.** Add the `items` namespace to `scripting-lua` for the implemented typed operations; test conversions, invalid handles, unknown definitions, malformed location DTOs, and errors. Preserve the synchronous bridge caveats.
7. **Tick-batched commands and persistence.** Before broad gameplay use across workers, add a host/domain command envelope with tick, stable producer identity, and sequence-tagged batches consistent with `docs/threading.md`. Treat save-stable instance IDs and load remapping as a distinct persistence feature, not an accidental use of current entity strings.

## Tradeoffs and decisions to settle

- **Definition defaults:** JSON defaults enable data/mod extensibility and Lua inspection; typed Rust components retain compile-time safety. Start with serializable defaults and explicit adapters, and avoid pretending JSON defaults provide schema validation without adding a schema layer.
- **Location storage:** a single location component is simple to query, while container slot arrays are convenient for capacity/slot semantics. Duplicated representations require transactional synchronization. Choose one source of truth or centralize every write in item helpers.
- **Interaction operation:** a typed intent command is easier to discover and validate than undocumented event-name conventions. It must remain an intent, not a game rule. If the host already has a routing convention, named events may be sufficient.
- **Atomicity scope:** atomic per-operation moves/splits/merges are essential. Atomic multi-command scripts are not currently provided and require a batch design; do not claim scripts are transactions.
- **Persistence:** stable definition IDs are straightforward; stable instance IDs, catalog version migration, and save compatibility need separate requirements and APIs.

## Integrity against generic ECS operations

A separate domain handler alone does not protect its invariants if callers can use generic ECS writes. Today scripts can call `ecs.set`, `ecs.remove`, and `ecs.despawn`; registered serde adapters are writable by default. Item support must define an authority boundary before exposing inventory/equipment mutation:

- Keep authoritative item components private to the item/domain crate where possible. If scripts need to read them, register read-only `ComponentAdapter`s that reject writes and removals. Do not use writable `register_serde_component` for state whose invariants require cross-entity checks.
- Add a host-side command policy that rejects generic writes/removals to reserved item component names and rejects generic despawn of protected item instances or owners with live contents. Lua helpers alone are not a security boundary because the current generic ECS API remains available.
- Funnel item destruction, transfer, split, merge, and owner cleanup through the item-domain handler. Generic ECS lifecycle tracking is opt-in and is not an automatic observer, so it cannot silently guarantee container cleanup.
- If generic `World::despawn` remains callable for item entities, provide an explicit integration hook/cleanup path owned by the host/domain. Otherwise a despawn can leave stale container references. Owner destruction policy (drop, transfer, destroy) must be selected by game code and executed before the owner is removed.
- Each domain command must validate all affected handles, definitions, containers, slots, stack bounds, and state compatibility before an infallible commit. Existing generic component adapters can reject or have side effects; do not use a fallible adapter write midway through a multi-entity mutation and then claim rollback/transaction semantics.

### Stack state policy

Split/merge must define behavior for state beyond item definition and count. Do not clone only snapshot-visible components: snapshots omit private typed components. For splitting, define per-component policy (copy, partition, reset, reject split, or delegate to a game rule) for every stateful component. For merging, require compatible definition and all stack-relevant state (for example expiry, quality, charges, ownership restrictions, or affixes), not merely matching definition IDs. If arbitrary component cloning is impossible, initially restrict stack operations to a documented whitelist of stack-safe components and reject unsupported instances. A fully consumed source should be removed through the domain cleanup path; zero-count stacks violate the proposed invariant.

## Host routing and data conversion

The separate domain crate should depend on the current ECS crate, not the reverse. A possible host-side envelope is:

```rust
pub enum HostCommand {
    Ecs(WorldCommand),
    Items { request_id: u64, command: ItemCommand },
}
```

The ECS owner dispatches `Items` to the item handler: the owner stores `World` and `ItemDomain` side by side so the handler can borrow each independently. Keep request IDs and results correlated in the host envelope. The precise owner object and route are an implementation decision, but one owner must serialize ECS and item writes. `Entity` derives serde as a struct, while current Lua APIs accept/return string IDs; define explicit Lua/domain DTO conversions instead of relying on implicit serialization compatibility.

Existing ECS event queues are destructive single-consumer queues: `drain_events` removes values. Do not treat them as broadcast delivery to multiple systems. For multiple consumers, route/copy events at the host boundary or give each consumer its own queue. If event ordering is part of item behavior, define the consumer and ownership semantics.

## Deterministic commands and load

A later batch protocol should include `tick`, stable producer identity, and per-producer sequence, plus a barrier/late-command rule. Arrival order across threads is not a deterministic ordering guarantee. A request timeout does not cancel a command already accepted by the worker. Avoid a Lua↔simulation wait cycle by choosing a phase/queue protocol: for example, Lua consumes a tick snapshot, writes commands to a buffer, and the simulation applies the buffer at a defined boundary. The current synchronous per-call bridge cannot provide this protocol by itself.

Definition IDs should be versioned/migratable if definitions change between saves. Save files should contain stable definition IDs and a separate persistent instance key, then construct/remap runtime `Entity` handles on load. Never persist current entity strings as stable item identities.

## Scale validation

Current dynamic and typed queries generally scan component/entity collections, and snapshots clone exposed data. Start with the simple catalog and validate realistic inventory/instance counts before promising large-world scale. Add maintained indexes only in the item/domain layer and only when measurements show that catalog lookup, inventory membership, or item queries are a bottleneck; keep their update path centralized in the validated domain operations.

