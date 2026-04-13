// --- SSE log streaming for running executions ---

function initLogStream() {
  const main = document.querySelector<HTMLElement>('main[data-execution-id]');
  if (!main) return;

  const executionId = main.dataset.executionId;
  const status = main.dataset.executionStatus;
  if (!executionId || status !== 'running') return;

  const stdoutEl = document.getElementById('stdout-log');
  const stderrEl = document.getElementById('stderr-log');
  const statusBadge = document.getElementById('status-badge');

  if (!stdoutEl) return;

  const source = new EventSource(`/executions/${executionId}/logs/stream`);

  source.addEventListener('stdout', (e: MessageEvent) => {
    if (stdoutEl.textContent === 'No output captured.') {
      stdoutEl.textContent = '';
    }
    stdoutEl.textContent += e.data;
    stdoutEl.scrollTop = stdoutEl.scrollHeight;
  });

  source.addEventListener('stderr', (e: MessageEvent) => {
    if (stderrEl && stderrEl.textContent === 'No output captured.') {
      stderrEl.textContent = '';
    }
    if (stderrEl) {
      stderrEl.textContent += e.data;
    }
  });

  source.addEventListener('done', (e: MessageEvent) => {
    source.close();
    try {
      const data = JSON.parse(e.data) as { status: string };
      if (statusBadge) {
        statusBadge.textContent = data.status;
        statusBadge.className = 'sw-badge ' + statusBadgeClass(data.status);
      }
    } catch {
      // ignore parse errors
    }
  });

  source.onerror = () => {
    source.close();
  };
}

function statusBadgeClass(status: string): string {
  if (status === 'success') return 'sw-badge-success';
  if (status === 'failed' || status === 'timed_out') return 'sw-badge-error';
  if (status === 'running') return 'sw-badge-info';
  if (status === 'pending' || status === 'pending_approval') return 'sw-badge-warning';
  return 'sw-badge-muted';
}

// --- Toast notifications ---

export function showToast(message: string, type: 'success' | 'error' | 'info' = 'info') {
  const container = document.getElementById('toasts');
  if (!container) return;

  const toast = document.createElement('div');
  toast.className = 'sw-toast ' + (
    type === 'success' ? 'sw-toast-success' :
    type === 'error'   ? 'sw-toast-error' :
                         'sw-toast-info'
  );
  toast.textContent = message;
  container.appendChild(toast);

  setTimeout(() => {
    toast.style.opacity = '0';
    setTimeout(() => toast.remove(), 300);
  }, 5000);
}

// Listen for HX-Trigger headers from HTMX responses.
document.addEventListener('htmx:afterOnLoad', (e: Event) => {
  const detail = (e as CustomEvent).detail as { xhr?: XMLHttpRequest };
  if (!detail.xhr) return;

  const trigger = detail.xhr.getResponseHeader('HX-Trigger');
  if (!trigger) return;

  try {
    const data = JSON.parse(trigger) as Record<string, unknown>;
    if (data.showToast && typeof data.showToast === 'object') {
      const t = data.showToast as { message?: string; type?: string };
      if (t.message) {
        showToast(t.message, (t.type as 'success' | 'error' | 'info') ?? 'info');
      }
    }
  } catch {
    // Plain string trigger — not a toast
  }
});

// --- Timestamp formatting ---

function formatRelativeTime(iso: string): string {
  const date = new Date(iso);
  const now = Date.now();
  const diffMs = now - date.getTime();
  const diffSec = Math.floor(diffMs / 1000);

  if (diffSec < 60) return 'just now';
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  const diffDays = Math.floor(diffHr / 24);
  if (diffDays < 30) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

function formatTimestamps() {
  document.querySelectorAll<HTMLElement>('[data-timestamp]').forEach((el) => {
    const iso = el.dataset.timestamp;
    if (!iso) return;
    el.textContent = formatRelativeTime(iso);
    el.title = iso;
  });
}

// Re-format timestamps after HTMX swaps inject new content.
document.addEventListener('htmx:afterSettle', () => {
  formatTimestamps();
});

// --- Init ---

document.addEventListener('DOMContentLoaded', () => {
  initLogStream();
  formatTimestamps();
});
