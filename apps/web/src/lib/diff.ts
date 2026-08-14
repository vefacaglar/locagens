export interface DiffRow {
  type: 'context' | 'add' | 'del';
  oldNo: number | null;
  newNo: number | null;
  text: string;
}

/**
 * Minimal LCS-based line diff producing red/green rows for the permission card.
 * Falls back to a plain "remove all / add all" rendering for very large inputs
 * to avoid the O(n*m) table blowing up.
 */
export function lineDiff(oldText: string, newText: string): DiffRow[] {
  const a = oldText.length ? oldText.split('\n') : [];
  const b = newText.length ? newText.split('\n') : [];

  const MAX = 1500;
  if (a.length > MAX || b.length > MAX) {
    const rows: DiffRow[] = [];
    a.forEach((t, i) => rows.push({ type: 'del', oldNo: i + 1, newNo: null, text: t }));
    b.forEach((t, i) => rows.push({ type: 'add', oldNo: null, newNo: i + 1, text: t }));
    return rows;
  }

  const n = a.length;
  const m = b.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }

  const rows: DiffRow[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      rows.push({ type: 'context', oldNo: i + 1, newNo: j + 1, text: a[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      rows.push({ type: 'del', oldNo: i + 1, newNo: null, text: a[i] });
      i++;
    } else {
      rows.push({ type: 'add', oldNo: null, newNo: j + 1, text: b[j] });
      j++;
    }
  }
  while (i < n) {
    rows.push({ type: 'del', oldNo: i + 1, newNo: null, text: a[i] });
    i++;
  }
  while (j < m) {
    rows.push({ type: 'add', oldNo: null, newNo: j + 1, text: b[j] });
    j++;
  }
  return rows;
}

// Context collapsing lives in DiffView.vue, which keeps the full rows and
// expands hidden ranges on click instead of discarding them here.
