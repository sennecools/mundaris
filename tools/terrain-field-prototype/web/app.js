(() => {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const api = async (path, body, signal) => {
    const response = await fetch(`/api/${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers: body === undefined ? {} : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal,
    });
    let data;
    try { data = await response.json(); }
    catch { throw new Error(`The authoring service returned HTTP ${response.status} without JSON.`); }
    if (!response.ok || data?.ok === false) throw new Error(data?.error || `Request failed (HTTP ${response.status}).`);
    return data;
  };
  const clone = (value) => JSON.parse(JSON.stringify(value));
  const state = {
    catalog: null, graph: null, selected: null, preview: null, undo: [], redo: [],
    previewTicket: 0, previewTimer: 0, galleryBusy: false, toastTimer: 0, autoNodePreview: false,
  };
  const svgNs = "http://www.w3.org/2000/svg";
  const kindFor = (kind) => state.catalog?.node_kinds?.find((item) => item.kind === kind);
  const templateFor = (id) => state.catalog?.templates?.find((item) => item.id === id);
  const labelFor = (kind) => kindFor(kind)?.label || kind;
  const nodeFor = (id) => state.graph?.nodes?.find((node) => node.id === id);
  const toast = (message) => {
    const el = $("toast");
    el.textContent = message;
    el.classList.add("visible");
    clearTimeout(state.toastTimer);
    state.toastTimer = setTimeout(() => el.classList.remove("visible"), 2200);
  };
  const setStatus = (text, mode = "ready") => {
    const el = $("preview-state");
    el.textContent = text;
    el.className = `status-pill${mode === "error" ? " status-error" : mode === "working" ? " status-working" : ""}`;
  };
  const showError = (message) => {
    $("error-banner").textContent = message;
    $("error-banner").hidden = false;
    setStatus("Needs attention", "error");
  };
  const clearError = () => { $("error-banner").hidden = true; };
  const setGraph = (graph, record = true) => {
    if (!graph || !Array.isArray(graph.nodes) || !graph.seeds) throw new Error("This file does not contain a supported terrain graph.");
    if (record && state.graph) pushUndo(state.graph);
    state.graph = clone(graph);
    state.graph.layout ||= {};
    state.selected = state.graph.outputs?.height || state.graph.nodes[0]?.id || null;
    state.autoNodePreview = false;
    normalizeLayout();
    renderAll();
  };
  const pushUndo = (value) => {
    state.undo.push(clone(value));
    if (state.undo.length > 100) state.undo.shift();
    state.redo.length = 0;
    updateHistoryButtons();
  };
  const updateHistoryButtons = () => {
    $("undo-button").disabled = state.undo.length === 0;
    $("redo-button").disabled = state.redo.length === 0;
  };
  const normalizeLayout = () => {
    if (!state.graph) return;
    const auto = graphLayout(state.graph);
    state.graph.nodes.forEach((node, index) => {
      if (!state.graph.layout[node.id]) state.graph.layout[node.id] = auto.get(node.id) || { x: 28 + (index % 3) * 270, y: 30 + Math.floor(index / 3) * 230 };
    });
  };
  const graphLayout = (graph) => {
    const byId = new Map((graph.nodes || []).map((node) => [node.id, node]));
    const depthCache = new Map(); const visiting = new Set();
    const depthOf = (node) => {
      if (depthCache.has(node.id)) return depthCache.get(node.id);
      if (visiting.has(node.id)) return 0;
      visiting.add(node.id);
      let depth = 0;
      Object.values(node.inputs || {}).forEach((sourceId) => {
        const source = byId.get(sourceId);
        if (source) depth = Math.max(depth, depthOf(source) + 1);
      });
      visiting.delete(node.id); depthCache.set(node.id, depth); return depth;
    };
    const columns = new Map();
    (graph.nodes || []).forEach((node) => { const depth = depthOf(node); if (!columns.has(depth)) columns.set(depth, []); columns.get(depth).push(node); });
    const positions = new Map();
    columns.forEach((nodes, depth) => nodes.forEach((node, row) => positions.set(node.id, { x: 30 + depth * 286, y: 28 + row * 220 })));
    return positions;
  };
  const mutate = (callback, preview = true) => {
    if (!state.graph) return;
    pushUndo(state.graph);
    callback(state.graph);
    renderAll();
    if (preview) schedulePreview();
  };
  const replaceWithValidated = async (graph, message) => {
    const checked = await api("validate", { graph });
    setGraph(graph, true);
    clearError();
    schedulePreview();
    toast(message || "Recipe accepted by the Rust validator.");
    return checked;
  };
  const inferredOutputType = (node, visited = new Set()) => {
    const descriptor = kindFor(node?.kind);
    const declared = descriptor?.output_type || "";
    if (declared !== "scalar") return declared;
    const unit = inferScalarUnit(node, visited);
    return unit ? `scalar[${unit}]` : "scalar";
  };
  const inferScalarUnit = (node, visited = new Set()) => {
    if (!node || visited.has(node.id)) return null;
    visited.add(node.id);
    const kind = String(node.kind || "").toLowerCase();
    if ((kind === "noise" || kind === "constant") && ["m", "1", "K"].includes(node.params?.unit)) return node.params.unit;
    if (kind === "crater_height") return "m";
    if (kind === "crater_support") return "1";
    const descriptor = kindFor(node.kind);
    for (const input of descriptor?.inputs || []) {
      const source = nodeFor(node.inputs?.[input.name]);
      const unit = inferScalarUnit(source, new Set(visited));
      if (unit) return unit;
    }
    return null;
  };
  const acceptsSource = (input, source) => {
    const expected = String(input.type || "");
    const actual = inferredOutputType(source);
    if (expected === "scalar") return actual === "scalar" || actual.startsWith("scalar[");
    if (expected.startsWith("scalar[")) return actual === expected;
    return actual === expected;
  };
  const compatibleSources = (input) => state.graph.nodes.filter((node) => acceptsSource(input, node));
  const uniqueId = () => globalThis.crypto?.randomUUID ? crypto.randomUUID() : `node-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  const displayValue = (value) => {
    if (Array.isArray(value)) return value.map((item) => Array.isArray(item) ? item.map((v) => Number(v).toFixed(2)).join(",") : String(item)).join(" · ");
    if (typeof value === "number") return Number.isInteger(value) ? String(value) : Number(value).toPrecision(4);
    return String(value ?? "—");
  };

  function renderAll() {
    if (!state.graph || !state.catalog) return;
    normalizeLayout();
    renderGraph();
    renderInspector();
    renderSeeds();
    updateHistoryButtons();
  }

  function renderGraph() {
    const layer = $("node-layer");
    layer.replaceChildren();
    $("wire-layer").replaceChildren();
    $("graph-empty").hidden = state.graph.nodes.length !== 0;
    $("node-count").textContent = `${state.graph.nodes.length} node${state.graph.nodes.length === 1 ? "" : "s"}`;
    state.graph.nodes.forEach((node) => {
      const descriptor = kindFor(node.kind) || { kind: node.kind, label: node.kind, output_type: "?", inputs: [], params: [] };
      const pos = state.graph.layout[node.id] || { x: 20, y: 20 };
      const card = document.createElement("article");
      card.className = `node-card${state.selected === node.id ? " selected" : ""}`;
      card.dataset.nodeId = node.id;
      card.style.left = `${Math.max(0, Number(pos.x) || 0)}px`;
      card.style.top = `${Math.max(0, Number(pos.y) || 0)}px`;
      card.setAttribute("aria-label", `${descriptor.label} node`);
      card.tabIndex = 0;
      const titlebar = document.createElement("div");
      titlebar.className = "node-titlebar";
      const tag = document.createElement("span"); tag.className = "node-type-tag"; tag.textContent = node.kind.toUpperCase();
      const title = document.createElement("span"); title.className = "node-title"; title.textContent = node.label || descriptor.label;
      titlebar.append(tag, title); card.append(titlebar);
      const content = document.createElement("div"); content.className = "node-content";
      (descriptor.inputs || []).forEach((input) => {
        const row = document.createElement("label"); row.className = "node-input-line";
        const dot = document.createElement("i"); dot.className = "port-dot"; dot.dataset.inputPort = `${node.id}:${input.name}`;
        const name = document.createElement("span"); name.textContent = input.name;
        const select = document.createElement("select"); select.className = "node-input-select";
        select.setAttribute("aria-label", `${node.label || descriptor.label} ${input.name} source`);
        const currentId = node.inputs?.[input.name] || "";
        addOption(select, "", input.required ? "Choose source…" : "Unconnected");
        const sources = compatibleSources(input).filter((source) => source.id !== node.id);
        if (currentId && !sources.some((source) => source.id === currentId)) addOption(select, currentId, `${nodeFor(currentId)?.label || currentId} · incompatible`);
        sources.forEach((source) => addOption(select, source.id, source.label || `${labelFor(source.kind)} ${source.id.slice(0, 5)}`));
        select.value = currentId;
        select.addEventListener("change", () => {
          mutate((graph) => {
            const target = graph.nodes.find((item) => item.id === node.id);
            target.inputs ||= {};
            if (select.value) target.inputs[input.name] = select.value;
            else delete target.inputs[input.name];
          });
        });
        row.append(dot, name, select); content.append(row);
      });
      const out = document.createElement("div"); out.className = "node-port-row";
      const outDot = document.createElement("i"); outDot.className = "port-dot port-out"; outDot.dataset.outputPort = node.id;
      const outName = document.createElement("span"); outName.textContent = "Output";
      const type = document.createElement("span"); type.className = "port-type"; type.textContent = inferredOutputType(node) || "unknown";
      out.append(outDot, outName, type); content.append(out);
      (descriptor.params || []).slice(0, 3).forEach((param) => {
        const row = document.createElement("div"); row.className = "node-port-row";
        const spacer = document.createElement("span");
        const name = document.createElement("span"); name.textContent = param.label || param.name;
        const value = document.createElement("span"); value.className = "node-value"; value.textContent = displayValue(node.params?.[param.name] ?? param.default);
        row.append(spacer, name, value); content.append(row);
      });
      card.append(content);
      card.addEventListener("click", (event) => {
        if (event.target.closest("select")) return;
        const changed = state.selected !== node.id;
        if (changed) { state.selected = node.id; renderAll(); }
        if (selectedScalarNode() && $("map-select").value !== "node_values") { state.autoNodePreview = true; schedulePreview(0); }
      });
      card.addEventListener("keydown", (event) => {
        if ((event.key === "Enter" || event.key === " ") && !event.target.closest("select")) {
          event.preventDefault();
          const changed = state.selected !== node.id;
          if (changed) { state.selected = node.id; renderAll(); }
          if (selectedScalarNode() && $("map-select").value !== "node_values") { state.autoNodePreview = true; schedulePreview(0); }
        }
      });
      titlebar.addEventListener("pointerdown", (event) => beginNodeDrag(event, node.id));
      layer.append(card);
    });
    requestAnimationFrame(drawWires);
  }

  function addOption(select, value, label) { const option = document.createElement("option"); option.value = value; option.textContent = label; select.append(option); }

  function beginNodeDrag(event, nodeId) {
    if (event.button !== 0 || event.target.closest("button,select,input")) return;
    event.preventDefault();
    const card = event.currentTarget.closest(".node-card");
    if (!card) return;
    state.selected = nodeId;
    const start = { x: event.clientX, y: event.clientY, left: parseFloat(card.style.left) || 0, top: parseFloat(card.style.top) || 0, before: clone(state.graph) };
    card.classList.add("dragging");
    card.setPointerCapture(event.pointerId);
    const move = (moveEvent) => {
      const layout = state.graph.layout[nodeId];
      layout.x = Math.max(0, start.left + moveEvent.clientX - start.x);
      layout.y = Math.max(0, start.top + moveEvent.clientY - start.y);
      card.style.left = `${layout.x}px`; card.style.top = `${layout.y}px`;
      drawWires();
    };
    const end = () => {
      card.classList.remove("dragging");
      card.removeEventListener("pointermove", move);
      card.removeEventListener("pointerup", end);
      card.removeEventListener("pointercancel", end);
      if (JSON.stringify(start.before.layout[nodeId]) !== JSON.stringify(state.graph.layout[nodeId])) {
        state.undo.push(start.before); if (state.undo.length > 100) state.undo.shift(); state.redo.length = 0; updateHistoryButtons();
      }
      renderInspector();
    };
    card.addEventListener("pointermove", move);
    card.addEventListener("pointerup", end, { once: true });
    card.addEventListener("pointercancel", end, { once: true });
  }

  function drawWires() {
    if (!state.graph) return;
    const viewport = $("graph-viewport");
    const canvas = $("graph-canvas");
    const svg = $("wire-layer");
    svg.setAttribute("width", canvas.scrollWidth || canvas.clientWidth);
    svg.setAttribute("height", canvas.scrollHeight || canvas.clientHeight);
    svg.replaceChildren();
    const cards = new Map([...$("node-layer").querySelectorAll(".node-card")].map((node) => [node.dataset.nodeId, node]));
    state.graph.nodes.forEach((target) => {
      const card = cards.get(target.id); if (!card) return;
      const targetRect = card.getBoundingClientRect();
      const viewRect = viewport.getBoundingClientRect();
      const targetX = targetRect.left - viewRect.left + viewport.scrollLeft;
      Object.entries(target.inputs || {}).forEach(([inputName, sourceId]) => {
        const sourceCard = cards.get(sourceId); if (!sourceCard) return;
        const sourceRect = sourceCard.getBoundingClientRect();
        const sourcePort = sourceCard.querySelector(`[data-output-port="${CSS.escape(sourceId)}"]`);
        const targetPort = card.querySelector(`[data-input-port="${CSS.escape(`${target.id}:${inputName}`)}"]`);
        if (!sourcePort || !targetPort) return;
        const a = sourcePort.getBoundingClientRect(); const b = targetPort.getBoundingClientRect();
        const x1 = sourceRect.left - viewRect.left + viewport.scrollLeft + sourceRect.width;
        const y1 = a.top - viewRect.top + viewport.scrollTop + a.height / 2;
        const x2 = targetX;
        const y2 = b.top - viewRect.top + viewport.scrollTop + b.height / 2;
        const bend = Math.max(44, Math.abs(x2 - x1) * .45);
        const path = document.createElementNS(svgNs, "path");
        path.setAttribute("d", `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`);
        path.setAttribute("class", `graph-wire${state.selected === target.id ? " is-selected" : ""}`);
        svg.append(path);
      });
    });
  }

  function renderInspector() {
    const node = nodeFor(state.selected);
    updateMapOptions(node);
    $("inspector-empty").hidden = Boolean(node);
    $("inspector-content").hidden = !node;
    $("delete-node").disabled = !node || state.graph.nodes.some((other) => Object.values(other.inputs || {}).includes(node.id)) || Object.values(state.graph.outputs || {}).includes(node.id);
    if (!node) { $("input-fields").replaceChildren(); $("parameter-fields").replaceChildren(); return; }
    const descriptor = kindFor(node.kind) || { kind: node.kind, label: node.kind, output_type: "?", inputs: [], params: [] };
    $("selected-node-kind").textContent = node.kind.toUpperCase();
    $("selected-node-label").textContent = node.label || descriptor.label;
    $("node-label").value = node.label || "";
    renderInspectorInputs(node, descriptor);
    renderParameters(node, descriptor);
  }

  function selectedScalarNode() {
    const node = nodeFor(state.selected);
    const output = inferredOutputType(node);
    return output === "scalar" || output.startsWith("scalar[") || output.startsWith("scalar<") ? node : null;
  }

  function updateMapOptions(node) {
    const select = $("map-select");
    const selected = select.value;
    for (const option of [...select.options]) if (option.dataset.dynamic === "true") option.remove();
    const weights = ["weight_0", "weight_1", "weight_2"];
    weights.forEach((key, index) => {
      const option = document.createElement("option"); option.value = key; option.textContent = `Material weight ${index + 1} · 0–1`; option.dataset.dynamic = "true"; select.append(option);
    });
    if (node && selectedScalarNode()) {
      const option = document.createElement("option"); option.value = "node_values"; option.textContent = `Selected node · ${node.label || labelFor(node.kind)}`; option.dataset.dynamic = "true"; select.append(option);
    }
    const available = [...select.options].some((option) => option.value === selected);
    if (available) select.value = selected;
    else select.value = "height";
  }

  function renderInspectorInputs(node, descriptor) {
    const holder = $("input-fields"); holder.replaceChildren();
    if (!(descriptor.inputs || []).length) return;
    appendSectionTitle(holder, "INPUT CONNECTIONS");
    descriptor.inputs.forEach((input) => {
      const field = document.createElement("label"); field.className = "inspector-field";
      const head = document.createElement("span"); head.className = "inspector-field-head";
      const name = document.createElement("span"); name.textContent = input.name;
      const type = document.createElement("small"); type.textContent = `${input.type}${input.required ? " · required" : " · optional"}`;
      head.append(name, type);
      const select = document.createElement("select"); select.setAttribute("aria-label", `${input.name} input source`);
      addOption(select, "", input.required ? "Choose source…" : "Unconnected");
      const sources = compatibleSources(input).filter((source) => source.id !== node.id);
      const current = node.inputs?.[input.name] || "";
      if (current && !sources.some((source) => source.id === current)) addOption(select, current, `${nodeFor(current)?.label || current} · incompatible`);
      sources.forEach((source) => addOption(select, source.id, source.label || `${labelFor(source.kind)} ${source.id.slice(0, 5)}`));
      select.value = current;
      select.addEventListener("change", () => mutate((graph) => {
        const target = graph.nodes.find((item) => item.id === node.id); target.inputs ||= {};
        if (select.value) target.inputs[input.name] = select.value; else delete target.inputs[input.name];
      }));
      field.append(head, select); holder.append(field);
    });
  }

  function appendSectionTitle(holder, text) { const title = document.createElement("h3"); title.className = "section-title"; title.textContent = text; holder.append(title); }

  function renderParameters(node, descriptor) {
    const holder = $("parameter-fields"); holder.replaceChildren();
    if (!(descriptor.params || []).length) return;
    appendSectionTitle(holder, "PARAMETERS");
    descriptor.params.forEach((param) => {
      const value = node.params?.[param.name] ?? param.default;
      if (param.type === "color_palette" || (Array.isArray(value) && value.length && Array.isArray(value[0]))) {
        renderPalette(holder, node, param, value); return;
      }
      if (Array.isArray(value) && value.length > 0 && value.every((item) => Number.isFinite(Number(item)))) {
        renderNumericVector(holder, node, param, value); return;
      }
      if ((param.type === "text" || param.type === "string") && Array.isArray(value)) {
        renderNames(holder, node, param, value); return;
      }
      const field = document.createElement("label"); field.className = "inspector-field";
      const head = document.createElement("span"); head.className = "inspector-field-head";
      const name = document.createElement("span"); name.textContent = param.label || param.name;
      const unit = document.createElement("small"); unit.textContent = param.unit || param.type || "";
      head.append(name, unit);
      let input;
      if (Array.isArray(param.choices) && param.choices.length) {
        input = document.createElement("select");
        param.choices.forEach((choice) => addOption(input, choice, choice));
        input.value = value;
      } else if (param.type === "text" || param.type === "string") {
        input = document.createElement("input"); input.type = "text"; input.maxLength = 128; input.value = value ?? "";
      } else {
        input = document.createElement("input"); input.type = "number";
        input.step = param.step ?? (param.type === "integer" ? "1" : "any");
        if (param.min !== undefined) input.min = param.min;
        if (param.max !== undefined) input.max = param.max;
        input.value = value ?? "";
      }
      input.setAttribute("aria-label", param.label || param.name);
      input.addEventListener("change", () => {
        let parsed = input.type === "number" ? Number(input.value) : input.value;
        if (input.type === "number" && (!Number.isFinite(parsed) || input.value.trim() === "")) { input.value = value ?? ""; return; }
        if (param.type === "integer") parsed = Math.round(parsed);
        mutate((graph) => { graph.nodes.find((item) => item.id === node.id).params[param.name] = parsed; });
      });
      field.append(head, input); holder.append(field);
    });
  }

  function renderPalette(holder, node, param, palette) {
    const field = document.createElement("div"); field.className = "inspector-field";
    const head = document.createElement("span"); head.className = "inspector-field-head";
    const name = document.createElement("span"); name.textContent = param.label || param.name;
    const unit = document.createElement("small"); unit.textContent = "linear RGB 0–1"; head.append(name, unit); field.append(head);
    const entries = Array.isArray(palette) ? palette : [];
    entries.forEach((rgb, index) => {
      const row = document.createElement("div"); row.className = "palette-row";
      const label = document.createElement("span"); label.textContent = `Layer ${index + 1}`;
      const color = document.createElement("input"); color.type = "color"; color.value = rgbToHex(rgb); color.setAttribute("aria-label", `${param.label || param.name} layer ${index + 1} color`);
      color.addEventListener("change", () => {
        const parsed = hexToRgb(color.value);
        mutate((graph) => { graph.nodes.find((item) => item.id === node.id).params[param.name][index] = parsed; });
      });
      row.append(label, color);
      [0, 1, 2].forEach((channel) => {
        const input = document.createElement("input"); input.type = "number"; input.min = "0"; input.max = "1"; input.step = "0.01";
        input.value = Number(rgb?.[channel] ?? 0).toFixed(2); input.setAttribute("aria-label", `Layer ${index + 1} ${["red", "green", "blue"][channel]} channel`);
        input.addEventListener("change", () => {
          const n = Number(input.value); if (!Number.isFinite(n)) return;
          mutate((graph) => { graph.nodes.find((item) => item.id === node.id).params[param.name][index][channel] = Math.min(1, Math.max(0, n)); });
        });
        row.append(input);
      });
      field.append(row);
    });
    holder.append(field);
  }

  function renderNumericVector(holder, node, param, vector) {
    const field = document.createElement("div"); field.className = "inspector-field";
    const head = document.createElement("span"); head.className = "inspector-field-head";
    const name = document.createElement("span"); name.textContent = param.label || param.name;
    const unit = document.createElement("small"); unit.textContent = "numeric vector"; head.append(name, unit); field.append(head);
    const row = document.createElement("div"); row.className = "vector-grid";
    const axisNames = param.name === "center_direction" ? ["x", "y", "z"] : vector.length === 3 ? ["1", "2", "3"] : vector.map((_, i) => String(i + 1));
    vector.forEach((value, index) => {
      const cell = document.createElement("label"); cell.className = "vector-cell";
      const axis = document.createElement("span"); axis.textContent = axisNames[index] || String(index + 1); cell.append(axis);
      const input = document.createElement("input"); input.type = "number"; input.step = "any"; input.value = Number(value); input.setAttribute("aria-label", `${param.label || param.name} component ${axisNames[index] || index + 1}`);
      input.addEventListener("change", () => {
        const n = Number(input.value); if (!Number.isFinite(n)) return;
        mutate((graph) => { graph.nodes.find((item) => item.id === node.id).params[param.name][index] = n; });
      });
      cell.append(input); row.append(cell);
    });
    field.append(row); holder.append(field);
  }

  function renderNames(holder, node, param, names) {
    const field = document.createElement("div"); field.className = "inspector-field";
    const head = document.createElement("span"); head.className = "inspector-field-head";
    const name = document.createElement("span"); name.textContent = param.label || param.name;
    const unit = document.createElement("small"); unit.textContent = "material names"; head.append(name, unit); field.append(head);
    const grid = document.createElement("div"); grid.className = "names-grid";
    names.forEach((value, index) => {
      const row = document.createElement("label"); const label = document.createElement("span"); label.textContent = `Layer ${index + 1}`;
      const input = document.createElement("input"); input.type = "text"; input.maxLength = 48; input.value = value ?? ""; input.setAttribute("aria-label", `Material name ${index + 1}`);
      input.addEventListener("change", () => mutate((graph) => { graph.nodes.find((item) => item.id === node.id).params[param.name][index] = input.value; }));
      row.append(label, input); grid.append(row);
    });
    field.append(grid); holder.append(field);
  }

  function rgbToHex(rgb) {
    const toSrgb = (linear) => linear <= 0.0031308 ? 12.92 * linear : 1.055 * Math.pow(linear, 1 / 2.4) - 0.055;
    return `#${(rgb || [0,0,0]).slice(0, 3).map((value) => Math.round(Math.max(0, Math.min(1, toSrgb(Math.max(0, Number(value) || 0)))) * 255).toString(16).padStart(2, "0")).join("")}`;
  }
  function hexToRgb(hex) {
    return [1, 3, 5].map((index) => {
      const srgb = parseInt(hex.slice(index, index + 2), 16) / 255;
      return srgb <= 0.04045 ? srgb / 12.92 : Math.pow((srgb + 0.055) / 1.055, 2.4);
    });
  }

  function renderSeeds() {
    ["geometry", "climate", "material"].forEach((key) => { $(`seed-${key}`).value = state.graph.seeds[key] ?? 0; });
  }

  function schedulePreview(delay = 280) {
    if (!state.graph) return;
    clearTimeout(state.previewTimer);
    const ticket = ++state.previewTicket;
    setStatus("Preview queued", "working");
    state.previewTimer = setTimeout(() => requestPreview(ticket), delay);
  }

  async function requestPreview(ticket = ++state.previewTicket, graph = state.graph, options = {}) {
    const sent = clone(graph);
    setStatus("Evaluating", "working");
    try {
      const inspected = options.inspectNode === undefined ? selectedScalarNode()?.id || null : options.inspectNode;
      const data = await api("preview", { graph: sent, resolution: options.resolution || Number($("resolution-select").value), face: options.face ?? Number($("face-select").value), inspect_node: inspected });
      if (ticket !== state.previewTicket || !state.graph || JSON.stringify(sent) !== JSON.stringify(state.graph)) return data;
      state.preview = data;
      if (state.autoNodePreview && data.node_values) { $("map-select").value = "node_values"; state.autoNodePreview = false; }
      clearError(); setStatus("Rust preview ready"); renderPreview();
      return data;
    } catch (error) {
      if (ticket === state.previewTicket && JSON.stringify(sent) === JSON.stringify(state.graph)) showError(error.message);
      return null;
    }
  }

  function renderPreview() {
    const data = state.preview; if (!data) return;
    const field = $("map-select").value;
    const nodeValues = field === "node_values" ? data.node_values : null;
    const values = field === "node_values" ? nodeValues?.values : data.maps?.[field];
    if (!values) {
      $("map-empty").hidden = false; $("map-empty").querySelector("strong").textContent = `${field} map not returned`;
      $("map-empty").querySelector("span").textContent = field === "node_values" ? "The selected node has no scalar inspection map in this response." : "The Rust preview response does not include this field.";
      return;
    }
    const width = Number(data.width) || 0, height = Number(data.height) || 0;
    if (!width || !height) { showError("The Rust preview response has no valid map dimensions."); return; }
    const canvas = $("map-canvas"); canvas.width = width; canvas.height = height;
    const ctx = canvas.getContext("2d", { alpha: false });
    const pixels = ctx.createImageData(width, height);
    if (field === "material" || field === "normal") {
      for (let i = 0; i < width * height; i++) {
        pixels.data[i * 4] = byte(values[i * 3]); pixels.data[i * 4 + 1] = byte(values[i * 3 + 1]); pixels.data[i * 4 + 2] = byte(values[i * 3 + 2]); pixels.data[i * 4 + 3] = 255;
      }
    } else {
      const range = field === "node_values" && Array.isArray(nodeValues?.range) ? nodeValues.range : scalarRange(data, field, values);
      const span = range[1] - range[0];
      for (let i = 0; i < width * height; i++) {
        const raw = Number(values[i]); const normalized = span > 0 ? Math.max(0, Math.min(1, (raw - range[0]) / span)) : .5;
        const shade = Math.round(normalized * 255);
        pixels.data[i * 4] = shade; pixels.data[i * 4 + 1] = shade; pixels.data[i * 4 + 2] = shade; pixels.data[i * 4 + 3] = 255;
      }
    }
    ctx.putImageData(pixels, 0, 0);
    $("map-empty").hidden = true;
    const labels = { height: "Height field", humidity: "Humidity field", temperature: "Temperature field", material: "Material color preview", support: "Feature support mask", normal: "Normal visualization", weight_0: "Material weight · layer 1", weight_1: "Material weight · layer 2", weight_2: "Material weight · layer 3", node_values: `Selected node · ${nodeValues?.unit || "scalar"}` };
    $("map-field-name").textContent = field === "node_values" ? `${nodeFor(nodeValues?.id)?.label || "Selected node"} · ${nodeValues?.unit || "scalar"}` : labels[field] || `${field} field`;
    if (field === "material" || field === "normal") $("map-range").textContent = "linear RGB · normalized";
    else { const range = field === "node_values" && Array.isArray(nodeValues?.range) ? nodeValues.range : scalarRange(data, field, values); const unit = field === "node_values" ? nodeValues?.unit : field === "height" ? "m" : field === "temperature" ? "K" : field.startsWith("weight_") || field === "support" || field === "humidity" ? "0–1" : ""; $("map-range").textContent = `${format(range[0])} – ${format(range[1])} ${unit || ""}`; }
    const ids = data.identities || {};
    $("identity-geometry").textContent = ids.geometry || "—";
    $("identity-climate").textContent = ids.climate || "—";
    $("identity-palette").textContent = ids.palette || ids.material || "—";
    const stats = data.stats || {};
    $("stat-height").textContent = `${format(stats.height_min_m)} … ${format(stats.height_max_m)} m`;
    $("stat-humidity").textContent = `${format(stats.humidity_min)} … ${format(stats.humidity_max)}`;
    $("stat-temperature").textContent = `${format(stats.temperature_min_k)} … ${format(stats.temperature_max_k)} K`;
    $("stat-size").textContent = `${width} × ${height} · face ${$("face-select").value}`;
  }

  function scalarRange(data, field, values) {
    const supplied = data.ranges?.[field];
    if (Array.isArray(supplied) && supplied.length >= 2 && Number.isFinite(Number(supplied[0])) && Number.isFinite(Number(supplied[1]))) return [Number(supplied[0]), Number(supplied[1])];
    const stats = data.stats || {};
    const fallback = field === "height" ? [stats.height_min_m, stats.height_max_m] : field === "humidity" ? [stats.humidity_min, stats.humidity_max] : field === "temperature" ? [stats.temperature_min_k, stats.temperature_max_k] : [0, 1];
    if (fallback.every((v) => Number.isFinite(Number(v)))) return fallback.map(Number);
    let low = Infinity, high = -Infinity;
    values.forEach((v) => { const n = Number(v); if (Number.isFinite(n)) { low = Math.min(low, n); high = Math.max(high, n); } });
    return Number.isFinite(low) ? [low, high] : [0, 1];
  }
  function byte(value) { return Math.round(Math.max(0, Math.min(1, Number(value) || 0)) * 255); }
  function format(value) { const n = Number(value); return Number.isFinite(n) ? n.toFixed(Math.abs(n) >= 100 ? 1 : 2) : "—"; }

  async function applyTemplate() {
    const template = templateFor($("template-select").value);
    if (!template) return;
    if (template.id === "rocky-feature") $("face-select").value = "4";
    try { await replaceWithValidated(template.graph, `${template.name || template.id} loaded.`); }
    catch (error) { showError(error.message); }
  }

  function createNode(kind) {
    const descriptor = kindFor(kind); if (!descriptor) return;
    const node = { id: uniqueId(), kind, label: descriptor.label || kind, params: {}, inputs: {} };
    (descriptor.params || []).forEach((param) => { if (param.default !== undefined) node.params[param.name] = clone(param.default); });
    mutate((graph) => {
      graph.nodes.push(node);
      graph.layout[node.id] = { x: 38 + (graph.nodes.length % 3) * 270, y: 38 + Math.floor((graph.nodes.length - 1) / 3) * 250 };
    }, false);
    state.selected = node.id; state.autoNodePreview = Boolean(selectedScalarNode()); renderAll();
    schedulePreview();
    toast(`${labelFor(kind)} node added. Connect its inputs to complete the recipe.`);
  }

  function deleteSelected() {
    const node = nodeFor(state.selected); if (!node || $("delete-node").disabled) return;
    mutate((graph) => {
      graph.nodes = graph.nodes.filter((item) => item.id !== node.id);
      delete graph.layout[node.id];
      graph.nodes.forEach((item) => { Object.entries(item.inputs || {}).forEach(([key, value]) => { if (value === node.id) delete item.inputs[key]; }); });
      state.selected = graph.nodes[0]?.id || null;
    });
  }

  async function loadFile(file) {
    if (!file) return;
    try {
      if (file.size > 2_000_000) throw new Error("Graph file exceeds the 2 MB editor limit.");
      const graph = JSON.parse(await file.text());
      await replaceWithValidated(graph, "Graph loaded and validated.");
    } catch (error) { showError(`Import rejected: ${error.message}`); }
    finally { $("load-file").value = ""; }
  }

  function downloadGraph() {
    if (!state.graph) return;
    const blob = new Blob([`${JSON.stringify(state.graph, null, 2)}\n`], { type: "application/json" });
    const url = URL.createObjectURL(blob); const anchor = document.createElement("a");
    const safeName = String(state.graph.recipe_id || "terrain-recipe").replace(/[^a-z0-9_-]+/gi, "-");
    anchor.href = url; anchor.download = `${safeName}.json`; anchor.click(); URL.revokeObjectURL(url); toast("Graph JSON downloaded.");
  }

  async function exportGraph() {
    if (!state.graph) return;
    const button = $("export-button"); button.disabled = true; button.textContent = "Exporting…";
    try {
      const result = await api("export", { graph: clone(state.graph), resolution: Number($("resolution-select").value) });
      $("export-path").textContent = result.bundle_path || "Export complete";
      $("export-hash").textContent = `Manifest SHA-256  ${result.manifest_sha256 || "not provided"}`;
      $("export-files").textContent = Array.isArray(result.files) ? `${result.files.length} files · ${result.files.map((file) => typeof file === "string" ? file : file.path || file.name || "file").join(" · ")}` : "File list not provided";
      $("export-result").hidden = false; toast("Export published by the Rust evaluator.");
    } catch (error) { showError(`Export failed: ${error.message}`); }
    finally { button.disabled = false; button.textContent = "Export fields"; }
  }

  async function generateGallery() {
    if (state.galleryBusy || !state.graph) return;
    state.galleryBusy = true; $("generate-gallery").disabled = true; $("gallery-grid").replaceChildren();
    const button = $("generate-gallery"); button.textContent = "Generating…";
    const locks = { geometry: $("lock-geometry").checked, climate: $("lock-climate").checked, palette: $("lock-palette").checked };
    const baseSeed = Number(state.graph.seeds.geometry) || 0;
    $("gallery-status").textContent = "Evaluating four constrained variations in sequence…";
    for (let index = 0; index < 4; index++) {
      const seed = (baseSeed + 104729 * (index + 1)) % Number.MAX_SAFE_INTEGER;
      try {
        const variation = await api("variation", { graph: clone(state.graph), seed, locks });
        const graph = variation.graph;
        if (!graph) throw new Error("Variation response did not include a graph.");
        const preview = await api("preview", { graph, resolution: 32, face: Number($("face-select").value), inspect_node: null });
        addGalleryCard(graph, preview, seed, index + 1);
      } catch (error) {
        const failed = document.createElement("div"); failed.className = "gallery-card";
        const title = document.createElement("strong"); title.textContent = `Variation ${index + 1} failed`;
        const detail = document.createElement("span"); detail.textContent = error.message; failed.append(title, detail); $("gallery-grid").append(failed);
      }
      $("gallery-status").textContent = `Completed ${index + 1} of 4 sequential candidates.`;
    }
    state.galleryBusy = false; button.disabled = false; button.textContent = "Generate 4";
  }

  function addGalleryCard(graph, preview, seed, number) {
    const card = document.createElement("button"); card.type = "button"; card.className = "gallery-card"; card.setAttribute("aria-label", `Adopt variation ${number} with variation seed ${seed}`);
    const thumb = document.createElement("canvas"); thumb.className = "gallery-thumb"; thumb.width = preview.width || 32; thumb.height = preview.height || 32;
    drawThumbnail(thumb, preview);
    const title = document.createElement("strong"); title.textContent = `Variation ${number}`;
    const identity = document.createElement("span"); identity.textContent = `seed ${seed} · ${preview.identities?.geometry || "geometry pending"}`;
    card.append(thumb, title, identity);
    card.addEventListener("click", async () => {
      try { await replaceWithValidated(graph, `Variation ${number} adopted.`); }
      catch (error) { showError(error.message); }
    });
    $("gallery-grid").append(card);
  }

  function drawThumbnail(canvas, data) {
    const field = $("map-select").value === "material" ? "material" : "height";
    const values = data.maps?.[field] || data.maps?.height; if (!values) return;
    const ctx = canvas.getContext("2d"); const image = ctx.createImageData(canvas.width, canvas.height);
    if (field === "material" && data.maps?.material) {
      for (let i = 0; i < canvas.width * canvas.height; i++) { image.data[i*4] = byte(values[i*3]); image.data[i*4+1] = byte(values[i*3+1]); image.data[i*4+2] = byte(values[i*3+2]); image.data[i*4+3] = 255; }
    } else {
      const range = scalarRange(data, "height", values); const span = range[1] - range[0];
      for (let i = 0; i < canvas.width * canvas.height; i++) { const shade = Math.round(Math.max(0,Math.min(1,span ? (Number(values[i])-range[0])/span : .5))*255); image.data[i*4] = shade; image.data[i*4+1] = shade; image.data[i*4+2] = shade; image.data[i*4+3] = 255; }
    }
    ctx.putImageData(image, 0, 0);
  }

  function undo() {
    if (!state.undo.length) return;
    state.redo.push(clone(state.graph)); state.graph = state.undo.pop(); state.selected = state.graph.nodes.find((node) => node.id === state.selected)?.id || state.graph.nodes[0]?.id || null;
    renderAll(); updateHistoryButtons(); schedulePreview();
  }
  function redo() {
    if (!state.redo.length) return;
    state.undo.push(clone(state.graph)); state.graph = state.redo.pop(); state.selected = state.graph.nodes.find((node) => node.id === state.selected)?.id || state.graph.nodes[0]?.id || null;
    renderAll(); updateHistoryButtons(); schedulePreview();
  }

  async function init() {
    setStatus("Connecting", "working");
    try {
      state.catalog = await api("catalog");
      const templates = state.catalog.templates || [];
      $("template-select").replaceChildren();
      templates.forEach((template) => addOption($("template-select"), template.id, template.name || template.id));
      $("add-node-kind").replaceChildren();
      (state.catalog.node_kinds || []).forEach((node) => addOption($("add-node-kind"), node.kind, node.label || node.kind));
      if (!templates.length) throw new Error("The catalog returned no recipe templates.");
      const initial = templateFor("rocky-feature") || templates[0];
      $("template-select").value = initial.id;
      if (initial.id === "rocky-feature") $("face-select").value = "4";
      setGraph(initial.graph, false); clearError(); setStatus("Catalog ready");
      await requestPreview(++state.previewTicket);
    } catch (error) {
      setStatus("Service unavailable", "error");
      showError(`Cannot load the Rust authoring service: ${error.message}`);
    }
  }

  $("apply-template").addEventListener("click", applyTemplate);
  $("add-node-button").addEventListener("click", () => createNode($("add-node-kind").value));
  $("delete-node").addEventListener("click", deleteSelected);
  $("undo-button").addEventListener("click", undo); $("redo-button").addEventListener("click", redo);
  $("save-button").addEventListener("click", downloadGraph);
  $("load-button").addEventListener("click", () => $("load-file").click());
  $("load-file").addEventListener("change", (event) => loadFile(event.target.files?.[0]));
  $("export-button").addEventListener("click", exportGraph);
  $("generate-gallery").addEventListener("click", generateGallery);
  $("refresh-preview").addEventListener("click", () => { clearTimeout(state.previewTimer); schedulePreview(0); });
  $("map-select").addEventListener("change", () => {
    if ($("map-select").value === "node_values" && !state.preview?.node_values) schedulePreview(0);
    else renderPreview();
  });
  $("face-select").addEventListener("change", () => schedulePreview(0));
  $("resolution-select").addEventListener("change", () => schedulePreview(0));
  $("node-label").addEventListener("change", (event) => {
    const value = event.target.value.trim();
    if (value.length > 64) return;
    mutate((graph) => { const node = graph.nodes.find((item) => item.id === state.selected); if (node) node.label = value || labelFor(node.kind); }, false);
  });
  ["geometry", "climate", "material"].forEach((key) => $(`seed-${key}`).addEventListener("change", (event) => {
    const value = Number(event.target.value); if (!Number.isSafeInteger(value) || value < 0) { event.target.value = state.graph.seeds[key] ?? 0; toast("Seeds must be nonnegative safe integers."); return; }
    mutate((graph) => { graph.seeds[key] = value; });
  }));
  $("graph-viewport").addEventListener("scroll", () => requestAnimationFrame(drawWires));
  window.addEventListener("resize", () => requestAnimationFrame(drawWires));
  document.addEventListener("keydown", (event) => {
    if (event.target.matches("input,textarea,select,[contenteditable=true]")) return;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") { event.preventDefault(); event.shiftKey ? redo() : undo(); }
    else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "y") { event.preventDefault(); redo(); }
    else if ((event.key === "Delete" || event.key === "Backspace") && state.selected) { event.preventDefault(); deleteSelected(); }
  });
  init();
})();
