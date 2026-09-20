/** Schedules a one-shot vault lock. Leaving System Settings arms this timer;
 *  reopening the settings calls the returned cancel. Cancellation is
 *  idempotent and the timer never reschedules itself. */
export function scheduleVaultLock(run: () => void, delayMs = 60_000): () => void {
  let cancelled = false;
  const handle = setTimeout(() => {
    if (cancelled) return;
    cancelled = true;
    run();
  }, delayMs);
  return () => {
    if (cancelled) return;
    cancelled = true;
    clearTimeout(handle);
  };
}
