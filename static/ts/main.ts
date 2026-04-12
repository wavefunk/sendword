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
        // Update badge color based on terminal status.
        statusBadge.className = 'px-2 py-1 rounded text-xs font-medium ' + statusClass(data.status);
      }
    } catch {
      // ignore parse errors
    }
  });

  source.onerror = () => {
    source.close();
  };
}

function statusClass(status: string): string {
  if (status === 'success') return 'bg-green-100 text-green-800';
  if (status === 'failed' || status === 'timed_out') return 'bg-red-100 text-red-800';
  if (status === 'running') return 'bg-blue-100 text-blue-800';
  if (status === 'pending' || status === 'pending_approval') return 'bg-yellow-100 text-yellow-800';
  return 'bg-gray-100 text-gray-600';
}

// --- Toast notifications ---

export function showToast(message: string, type: 'success' | 'error' | 'info' = 'info') {
  const container = document.getElementById('toasts');
  if (!container) return;

  const toast = document.createElement('div');
  toast.className = [
    'px-4 py-3 rounded shadow-lg text-sm font-medium pointer-events-auto transition-opacity duration-300',
    type === 'success' ? 'bg-green-600 text-white' :
    type === 'error'   ? 'bg-red-600 text-white' :
                         'bg-gray-800 text-white',
  ].join(' ');
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

// --- Init ---

document.addEventListener('DOMContentLoaded', () => {
  initLogStream();
});
