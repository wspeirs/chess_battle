import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./engine-dialog.css";

export type LimitCfg =
  | { kind: "Movetime"; value: number }
  | { kind: "Depth"; value: number }
  | { kind: "Nodes"; value: number };

export interface EngineConfig {
  preset: string;
  path: string;
  args: string[];
  options: [string, string][];
  limit: LimitCfg;
}

export interface Config {
  white: EngineConfig;
  black: EngineConfig;
  evaluator: EngineConfig;
  side_boards: boolean;
}

export interface Preset {
  name: string;
  binary: string;
  args: string[];
  options: [string, string][];
}

export interface ProbeOption {
  name: string;
  type_: string;
}

export interface ProbeResult {
  name: string | null;
  author: string | null;
  options: ProbeOption[];
}

const PROFILES_KEY = "engineProfiles";

function loadProfiles(): Record<string, EngineConfig> {
  try {
    return JSON.parse(localStorage.getItem(PROFILES_KEY) ?? "{}");
  } catch {
    return {};
  }
}

function saveProfile(cfg: EngineConfig) {
  const profiles = loadProfiles();
  profiles[cfg.preset.toLowerCase()] = cfg;
  localStorage.setItem(PROFILES_KEY, JSON.stringify(profiles));
}

type SectionKey = "white" | "black" | "evaluator";

let dialogEl: HTMLDialogElement | null = null;
let currentConfig: Config | null = null;
let availablePresets: Preset[] = [];

interface SectionElements {
  presetSelect: HTMLSelectElement;
  pathInput: HTMLInputElement;
  argsInput: HTMLInputElement;
  weightsRow: HTMLElement;
  weightsInput: HTMLInputElement;
  threadsInput: HTMLInputElement;
  hashInput: HTMLInputElement;
  limitTypeSelect: HTMLSelectElement;
  limitValueInput: HTMLInputElement;
  extraOptionsList: HTMLElement;
  testResultBox: HTMLElement;
}

const sectionMap: Record<SectionKey, SectionElements | null> = {
  white: null,
  black: null,
  evaluator: null,
};

function createSectionUI(
  title: string,
  key: SectionKey,
  presets: Preset[]
): HTMLElement {
  const section = document.createElement("div");
  section.className = "engine-section";
  section.innerHTML = `
    <h3>${title}</h3>
    <div class="form-grid">
      <div class="form-group">
        <label>Preset</label>
        <select class="sec-preset"></select>
      </div>
      <div class="form-group full-width">
        <label>Binary Path</label>
        <div class="input-with-button">
          <input type="text" class="sec-path" placeholder="/path/to/engine" />
          <button type="button" class="btn sec-browse">Browse…</button>
        </div>
      </div>
      <div class="form-group full-width">
        <label>Arguments (space-separated)</label>
        <input type="text" class="sec-args" placeholder="e.g. --uci" />
      </div>
      <div class="form-group full-width sec-weights-row" style="display: none;">
        <label>Lc0 Weights File</label>
        <div class="input-with-button">
          <input type="text" class="sec-weights" placeholder="/path/to/weights.pb.gz" />
          <button type="button" class="btn sec-weights-browse">Browse…</button>
        </div>
      </div>
      <div class="form-group">
        <label>Threads</label>
        <input type="number" class="sec-threads" min="1" placeholder="e.g. 1" />
      </div>
      <div class="form-group">
        <label>Hash (MB)</label>
        <input type="number" class="sec-hash" min="1" placeholder="e.g. 256" />
      </div>
      <div class="form-group">
        <label>Limit Type</label>
        <select class="sec-limit-type">
          <option value="Movetime">Movetime (ms)</option>
          <option value="Depth">Depth</option>
          <option value="Nodes">Nodes</option>
        </select>
      </div>
      <div class="form-group">
        <label>Limit Value</label>
        <input type="number" class="sec-limit-val" min="1" />
      </div>
      <div class="extra-options-container">
        <div class="extra-options-header">
          <span>Extra UCI Options</span>
          <button type="button" class="btn btn-sm sec-add-opt">+ Add Option</button>
        </div>
        <div class="sec-extra-opts"></div>
      </div>
      <div class="test-section">
        <div style="display: flex; gap: 0.5rem; align-items: center;">
          <button type="button" class="btn sec-test">Test Engine</button>
          <button type="button" class="btn sec-save-profile">Save Profile</button>
        </div>
        <div class="test-result-box sec-test-result" style="display: none;"></div>
      </div>
    </div>
  `;

  const presetSelect = section.querySelector(".sec-preset") as HTMLSelectElement;
  const pathInput = section.querySelector(".sec-path") as HTMLInputElement;
  const browseBtn = section.querySelector(".sec-browse") as HTMLButtonElement;
  const argsInput = section.querySelector(".sec-args") as HTMLInputElement;
  const weightsRow = section.querySelector(".sec-weights-row") as HTMLElement;
  const weightsInput = section.querySelector(".sec-weights") as HTMLInputElement;
  const weightsBrowseBtn = section.querySelector(".sec-weights-browse") as HTMLButtonElement;
  const threadsInput = section.querySelector(".sec-threads") as HTMLInputElement;
  const hashInput = section.querySelector(".sec-hash") as HTMLInputElement;
  const limitTypeSelect = section.querySelector(".sec-limit-type") as HTMLSelectElement;
  const limitValueInput = section.querySelector(".sec-limit-val") as HTMLInputElement;
  const addOptBtn = section.querySelector(".sec-add-opt") as HTMLButtonElement;
  const extraOptionsList = section.querySelector(".sec-extra-opts") as HTMLElement;
  const testBtn = section.querySelector(".sec-test") as HTMLButtonElement;
  const testResultBox = section.querySelector(".sec-test-result") as HTMLElement;

  const saveProfileBtn = section.querySelector(".sec-save-profile") as HTMLButtonElement;
  saveProfileBtn.addEventListener("click", () => {
    saveProfile(collectSectionConfig(key));
    saveProfileBtn.textContent = "Saved ✓";
    setTimeout(() => (saveProfileBtn.textContent = "Save Profile"), 1200);
  });

  // Fill presets dropdown
  for (const p of presets) {
    const opt = document.createElement("option");
    opt.value = p.name;
    opt.textContent = p.name;
    presetSelect.appendChild(opt);
  }

  // Browse binary
  browseBtn.addEventListener("click", async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: `Select ${title} Binary`,
      });
      if (selected && typeof selected === "string") {
        pathInput.value = selected;
      }
    } catch (err) {
      console.error("Browse failed:", err);
    }
  });

  // Browse weights
  weightsBrowseBtn.addEventListener("click", async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: "Select Lc0 Weights File",
      });
      if (selected && typeof selected === "string") {
        weightsInput.value = selected;
      }
    } catch (err) {
      console.error("Browse weights failed:", err);
    }
  });

  // Add extra option row
  addOptBtn.addEventListener("click", () => {
    addOptionRow(extraOptionsList, "", "");
  });

  // Preset change handler
  presetSelect.addEventListener("change", async () => {
    const selectedPresetName = presetSelect.value;
    const preset = presets.find(
      (p) => p.name.toLowerCase() === selectedPresetName.toLowerCase()
    );

    // Restore the saved profile for this preset, if any
    const saved = loadProfiles()[selectedPresetName.toLowerCase()];
    if (saved) {
      populateSection(key, saved);
      return;
    }

    // Apply detect_engine
    try {
      const detected = await invoke<string | null>("detect_engine", {
        preset: selectedPresetName,
      });
      if (detected) {
        pathInput.value = detected;
      } else if (preset) {
        pathInput.value = preset.binary;
      }
    } catch (e) {
      console.error("detect_engine failed:", e);
    }

    if (preset) {
      argsInput.value = preset.args.join(" ");
      weightsRow.style.display =
        preset.name.toLowerCase() === "lc0" ? "block" : "none";

      // Options
      let threads = "";
      let hash = "";
      let weights = "";
      const extras: [string, string][] = [];

      for (const [k, v] of preset.options) {
        if (k.toLowerCase() === "threads") threads = v;
        else if (k.toLowerCase() === "hash") hash = v;
        else if (k.toLowerCase() === "weightsfile") weights = v;
        else extras.push([k, v]);
      }

      threadsInput.value = threads;
      hashInput.value = hash;
      weightsInput.value = weights;

      extraOptionsList.innerHTML = "";
      for (const [k, v] of extras) {
        addOptionRow(extraOptionsList, k, v);
      }
    } else {
      weightsRow.style.display =
        selectedPresetName.toLowerCase() === "lc0" ? "block" : "none";
    }
  });

  // Test engine handler
  testBtn.addEventListener("click", async () => {
    testResultBox.style.display = "block";
    testResultBox.className = "test-result-box";
    testResultBox.textContent = "Probing engine…";

    const cfg = collectSectionConfig(key);
    try {
      const res = await invoke<ProbeResult>("probe_engine", { config: cfg });
      testResultBox.className = "test-result-box success";
      const optList = res.options.map((o) => `${o.name} (${o.type_})`).join(", ");
      testResultBox.textContent =
        `Name: ${res.name ?? "Unknown"}\n` +
        `Author: ${res.author ?? "Unknown"}\n` +
        `Declared options (${res.options.length}): ${optList || "None"}`;
    } catch (err: unknown) {
      testResultBox.className = "test-result-box error";
      const msg = err instanceof Error ? err.message : String(err);
      testResultBox.textContent = `Probe failed: ${msg}`;
    }
  });

  sectionMap[key] = {
    presetSelect,
    pathInput,
    argsInput,
    weightsRow,
    weightsInput,
    threadsInput,
    hashInput,
    limitTypeSelect,
    limitValueInput,
    extraOptionsList,
    testResultBox,
  };

  return section;
}

function addOptionRow(
  container: HTMLElement,
  name: string,
  value: string
): HTMLElement {
  const row = document.createElement("div");
  row.className = "option-row";
  row.innerHTML = `
    <input type="text" class="opt-name" placeholder="Option Name" value="${name}" />
    <input type="text" class="opt-val" placeholder="Value" value="${value}" />
    <button type="button" class="btn btn-sm btn-danger opt-del">×</button>
  `;

  row.querySelector(".opt-del")?.addEventListener("click", () => {
    row.remove();
  });

  container.appendChild(row);
  return row;
}

function populateSection(key: SectionKey, cfg: EngineConfig) {
  const el = sectionMap[key];
  if (!el) return;

  el.presetSelect.value = cfg.preset;
  el.pathInput.value = cfg.path;
  el.argsInput.value = cfg.args.join(" ");

  const isLc0 = cfg.preset.toLowerCase() === "lc0";
  el.weightsRow.style.display = isLc0 ? "block" : "none";

  let threads = "";
  let hash = "";
  let weights = "";
  const extras: [string, string][] = [];

  for (const [k, v] of cfg.options) {
    if (k.toLowerCase() === "threads") {
      threads = v;
    } else if (k.toLowerCase() === "hash") {
      hash = v;
    } else if (k.toLowerCase() === "weightsfile") {
      weights = v;
    } else {
      extras.push([k, v]);
    }
  }

  el.threadsInput.value = threads;
  el.hashInput.value = hash;
  el.weightsInput.value = weights;

  el.limitTypeSelect.value = cfg.limit.kind;
  el.limitValueInput.value = String(cfg.limit.value);

  el.extraOptionsList.innerHTML = "";
  for (const [k, v] of extras) {
    addOptionRow(el.extraOptionsList, k, v);
  }

  el.testResultBox.style.display = "none";
  el.testResultBox.textContent = "";
}

function collectSectionConfig(key: SectionKey): EngineConfig {
  const el = sectionMap[key];
  if (!el) {
    throw new Error(`Section ${key} not initialized`);
  }

  const preset = el.presetSelect.value;
  const path = el.pathInput.value.trim();
  const args = el.argsInput.value
    .trim()
    .split(/\s+/)
    .filter((a) => a.length > 0);

  const options: [string, string][] = [];

  // Dedicated mapped options
  if (preset.toLowerCase() === "lc0" && el.weightsInput.value.trim()) {
    options.push(["WeightsFile", el.weightsInput.value.trim()]);
  }
  if (el.threadsInput.value.trim()) {
    options.push(["Threads", el.threadsInput.value.trim()]);
  }
  if (el.hashInput.value.trim()) {
    options.push(["Hash", el.hashInput.value.trim()]);
  }

  // Extra options
  const rows = el.extraOptionsList.querySelectorAll(".option-row");
  rows.forEach((row) => {
    const nameInput = row.querySelector(".opt-name") as HTMLInputElement;
    const valInput = row.querySelector(".opt-val") as HTMLInputElement;
    const name = nameInput.value.trim();
    const val = valInput.value.trim();
    if (name) {
      options.push([name, val]);
    }
  });

  const limitKind = el.limitTypeSelect.value as "Movetime" | "Depth" | "Nodes";
  const limitNum = Number(el.limitValueInput.value) || 1;
  const limit: LimitCfg = {
    kind: limitKind,
    value: limitNum,
  };

  return {
    preset,
    path,
    args,
    options,
    limit,
  };
}

function buildDialog(): HTMLDialogElement {
  const dialog = document.createElement("dialog");
  dialog.className = "engine-dialog";

  dialog.innerHTML = `
    <div class="dialog-container">
      <div class="dialog-header">
        <h2>Engine Configuration</h2>
      </div>
      <div class="dialog-sections"></div>
      <div class="dialog-actions">
        <button type="button" class="btn dialog-cancel">Cancel</button>
        <button type="button" class="btn btn-primary dialog-save">Save</button>
      </div>
    </div>
  `;

  const sectionsContainer = dialog.querySelector(".dialog-sections") as HTMLElement;
  const cancelBtn = dialog.querySelector(".dialog-cancel") as HTMLButtonElement;
  const saveBtn = dialog.querySelector(".dialog-save") as HTMLButtonElement;

  sectionsContainer.appendChild(createSectionUI("White Engine", "white", availablePresets));
  sectionsContainer.appendChild(createSectionUI("Black Engine", "black", availablePresets));
  sectionsContainer.appendChild(
    createSectionUI("Evaluator Engine", "evaluator", availablePresets)
  );

  cancelBtn.addEventListener("click", () => {
    dialog.close();
  });

  saveBtn.addEventListener("click", async () => {
    try {
      const white = collectSectionConfig("white");
      const black = collectSectionConfig("black");
      const evaluator = collectSectionConfig("evaluator");
      for (const c of [white, black, evaluator]) saveProfile(c);

      const newConfig: Config = {
        white,
        black,
        evaluator,
        side_boards: currentConfig?.side_boards ?? true,
      };

      await invoke("set_config", { config: newConfig });
      currentConfig = newConfig;
      dialog.close();
    } catch (err: unknown) {
      console.error("Failed to save config:", err);
      const msg = err instanceof Error ? err.message : String(err);
      alert(`Failed to save config: ${msg}`);
    }
  });

  document.body.appendChild(dialog);
  return dialog;
}

export async function openEngineDialog(): Promise<void> {
  if (availablePresets.length === 0) {
    try {
      availablePresets = await invoke<Preset[]>("list_presets");
    } catch (e) {
      console.error("Failed to fetch presets:", e);
      availablePresets = [];
    }
  }

  if (!dialogEl) {
    dialogEl = buildDialog();
  }

  try {
    currentConfig = await invoke<Config>("get_config");
  } catch (e) {
    console.error("Failed to get config:", e);
  }

  if (currentConfig) {
    populateSection("white", currentConfig.white);
    populateSection("black", currentConfig.black);
    populateSection("evaluator", currentConfig.evaluator);
  }

  dialogEl.showModal();
}
