// src/boards.ts - Chessground instances for main board and 6 side boards
import { Chessground } from "@lichess-org/chessground";
import type { Api } from "@lichess-org/chessground/api";
import type { Key } from "@lichess-org/chessground/types";
import { formatScore, type PvLine } from "./panels";

export interface SideBoardSlot {
  container: HTMLElement;
  boardEl: HTMLElement;
  captionEl: HTMLElement;
  cg: Api;
}

export interface BoardManager {
  updateMainPosition(
    fen: string,
    lastMove: [string, string] | null,
    check: boolean,
    sideToMove: "white" | "black"
  ): void;
  updateSideLines(side: "white" | "black", depth: number | null, lines: PvLine[]): void;
  setFlipped(flipped: boolean): void;
  setSideBoards(enabled: boolean): void;
  reset(fen?: string): void;
  getMainApi(): Api;
}

export interface BoardElements {
  mainBoardContainer: HTMLElement;
  leftContainer: HTMLElement;
  rightContainer: HTMLElement;
}

function parseUciMove(uci: string): { orig: Key; dest: Key } | null {
  if (!uci || uci.length < 4) return null;
  const orig = uci.slice(0, 2) as Key;
  const dest = uci.slice(2, 4) as Key;
  return { orig, dest };
}

const formatScoreBrief = formatScore;

export function createBoards(elements: BoardElements): BoardManager {
  let isFlipped = false;
  let sideBoardsVisible = true;

  // Main board initialization
  const mainCg = Chessground(elements.mainBoardContainer, {
    viewOnly: true,
    coordinates: true,
    orientation: "white",
    animation: { enabled: true, duration: 150 },
  });

  // Create 3 side boards per side column
  function createColumnSlots(container: HTMLElement): SideBoardSlot[] {
    container.innerHTML = "";
    const slots: SideBoardSlot[] = [];
    for (let i = 1; i <= 3; i++) {
      const slotEl = document.createElement("div");
      slotEl.className = "side-board-slot";

      const boardEl = document.createElement("div");
      boardEl.className = "side-board-view";

      const captionEl = document.createElement("div");
      captionEl.className = "side-board-caption";
      captionEl.textContent = `PV ${i} · --`;

      slotEl.appendChild(boardEl);
      slotEl.appendChild(captionEl);
      container.appendChild(slotEl);

      const cg = Chessground(boardEl, {
        viewOnly: true,
        coordinates: false,
        orientation: "white",
        animation: { enabled: false },
      });

      slots.push({ container: slotEl, boardEl, captionEl, cg });
    }
    return slots;
  }

  const leftSlots = createColumnSlots(elements.leftContainer);
  const rightSlots = createColumnSlots(elements.rightContainer);

  // Cached lines and depths for White and Black
  const cachedPvData: Record<"white" | "black", { depth: number | null; lines: PvLine[] }> = {
    white: { depth: null, lines: [] },
    black: { depth: null, lines: [] },
  };

  function renderSideColumn(slots: SideBoardSlot[], side: "white" | "black") {
    const { depth, lines } = cachedPvData[side];
    const orientation = isFlipped ? "black" : "white";

    for (let i = 0; i < 3; i++) {
      const slot = slots[i];
      const pvIndex = i + 1;
      const line = lines.find((l) => l.multipv === pvIndex);

      slot.cg.set({ orientation });

      if (line && line.end_fen) {
        const arrow = line.first_move ? parseUciMove(line.first_move) : null;
        slot.cg.set({
          fen: line.end_fen,
          drawable: {
            autoShapes: arrow ? [{ orig: arrow.orig, dest: arrow.dest, brush: "green" }] : [],
          },
        });

        const scoreStr = formatScoreBrief(line.score);
        const depthStr = depth != null ? ` · d${depth}` : "";
        slot.captionEl.textContent = `PV ${pvIndex} · ${scoreStr}${depthStr}`;
      } else {
        slot.cg.set({
          drawable: { autoShapes: [] },
        });
        slot.captionEl.textContent = `PV ${pvIndex} · --`;
      }
    }
  }

  function renderAllSideBoards() {
    if (!sideBoardsVisible) return;
    // Left column = top-color engine. Right column = bottom-color engine.
    // Unflipped: top is black, bottom is white.
    // Flipped: top is white, bottom is black.
    const topColor: "white" | "black" = isFlipped ? "white" : "black";
    const bottomColor: "white" | "black" = isFlipped ? "black" : "white";

    renderSideColumn(leftSlots, topColor);
    renderSideColumn(rightSlots, bottomColor);
  }

  function updateOrientation() {
    const orientation = isFlipped ? "black" : "white";
    mainCg.set({ orientation });
    for (const slot of leftSlots) {
      slot.cg.set({ orientation });
    }
    for (const slot of rightSlots) {
      slot.cg.set({ orientation });
    }
    renderAllSideBoards();
  }

  return {
    updateMainPosition(fen, lastMove, check, sideToMove) {
      mainCg.set({
        fen,
        turnColor: sideToMove,
        check,
        lastMove: lastMove ? (lastMove as [Key, Key]) : undefined,
      });
    },

    updateSideLines(side, depth, lines) {
      cachedPvData[side] = { depth, lines };
      renderAllSideBoards();
    },

    setFlipped(flipped) {
      isFlipped = flipped;
      updateOrientation();
    },

    setSideBoards(enabled) {
      sideBoardsVisible = enabled;
      elements.leftContainer.style.display = enabled ? "" : "none";
      elements.rightContainer.style.display = enabled ? "" : "none";
      if (enabled) {
        renderAllSideBoards();
      }
    },

    reset(fen) {
      const startFen = fen || "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
      mainCg.set({
        fen: startFen,
        lastMove: undefined,
        check: false,
        turnColor: "white",
      });
      cachedPvData.white = { depth: null, lines: [] };
      cachedPvData.black = { depth: null, lines: [] };
      for (const slot of [...leftSlots, ...rightSlots]) {
        slot.cg.set({
          fen: startFen,
          drawable: { autoShapes: [] },
        });
        slot.captionEl.textContent = "PV --";
      }
    },

    getMainApi() {
      return mainCg;
    },
  };
}
