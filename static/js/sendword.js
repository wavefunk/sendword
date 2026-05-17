(function () {
  function find(root, selector) {
    if (!root) return null;
    if (root.matches && root.matches(selector)) return root;
    return root.querySelector ? root.querySelector(selector) : null;
  }

  function eventElement(event) {
    if (event.target && event.target.closest) return event.target;
    return event.target && event.target.parentElement ? event.target.parentElement : null;
  }

  function filterHooks(query) {
    const q = (query || '').toLowerCase();
    document.querySelectorAll('#hook-list tr').forEach(row => {
      const name = row.getAttribute('data-hook-name') || '';
      const slug = row.getAttribute('data-hook-slug') || '';
      row.style.display = name.includes(q) || slug.includes(q) ? '' : 'none';
    });
  }

  function initHookFilter(root) {
    const filter = find(root, '[data-sendword-hook-filter]');
    if (!filter || filter.dataset.sendwordReady === 'true') return;
    filter.dataset.sendwordReady = 'true';
    filter.addEventListener('input', () => filterHooks(filter.value));
    filterHooks(filter.value);
  }

  function switchActivityTab(tab, trigger) {
    document.querySelectorAll('#activity-tabs button').forEach(button => {
      button.classList.remove('is-active');
    });
    trigger.classList.add('is-active');

    const executions = document.getElementById('activity-tab-executions');
    const attempts = document.getElementById('activity-tab-attempts');
    if (!executions || !attempts) return;

    if (tab === 'executions') {
      executions.classList.remove('wf-hidden');
      executions.classList.add('wf-f');
      attempts.classList.add('wf-hidden');
      attempts.classList.remove('wf-f');
    } else {
      executions.classList.add('wf-hidden');
      executions.classList.remove('wf-f');
      attempts.classList.remove('wf-hidden');
      attempts.classList.add('wf-f');
    }
  }

  function initActivityTabs(root) {
    const tabs = find(root, '#activity-tabs');
    if (!tabs || tabs.dataset.sendwordReady === 'true') return;
    tabs.dataset.sendwordReady = 'true';
    tabs.addEventListener('click', event => {
      const target = eventElement(event);
      const trigger = target && target.closest('[data-sendword-activity-tab]');
      if (!trigger) return;
      switchActivityTab(trigger.getAttribute('data-sendword-activity-tab'), trigger);
    });
  }

  function toggleAuthFields() {
    const mode = document.getElementById('auth_mode');
    const bearer = document.getElementById('bearer-fields');
    const hmac = document.getElementById('hmac-fields');
    if (!mode || !bearer || !hmac) return;
    bearer.style.display = mode.value === 'bearer' ? '' : 'none';
    hmac.style.display = mode.value === 'hmac' ? '' : 'none';
  }

  function updateExecutorField() {
    const type = document.getElementById('executor_type');
    const command = document.getElementById('command');
    const label = document.getElementById('command_label');
    const hint = document.getElementById('command_hint');
    if (!type || !command || !label || !hint) return;

    const copy = {
      shell: {
        label: 'Shell command',
        placeholder: 'make deploy',
        hint: 'Shell commands may use payload interpolation like {{ action }}.'
      },
      script: {
        label: 'Script path',
        placeholder: 'data/scripts/deploy.sh',
        hint: 'Executable scripts are run directly and need a shebang plus executable permissions.'
      },
      javascript: {
        label: 'JavaScript path',
        placeholder: 'data/scripts/deploy.js',
        hint: 'JavaScript scripts run with node and can read payload fields from process.env.'
      },
      python: {
        label: 'Python path',
        placeholder: 'data/scripts/deploy.py',
        hint: 'Python scripts run with python3, then python, and can read payload fields from os.environ.'
      },
      http: {
        label: 'HTTP URL',
        placeholder: 'https://example.com/webhook',
        hint: 'HTTP executors are view-only in this form and cannot be saved here.'
      }
    };

    const selected = copy[type.value] || copy.shell;
    label.textContent = selected.label;
    command.placeholder = selected.placeholder;
    hint.textContent = selected.hint;
  }

  function initHookForm(root) {
    const authMode = find(root, '#auth_mode');
    const executorType = find(root, '#executor_type');

    if (authMode && authMode.dataset.sendwordReady !== 'true') {
      authMode.dataset.sendwordReady = 'true';
      authMode.addEventListener('change', toggleAuthFields);
    }

    if (executorType && executorType.dataset.sendwordReady !== 'true') {
      executorType.dataset.sendwordReady = 'true';
      executorType.addEventListener('change', updateExecutorField);
    }

    toggleAuthFields();
    updateExecutorField();
  }

  function initReplayRedirect() {
    document.addEventListener('htmx:afterRequest', event => {
      const target = eventElement(event);
      const replay = target && target.closest('[data-sendword-replay]');
      if (!replay || !event.detail || !event.detail.successful) return;

      try {
        const body = JSON.parse(event.detail.xhr.responseText || '{}');
        if (body.execution_id) {
          window.location.assign('/executions/' + encodeURIComponent(body.execution_id));
        }
      } catch (_) {
        return;
      }
    });
  }

  function initFormConfirm() {
    document.addEventListener('submit', event => {
      const target = eventElement(event);
      const form = target && target.closest('[data-sendword-confirm]');
      if (!form) return;

      const message = form.getAttribute('data-sendword-confirm');
      if (message && !window.confirm(message)) {
        event.preventDefault();
      }
    });
  }

  function maybeReloadAfterSettle(event) {
    const target = eventElement(event);
    const trigger = target && target.closest('[data-sendword-reload-after-settle]');
    if (!trigger) return;

    const delay = Number.parseInt(
      trigger.getAttribute('data-sendword-reload-after-settle') || '0',
      10
    );
    window.setTimeout(() => window.location.reload(), Number.isFinite(delay) ? delay : 0);
  }

  function init(root) {
    initHookFilter(root);
    initActivityTabs(root);
    initHookForm(root);
  }

  initReplayRedirect();
  initFormConfirm();
  document.addEventListener('DOMContentLoaded', () => init(document));
  document.addEventListener('htmx:afterSettle', event => {
    maybeReloadAfterSettle(event);
    init(event.target || document);
  });
})();
