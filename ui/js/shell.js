// UI shell: panel registry + cross-panel event bus.
//
// Panels register a `render(ctx)` callback that returns a DOM node (or
// anything `replaceChildren` accepts). The registry mounts each panel into
// its slot when `renderAll(db)` is called. Panels can subscribe to a
// `yearSelected` topic to draw a cursor when another panel broadcasts a year.

export class EventBus {
  constructor() {
    this._topics = new Map();
  }

  on(topic, handler) {
    let bucket = this._topics.get(topic);
    if (!bucket) {
      bucket = new Set();
      this._topics.set(topic, bucket);
    }
    bucket.add(handler);
  }

  off(topic, handler) {
    const bucket = this._topics.get(topic);
    if (bucket) bucket.delete(handler);
  }

  emit(topic, payload) {
    const bucket = this._topics.get(topic);
    if (!bucket) return;
    for (const h of bucket) h(payload);
  }
}

export class PanelRegistry {
  constructor({ getSlot, bus }) {
    this._getSlot = getSlot;
    this._bus = bus;
    this._panels = [];
    this._lastDb = null;
    this._selectedYear = null;
  }

  register(panel) {
    this._panels.push(panel);
    if (panel.onYearSelected) {
      this._bus.on("yearSelected", panel.onYearSelected);
    }
    // If we've already rendered (or have a db ready), render this late panel.
    if (this._lastDb !== null) {
      this._mount(panel, this._lastDb);
      if (panel.onYearSelected && this._selectedYear !== null) {
        panel.onYearSelected(this._selectedYear);
      }
    } else if (panel.onYearSelected && this._selectedYear !== null) {
      // No db yet, but a selection has been made — replay it on mount-time.
      // We defer until renderAll is called so the panel can react with data.
      panel._pendingYear = this._selectedYear;
    }
  }

  renderAll(db) {
    this._lastDb = db;
    for (const panel of this._panels) {
      this._mount(panel, db);
      if (panel._pendingYear !== undefined) {
        const y = panel._pendingYear;
        delete panel._pendingYear;
        if (panel.onYearSelected) panel.onYearSelected(y);
      }
    }
  }

  selectYear(year) {
    this._selectedYear = year;
    this._bus.emit("yearSelected", year);
  }

  _mount(panel, db) {
    const slot = this._getSlot(panel.slotId);
    if (!slot) return;
    const node = panel.render({ db, bus: this._bus, registry: this });
    if (node !== undefined && node !== null) {
      slot.replaceChildren(node);
    }
  }
}
