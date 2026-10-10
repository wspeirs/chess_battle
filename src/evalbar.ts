import "./evalbar.css";

export interface EvalScore {
  type: "cp" | "mate";
  value: number;
}

export interface EvalEvent {
  ply: number;
  depth: number;
  score: EvalScore;
  wdl?: number[];
  white_pct: number;
}

export interface EvalBar {
  update(e: EvalEvent, currentPly: number): void;
  setFlipped(b: boolean): void;
  hide(): void;
  reset(): void;
}

function formatScore(score: EvalScore): string {
  // Positive score is White advantage.
  // When flipped (Black at bottom), does label reflect bottom side or white?
  // Standard chess convention (+0.35 is always white or leading end? Plan: "score label (+0.35, M5) at the leading end")
  // Let's format value from White perspective (+0.35, -1.20, M5, -M3)
  if (score.type === "mate") {
    const sign = score.value > 0 ? "+M" : "-M";
    return `${sign}${Math.abs(score.value)}`;
  }
  const cp = score.value / 100;
  const sign = cp > 0 ? "+" : "";
  return `${sign}${cp.toFixed(2)}`;
}

export function createEvalBar(container: HTMLElement): EvalBar {
  container.innerHTML = "";

  const bar = document.createElement("div");
  bar.className = "eval-bar";

  const fill = document.createElement("div");
  fill.className = "eval-bar-fill";
  fill.style.height = "50%";

  const labelBottom = document.createElement("div");
  labelBottom.className = "eval-bar-label label-bottom";

  const labelTop = document.createElement("div");
  labelTop.className = "eval-bar-label label-top";

  bar.appendChild(fill);
  bar.appendChild(labelBottom);
  bar.appendChild(labelTop);
  container.appendChild(bar);
  let isFlipped = false;
  let lastEvent: EvalEvent | null = null;

  function render() {
    if (!lastEvent) {
      fill.style.height = "50%";
      labelBottom.textContent = "";
      labelTop.textContent = "";
      bar.title = "";
      return;
    }

    // white_pct is White's expected score (0..100)
    // If not flipped (White at bottom), fill height is white_pct%
    // If flipped (Black at bottom), fill height is (100 - white_pct)%
    const bottomPct = isFlipped ? 100 - lastEvent.white_pct : lastEvent.white_pct;
    const clampedPct = Math.max(0, Math.min(100, bottomPct));
    fill.style.height = `${clampedPct}%`;

    const text = formatScore(lastEvent.score);
    // Position label at leading end: if bottom side is winning (bottomPct >= 50), show at bottom, else top
    if (bottomPct >= 50) {
      labelBottom.textContent = text;
      labelTop.textContent = "";
    } else {
      labelTop.textContent = text;
      labelBottom.textContent = "";
    }

    bar.title = `depth ${lastEvent.depth}`;
  }

  return {
    update(e: EvalEvent, currentPly: number) {
      if (e.ply !== currentPly) {
        return;
      }
      lastEvent = e;
      bar.style.display = "flex";
      render();
    },
    setFlipped(b: boolean) {
      isFlipped = b;
      render();
    },
    hide() {
      bar.style.display = "none";
    },
    reset() {
      lastEvent = null;
      render();
    },
  };
}
