// Popover toggles — click trigger to open, click outside to close.
document.addEventListener('click', e => {
  const trigger = e.target.closest('[data-popover-toggle]');
  if (trigger) {
    const anchor = trigger.closest('.wf-pop-anchor');
    const pop = anchor && anchor.querySelector('.wf-popover');
    if (pop) {
      const wasOpen = pop.classList.contains('is-open');
      document.querySelectorAll('.wf-popover.is-open').forEach(p => p.classList.remove('is-open'));
      if (!wasOpen) pop.classList.add('is-open');
    }
    return;
  }
  if (!e.target.closest('.wf-popover')) {
    document.querySelectorAll('.wf-popover.is-open').forEach(p => p.classList.remove('is-open'));
  }
});

// Toast listener — wfToast custom events from htmx HX-Trigger headers.
// Uses textContent for the message to avoid XSS from server-provided strings.
document.body.addEventListener('wfToast', e => {
  const { kind = '', msg = '' } = e.detail || {};
  const host = document.getElementById('toast-host');
  if (!host) return;
  const t = document.createElement('div');
  t.className = 'wf-toast' + (kind ? ' ' + kind : '');
  const dot = document.createElement('span');
  dot.className = 'wf-dot';
  const span = document.createElement('span');
  span.textContent = msg;
  t.appendChild(dot);
  t.appendChild(span);
  host.appendChild(t);
  setTimeout(() => {
    t.style.opacity = '0';
    t.style.transition = 'opacity 200ms';
    setTimeout(() => t.remove(), 220);
  }, 2600);
});

// Relative timestamps — format [data-ts] elements as "3m ago", "2h ago", etc.
function formatTimestamps() {
  document.querySelectorAll('[data-ts]').forEach(el => {
    const iso = el.getAttribute('data-ts');
    if (!iso) return;
    const diff = Date.now() - new Date(iso).getTime();
    const secs = Math.floor(diff / 1000);
    let text;
    if (secs < 60) text = 'just now';
    else if (secs < 3600) text = Math.floor(secs / 60) + 'm ago';
    else if (secs < 86400) text = Math.floor(secs / 3600) + 'h ago';
    else if (secs < 604800) text = Math.floor(secs / 86400) + 'd ago';
    else text = new Date(iso).toLocaleDateString();
    el.textContent = text;
  });
}

document.addEventListener('DOMContentLoaded', formatTimestamps);
document.body.addEventListener('htmx:afterSettle', formatTimestamps);
