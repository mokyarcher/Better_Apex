type RecordId = {id: string};

// Pin the detail that was already visible, including the implicit first item.
export function historySelection<T extends RecordId>(previous: T[], next: T[], activeId: string) {
  const visibleId = previous.find(item => item.id === activeId)?.id ?? previous[0]?.id;
  const selectedId = next.find(item => item.id === visibleId)?.id ?? next[0]?.id ?? '';
  const newRecordId = next[0] && previous.length > 0 && !previous.some(item => item.id === next[0].id)
    ? next[0].id : '';
  return {selectedId, newRecordId};
}

// At most one database read in flight. An event during that read requests a
// follow-up read so a commit arriving after the query cannot be lost.
export function createRefreshQueue<T>(load: () => Promise<T>, apply: (value: T) => void) {
  let requested = false;
  let pending: Promise<void> | undefined;
  return function refresh(): Promise<void> {
    requested = true;
    if (!pending) {
      pending = Promise.resolve().then(async () => {
        try {
          while (requested) {
            requested = false;
            apply(await load());
          }
        } finally {
          pending = undefined;
        }
      });
    }
    return pending;
  };
}
