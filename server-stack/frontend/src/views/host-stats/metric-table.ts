/** Lightweight sortable table: click `<th>` with `data-sort-key` (column index). */

export function bindSortableTable(table: HTMLTableElement): void {
  const thead = table.querySelector("thead");
  if (!thead) {
    return;
  }
  const ths = thead.querySelectorAll("th[data-sort-key]");
  let sortCol = -1;
  let sortDir: "asc" | "desc" = "asc";

  const parseCell = (text: string, colIdx: number): string | number => {
    const t = text.trim();
    if (/^-?\d+(\.\d+)?$/.test(t)) {
      return parseFloat(t);
    }
    if (/^\d+$/.test(t)) {
      return parseInt(t, 10);
    }
    const suffix = t.match(/^([\d.]+)\s*(B|KiB|MiB|GiB|TiB)/i);
    if (suffix) {
      const n = parseFloat(suffix[1]);
      const u = suffix[2].toLowerCase();
      const mul =
        u === "b"
          ? 1
          : u === "kib"
            ? 1024
            : u === "mib"
              ? 1024 ** 2
              : u === "gib"
                ? 1024 ** 3
                : u === "tib"
                  ? 1024 ** 4
                  : 1;
      return n * mul;
    }
    if (t.endsWith("%")) {
      return parseFloat(t) || 0;
    }
    if (colIdx === 0 && /^\d+$/.test(t)) {
      return parseInt(t, 10);
    }
    return t.toLowerCase();
  };

  const sort = (colIdx: number): void => {
    const tbody = table.querySelector("tbody");
    if (!tbody) {
      return;
    }
    const rows = Array.from(tbody.querySelectorAll("tr"));
    if (rows.length === 0) {
      return;
    }
    if (sortCol === colIdx) {
      sortDir = sortDir === "asc" ? "desc" : "asc";
    } else {
      sortCol = colIdx;
      sortDir = "asc";
    }
    for (const th of ths) {
      const el = th as HTMLElement;
      const idx = parseInt(el.dataset.sortKey ?? "-1", 10);
      el.setAttribute("aria-sort", idx === sortCol ? sortDir : "none");
    }
    rows.sort((a, b) => {
      const ca = a.cells[colIdx]?.textContent ?? "";
      const cb = b.cells[colIdx]?.textContent ?? "";
      const va = parseCell(ca, colIdx);
      const vb = parseCell(cb, colIdx);
      let cmp = 0;
      if (typeof va === "number" && typeof vb === "number") {
        cmp = va - vb;
      } else {
        cmp = String(va).localeCompare(String(vb));
      }
      return sortDir === "asc" ? cmp : -cmp;
    });
    for (const r of rows) {
      tbody.appendChild(r);
    }
  };

  for (const th of ths) {
    const el = th as HTMLElement;
    const idx = parseInt(el.dataset.sortKey ?? "-1", 10);
    if (idx < 0) {
      continue;
    }
    el.style.cursor = "pointer";
    el.tabIndex = 0;
    el.addEventListener("click", () => sort(idx));
    el.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" || ev.key === " ") {
        ev.preventDefault();
        sort(idx);
      }
    });
  }
}
