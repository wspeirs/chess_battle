// src/main.ts - Main entry point and orchestration
import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createBoards } from "./boards";
import { createPanels, type EngineInfo } from "./panels";
import { createEvalBar, type EvalEvent } from "./evalbar";
import { openEngineDialog } from "./engine-dialog";
import { save } from "@tauri-apps/plugin-dialog";

interface PositionEvent {
  ply: number;
  fen: string;
  last_move: [string, string] | null;
  san_history: string[];
  side_to_move: "white" | "black";
  check: boolean;
}

interface GameOverEvent {
  result: string;
  reason: string;
}

interface EngineErrorEvent {
  side: string;
  message: string;
}

// DOM Elements
const mainBoardEl = document.getElementById("main-board")!;
const leftSideColumnEl = document.getElementById("left-side-column")!;
const rightSideColumnEl = document.getElementById("right-side-column")!;
const evalBarContainerEl = document.getElementById("eval-bar-container")!;
const topPanelEl = document.getElementById("top-panel")!;
const bottomPanelEl = document.getElementById("bottom-panel")!;
const moveListEl = document.getElementById("move-list")!;
const arenaGridEl = document.getElementById("arena-grid")!;

const btnNewGame = document.getElementById("btn-new-game") as HTMLButtonElement;
const inputFen = document.getElementById("input-fen") as HTMLInputElement;
const btnEngines = document.getElementById("btn-engines") as HTMLButtonElement;
const btnMode = document.getElementById("btn-mode") as HTMLButtonElement;
const btnNext = document.getElementById("btn-next") as HTMLButtonElement;
const btnMoveNow = document.getElementById("btn-move-now") as HTMLButtonElement;
const btnFlip = document.getElementById("btn-flip") as HTMLButtonElement;
const btnSideBoards = document.getElementById("btn-side-boards") as HTMLButtonElement;

const bannerResult = document.getElementById("banner-result") as HTMLDivElement;
const bannerError = document.getElementById("banner-error") as HTMLDivElement;

// Sub-components
const boards = createBoards({
  mainBoardContainer: mainBoardEl,
  leftContainer: leftSideColumnEl,
  rightContainer: rightSideColumnEl,
});

const panels = createPanels(topPanelEl, bottomPanelEl, moveListEl);
const evalBar = createEvalBar(evalBarContainerEl);

// State
let isFlipped = false;
let sideBoardsOn = true;
let currentMode: "play-to-end" | "turn-by-turn" = "play-to-end";
let currentPly = 0;
let isGameOver = false;
let awaitingNext = false;
let errorTimeout: number | null = null;

// Sizing eval bar to match main board height
function syncEvalBarHeight() {
  const rect = mainBoardEl.getBoundingClientRect();
  if (rect.height > 0) {
    evalBarContainerEl.style.height = `${rect.height}px`;
  }
}
window.addEventListener("resize", syncEvalBarHeight);
// Initial check after paint
setTimeout(syncEvalBarHeight, 50);

function updateNextButton() {
  // AwaitingNext derivation: arena lacks explicit awaiting-next event, so we derive:
  // turn-by-turn mode and game not over and engine-info received for current side to move.
  btnNext.disabled = !(currentMode === "turn-by-turn" && !isGameOver && awaitingNext);
}

function showError(message: string) {
  bannerError.textContent = message;
  bannerError.classList.remove("hidden");
  clearTimeout(errorTimeout ?? undefined);
  errorTimeout = window.setTimeout(() => {
    bannerError.classList.add("hidden");
  }, 6000);
}

// Initial position rendered at startpos
boards.reset();

// Setup UI listeners
btnNewGame.addEventListener("click", async () => {
  bannerResult.classList.add("hidden");
  bannerError.classList.add("hidden");
  isGameOver = false;
  btnSavePgn.disabled = true;
  awaitingNext = false;
  currentPly = 0;

  const rawFen = inputFen.value.trim();
  const startFen = rawFen.length > 0 ? rawFen : null;

  try {
    await invoke("new_game", { startFen });
    boards.reset(startFen || undefined);
    evalBar.reset();
    panels.reset();
    syncEvalBarHeight();
  } catch (err: unknown) {
  }
});

btnEngines.addEventListener("click", async () => {
  try {
    await openEngineDialog();
  } catch (err: unknown) {
    showError(`Engines dialog error: ${String(err)}`);
  }
});

btnMode.addEventListener("click", async () => {
  const newMode = currentMode === "play-to-end" ? "turn-by-turn" : "play-to-end";
  try {
    await invoke("set_mode", { mode: newMode });
    currentMode = newMode;
    btnMode.textContent = newMode === "play-to-end" ? "Mode: Play" : "Mode: Turn";
    updateNextButton();
  } catch (err: unknown) {
    showError(`Mode switch error: ${String(err)}`);
  }
});

btnNext.addEventListener("click", async () => {
  try {
    await invoke("next");
    awaitingNext = false;
    updateNextButton();
  } catch (err: unknown) {
    showError(`Next error: ${String(err)}`);
  }
});

btnMoveNow.addEventListener("click", async () => {
  try {
    await invoke("move_now");
  } catch (err: unknown) {
    showError(`Move now error: ${String(err)}`);
  }
});

btnFlip.addEventListener("click", () => {
  isFlipped = !isFlipped;
  boards.setFlipped(isFlipped);
  panels.setFlipped(isFlipped);
  evalBar.setFlipped(isFlipped);
});

btnSideBoards.addEventListener("click", async () => {
  const nextVal = !sideBoardsOn;
  try {
    await invoke("set_side_boards", { sideBoards: nextVal });
    sideBoardsOn = nextVal;
    btnSideBoards.textContent = sideBoardsOn ? "Side boards: ON" : "Side boards: OFF";
    btnSideBoards.classList.toggle("active", sideBoardsOn);
    arenaGridEl.classList.toggle("no-side-boards", !sideBoardsOn);
    boards.setSideBoards(sideBoardsOn);
    panels.setSideBoards(sideBoardsOn);
    syncEvalBarHeight();
  } catch (err: unknown) {
    showError(`Side boards error: ${String(err)}`);
  }
});

// Tauri Events
listen<PositionEvent>("position", (event) => {
  const p = event.payload;
  currentPly = p.ply;
  boards.updateMainPosition(p.fen, p.last_move, p.check, p.side_to_move);
  panels.updateMoveList(p.san_history);
  syncEvalBarHeight();

  // Reset awaitingNext on move arrival
  awaitingNext = false;
  updateNextButton();
});

listen<EngineInfo>("engine-info", (event) => {
  const info = event.payload;
  panels.updateInfo(info);
  const side = info.side.toLowerCase() as "white" | "black";
  boards.updateSideLines(side, info.depth, info.lines);

  // In turn-by-turn mode, receipt of engine-info signals that the engine is searching and ready for Next
  if (currentMode === "turn-by-turn" && !isGameOver) {
    awaitingNext = true;
    updateNextButton();
  }
});

listen<EvalEvent>("eval", (event) => {
  // Plan requirement: frontend drops eval events whose ply != current
  evalBar.update(event.payload, currentPly);
});

listen<GameOverEvent>("game-over", (event) => {
  const go = event.payload;
  isGameOver = true;
  awaitingNext = false;
  updateNextButton();

  bannerResult.textContent = `Game Over: ${go.result} — ${go.reason}`;
  bannerResult.classList.remove("hidden");
  btnSavePgn.disabled = false;
});

listen<EngineErrorEvent>("engine-error", (event) => {
  const err = event.payload;
  showError(`Engine Error [${err.side}]: ${err.message}`);
});

const btnSavePgn = document.getElementById("btn-save-pgn") as HTMLButtonElement;
const btnSaveFen = document.getElementById("btn-save-fen") as HTMLButtonElement;

async function saveTo(cmd: "save_pgn" | "save_fen", name: string, ext: string) {
  try {
    const path = await save({ defaultPath: name, filters: [{ name: ext.toUpperCase(), extensions: [ext] }] });
    if (path) await invoke(cmd, { path });
  } catch (err: unknown) {
    showError(`Save error: ${String(err)}`);
  }
}
btnSavePgn.addEventListener("click", () => saveTo("save_pgn", "game.pgn", "pgn"));
btnSaveFen.addEventListener("click", () => saveTo("save_fen", "position.fen", "fen"));
