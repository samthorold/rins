import { test } from "node:test";
import assert from "node:assert/strict";

import { EventBus, PanelRegistry } from "./shell.js";

// ---------- EventBus ----------

test("EventBus delivers published events to subscribers", () => {
  const bus = new EventBus();
  const seen = [];
  bus.on("yearSelected", (y) => seen.push(y));
  bus.emit("yearSelected", 5);
  bus.emit("yearSelected", 12);
  assert.deepEqual(seen, [5, 12]);
});

test("EventBus supports multiple subscribers per topic", () => {
  const bus = new EventBus();
  const a = [];
  const b = [];
  bus.on("yearSelected", (y) => a.push(y));
  bus.on("yearSelected", (y) => b.push(y));
  bus.emit("yearSelected", 7);
  assert.deepEqual(a, [7]);
  assert.deepEqual(b, [7]);
});

test("EventBus.off removes a subscriber", () => {
  const bus = new EventBus();
  const seen = [];
  const handler = (y) => seen.push(y);
  bus.on("yearSelected", handler);
  bus.emit("yearSelected", 1);
  bus.off("yearSelected", handler);
  bus.emit("yearSelected", 2);
  assert.deepEqual(seen, [1]);
});

test("EventBus emits to topics with no subscribers without throwing", () => {
  const bus = new EventBus();
  assert.doesNotThrow(() => bus.emit("nothingListens", 42));
});

test("EventBus isolates topics", () => {
  const bus = new EventBus();
  const seen = [];
  bus.on("a", (v) => seen.push(["a", v]));
  bus.on("b", (v) => seen.push(["b", v]));
  bus.emit("a", 1);
  bus.emit("b", 2);
  assert.deepEqual(seen, [["a", 1], ["b", 2]]);
});

// ---------- PanelRegistry ----------

test("PanelRegistry renders registered panels into their slots", () => {
  const slots = new Map([
    ["panel-1", { calls: [], replaceChildren(...args) { this.calls.push(args); } }],
    ["panel-2", { calls: [], replaceChildren(...args) { this.calls.push(args); } }],
  ]);
  const registry = new PanelRegistry({
    getSlot: (id) => slots.get(id),
    bus: new EventBus(),
  });

  const renders = [];
  registry.register({
    slotId: "panel-1",
    render: (ctx) => {
      renders.push(["p1", ctx.db]);
      return "node-1";
    },
  });
  registry.register({
    slotId: "panel-2",
    render: (ctx) => {
      renders.push(["p2", ctx.db]);
      return "node-2";
    },
  });

  const fakeDb = { tag: "db" };
  registry.renderAll(fakeDb);

  assert.deepEqual(renders, [["p1", fakeDb], ["p2", fakeDb]]);
  assert.deepEqual(slots.get("panel-1").calls, [["node-1"]]);
  assert.deepEqual(slots.get("panel-2").calls, [["node-2"]]);
});

test("PanelRegistry skips panels whose slot is missing", () => {
  const slots = new Map([
    ["present", { calls: [], replaceChildren(...args) { this.calls.push(args); } }],
  ]);
  const registry = new PanelRegistry({
    getSlot: (id) => slots.get(id),
    bus: new EventBus(),
  });

  let absentRendered = false;
  registry.register({ slotId: "absent", render: () => { absentRendered = true; return "x"; } });
  registry.register({ slotId: "present", render: () => "ok" });

  assert.doesNotThrow(() => registry.renderAll({}));
  assert.equal(absentRendered, false);
  assert.deepEqual(slots.get("present").calls, [["ok"]]);
});

test("PanelRegistry forwards yearSelected to panels that subscribe", () => {
  const bus = new EventBus();
  const slot = { calls: [], replaceChildren(...args) { this.calls.push(args); } };
  const registry = new PanelRegistry({
    getSlot: () => slot,
    bus,
  });

  const seen = [];
  registry.register({
    slotId: "p",
    render: () => "node",
    onYearSelected: (y) => seen.push(y),
  });

  registry.renderAll({});
  bus.emit("yearSelected", 9);
  bus.emit("yearSelected", null);

  assert.deepEqual(seen, [9, null]);
});

test("PanelRegistry.selectYear broadcasts via the bus", () => {
  const bus = new EventBus();
  const registry = new PanelRegistry({ getSlot: () => null, bus });

  const seen = [];
  bus.on("yearSelected", (y) => seen.push(y));
  registry.selectYear(4);
  registry.selectYear(null);

  assert.deepEqual(seen, [4, null]);
});

test("PanelRegistry remembers the last selected year for late-rendering panels", () => {
  const bus = new EventBus();
  const slot = { calls: [], replaceChildren(...args) { this.calls.push(args); } };
  const registry = new PanelRegistry({ getSlot: () => slot, bus });

  registry.selectYear(11);

  const seen = [];
  registry.register({
    slotId: "late",
    render: () => "node",
    onYearSelected: (y) => seen.push(y),
  });
  registry.renderAll({});

  // The late-registered panel should be told the current selection on mount.
  assert.deepEqual(seen, [11]);
});
