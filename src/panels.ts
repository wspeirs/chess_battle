// src/panels.ts - PV and search stats panels
export type ScoreValue =
  | { type: "cp" | "mate"; value: number }
  | { Cp: number }
  | { Mate: number };

export interface PvLine {
  multipv: number;
  score: ScoreValue | null;
  wdl?: [number, number, number] | null;
  san: string[];
  end_fen: string;
  first_move: string | null;
}

export interface EngineInfo {
  side: "white" | "black";
  name: string;
  depth: number | null;
  seldepth: number | null;
  nodes: number | null;
  nps: number | null;
  hashfull: number | null;
  tbhits: number | null;
  time_ms: number | null;
  lines: PvLine[];
}

export interface MoveHistoryItem {
  ply: number;
  san: string;
}

export function formatScore(score: ScoreValue | null | undefined): string {
  if (!score) return "--";
  if ("type" in score) {
    if (score.type === "mate") {
      const sign = score.value > 0 ? "+M" : "-M";
      return `${sign}${Math.abs(score.value)}`;
    }
    const cp = score.value / 100;
    const sign = cp > 0 ? "+" : "";
    return `${sign}${cp.toFixed(2)}`;
  }
  if ("Mate" in score) {
    const sign = score.Mate > 0 ? "+M" : "-M";
    return `${sign}${Math.abs(score.Mate)}`;
  }
  if ("Cp" in score) {
    const cp = score.Cp / 100;
    const sign = cp > 0 ? "+" : "";
    return `${sign}${cp.toFixed(2)}`;
  }
  return "--";
}

export function formatTime(ms: number | null): string {
  if (ms == null) return "--";
  if (ms < 1000) return `${ms}ms`;
  const sec = (ms / 1000).toFixed(1);
  return `${sec}s`;
}

export function formatNumber(n: number | null): string {
  if (n == null) return "--";
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
}

export interface PanelManager {
  updateInfo(info: EngineInfo): void;
  setFlipped(flipped: boolean): void;
  setSideBoards(enabled: boolean): void;
  updateMoveList(sanHistory: string[]): void;
  reset(): void;
}

export function createPanels(
  topContainer: HTMLElement,
  bottomContainer: HTMLElement,
  moveListContainer: HTMLElement
): PanelManager {
  let isFlipped = false;
  let sideBoardsEnabled = true;

  // Stored state for white and black engines
  const engineData: Record<"white" | "black", EngineInfo | null> = {
    white: null,
    black: null,
  };

  function renderPanel(container: HTMLElement, side: "white" | "black") {
    container.innerHTML = "";
    const info = engineData[side];

    const header = document.createElement("div");
    header.className = "panel-header";

    const titleSpan = document.createElement("span");
    titleSpan.className = "panel-title";
    const sideName = side === "white" ? "White" : "Black";
    titleSpan.textContent = `${info?.name || (side === "white" ? "White" : "Black")} (${sideName})`;

    const statsSpan = document.createElement("span");
    statsSpan.className = "panel-stats";
    if (info) {
      const stats: string[] = [];
      if (info.depth != null) {
        const dStr = info.seldepth != null ? `d${info.depth}/${info.seldepth}` : `d${info.depth}`;
        stats.push(dStr);
      }
      if (info.nodes != null) stats.push(`nodes ${formatNumber(info.nodes)}`);
      if (info.nps != null) stats.push(`nps ${formatNumber(info.nps)}`);
      if (info.hashfull != null) stats.push(`hash ${(info.hashfull / 10).toFixed(0)}%`);
      if (info.tbhits != null && info.tbhits > 0) stats.push(`tb ${formatNumber(info.tbhits)}`);
      if (info.time_ms != null) stats.push(`time ${formatTime(info.time_ms)}`);
      if (info.lines[0]?.wdl) {
        const [w, d, l] = info.lines[0].wdl;
        stats.push(`wdl ${w}/${d}/${l}`);
      }
      statsSpan.textContent = stats.join(" · ");
    } else {
      statsSpan.textContent = "idle";
    }

    header.appendChild(titleSpan);
    header.appendChild(statsSpan);
    container.appendChild(header);

    const pvLinesContainer = document.createElement("div");
    pvLinesContainer.className = "panel-pv-lines";

    if (info && info.lines.length > 0) {
      // If side boards are off, show PV1 only; otherwise show up to 3 PV lines
      const maxLines = sideBoardsEnabled ? 3 : 1;
      const linesToShow = info.lines.slice(0, maxLines);

      for (const line of linesToShow) {
        const lineEl = document.createElement("div");
        lineEl.className = "panel-pv-line";

        const scoreBadge = document.createElement("span");
        scoreBadge.className = "panel-score";
        scoreBadge.textContent = `[PV${line.multipv} ${formatScore(line.score)}]`;

        const movesSpan = document.createElement("span");
        movesSpan.className = "panel-moves";
        movesSpan.textContent = line.san.join(" ");

        lineEl.appendChild(scoreBadge);
        lineEl.appendChild(movesSpan);
        pvLinesContainer.appendChild(lineEl);
      }
    }

    container.appendChild(pvLinesContainer);
  }

  function renderBoth() {
    // Top container: top-color engine. When not flipped, Black is at top. When flipped, White is at top.
    const topSide: "white" | "black" = isFlipped ? "white" : "black";
    const bottomSide: "white" | "black" = isFlipped ? "black" : "white";

    renderPanel(topContainer, topSide);
    renderPanel(bottomContainer, bottomSide);
  }

  function renderMoveList(sanHistory: string[]) {
    moveListContainer.innerHTML = "";
    if (sanHistory.length === 0) return;

    const list = document.createElement("div");
    list.className = "move-list-compact";

    // Group into pairs: 1. e4 e5 2. Nf3 Nc6 ...
    const tokens: string[] = [];
    for (let i = 0; i < sanHistory.length; i += 2) {
      const moveNum = Math.floor(i / 2) + 1;
      const whiteMove = sanHistory[i];
      const blackMove = sanHistory[i + 1];
      if (blackMove) {
        tokens.push(`${moveNum}. ${whiteMove} ${blackMove}`);
      } else {
        tokens.push(`${moveNum}. ${whiteMove}`);
      }
    }
    list.textContent = tokens.join("  ");
    moveListContainer.appendChild(list);
    // Auto-scroll to end
    moveListContainer.scrollTop = moveListContainer.scrollHeight;
  }

  renderBoth();

  return {
    updateInfo(info: EngineInfo) {
      const sideKey = (info.side.toLowerCase() as "white" | "black");
      engineData[sideKey] = info;
      renderBoth();
    },
    setFlipped(flipped: boolean) {
      isFlipped = flipped;
      renderBoth();
    },
    setSideBoards(enabled: boolean) {
      sideBoardsEnabled = enabled;
      renderBoth();
    },
    updateMoveList(sanHistory: string[]) {
      renderMoveList(sanHistory);
    },
    reset() {
      engineData.white = null;
      engineData.black = null;
      renderBoth();
      moveListContainer.innerHTML = "";
    },
  };
}
