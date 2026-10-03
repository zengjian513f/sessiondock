const rowKey = (row, key) => `${row.node_id || ''}\n${row[key]}`;

// Exact transport snapshot expansion; UI rows never mutate this baseline.
export function expandRows(baseline, patch) {
    const old = new Map(baseline.map(row => [rowKey(row, patch.key), row]));
    const removed = new Set(patch.remove);
    const replaced = new Set(patch.upsert.map(item => item.id ?? rowKey(item.row, patch.key)));
    let rows = baseline.filter(row => !removed.has(rowKey(row, patch.key)) && !replaced.has(rowKey(row, patch.key)));
    for (const item of patch.upsert) {
      if (!Number.isInteger(item.index) || item.index < 0 || item.index > rows.length) throw new Error('列表增量顺序无效');
      let row = item.row;
      if (item.id !== undefined) {
        if (!old.has(item.id)) throw new Error('列表增量缺少条目');
        row = {...old.get(item.id), ...item.set};
        for (const field of item.unset) delete row[field];
        if (item.agents) row.agent_items = expandRows(row.agent_items, item.agents);
      }
      rows.splice(item.index, 0, row);
    }
    if (patch.order) {
      const byId = new Map(rows.map(row => [rowKey(row, patch.key), row]));
      rows = patch.order.map(id => {
        if (!byId.has(id)) throw new Error('列表增量缺少条目');
        const row = byId.get(id); byId.delete(id); return row;
      });
      if (byId.size) throw new Error('列表增量顺序不完整');
    }
    return rows;
  }

export function expand(wire, baseline) {
    const delta = wire.list_delta;
    if (!delta) return wire;
    if (!baseline || delta.base !== baseline.list_version) throw new Error('列表同步基线已失效');
    const result = {...wire};
    for (const [name, patch] of Object.entries(delta.collections)) result[name] = expandRows(baseline[name], patch);
    delete result.list_delta;
    delete result.list_unchanged;
    return result;
  }
